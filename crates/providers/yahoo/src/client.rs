use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use dynamo_domain_stock::StockQuote;
use dynamo_repositories::ProviderStateRepository;
use dynamo_service_stock::{Error, StockQuoteService};
use reqwest::{
    Client, Response,
    header::{ACCEPT, COOKIE, HeaderMap},
    redirect::Policy,
};
use tokio::sync::Mutex;
use tracing::warn;

use crate::{
    consent::build_consent_form_body,
    constants::{
        DEFAULT_USER_AGENT, GET_CRUMB_URL, PROVIDER_ID, QUOTE_PAGE_URL, QUOTE_SUMMARY_MODULES,
    },
    headers::{crumb_headers, form_headers, header_location, html_headers, json_headers},
    models::{ChartEnvelope, ChartResult, QuoteSummaryEnvelope, QuoteSummaryResult},
    quote::{looks_like_auth_error, merge_quote},
    session::{PersistedYahooSession, YahooSession, build_cookie_header, capture_set_cookies},
};

#[derive(Clone)]
pub struct YahooFinanceClient {
    client: Client,
    session_repo: Option<Arc<dyn ProviderStateRepository>>,
    session: Arc<Mutex<YahooSession>>,
    loaded_from_repo: Arc<AtomicBool>,
    load_guard: Arc<Mutex<()>>,
}

impl YahooFinanceClient {
    pub fn new(session_repo: Option<Arc<dyn ProviderStateRepository>>) -> Result<Self, Error> {
        let client = Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .timeout(Duration::from_secs(15))
            .redirect(Policy::none())
            .build()?;

        Ok(Self {
            client,
            session_repo,
            session: Arc::new(Mutex::new(YahooSession::default())),
            loaded_from_repo: Arc::new(AtomicBool::new(false)),
            load_guard: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) async fn ensure_loaded_from_repo(&self) -> Result<(), Error> {
        if self.loaded_from_repo.load(Ordering::SeqCst) {
            return Ok(());
        }

        let _guard = self.load_guard.lock().await;
        if self.loaded_from_repo.load(Ordering::SeqCst) {
            return Ok(());
        }

        let Some(repo) = &self.session_repo else {
            self.loaded_from_repo.store(true, Ordering::SeqCst);
            return Ok(());
        };

        let loaded = repo.load_json(PROVIDER_ID).await?;
        if let Some(value) = loaded {
            if let Ok(state) = serde_json::from_value::<PersistedYahooSession>(value) {
                let mut session = self.session.lock().await;
                session.crumb = state.crumb;
                session.cookies = state.cookies;
            }
        }

        self.loaded_from_repo.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn persist_session(&self) -> Result<(), Error> {
        let Some(repo) = &self.session_repo else {
            return Ok(());
        };

        let session = self.session.lock().await;
        let value = serde_json::to_value(PersistedYahooSession {
            crumb: session.crumb.clone(),
            cookies: session.cookies.clone(),
        })?;
        drop(session);

        repo.save_json(PROVIDER_ID, value).await
    }

    async fn send_get(
        &self,
        url: &str,
        extra_headers: HeaderMap,
        session: &YahooSession,
    ) -> Result<Response, Error> {
        let mut request = self.client.get(url).headers(extra_headers);
        if !session.cookies.is_empty() {
            request = request.header(COOKIE, build_cookie_header(&session.cookies));
        }
        Ok(request.send().await?)
    }

    async fn send_post_form(
        &self,
        url: &str,
        form_body: String,
        extra_headers: HeaderMap,
        session: &YahooSession,
    ) -> Result<Response, Error> {
        let mut request = self.client.post(url).headers(extra_headers).body(form_body);
        if !session.cookies.is_empty() {
            request = request.header(COOKIE, build_cookie_header(&session.cookies));
        }
        Ok(request.send().await?)
    }

    async fn refresh_session(&self) -> Result<(), Error> {
        self.ensure_loaded_from_repo().await?;

        let mut session = self.session.lock().await;
        session.crumb = None;
        session.cookies.clear();

        let mut response = self
            .send_get(QUOTE_PAGE_URL, html_headers(None, None), &session)
            .await?;
        capture_set_cookies(response.headers(), &mut session)?;

        if let Some(location) = header_location(response.headers(), QUOTE_PAGE_URL)? {
            if location.contains("guce.yahoo.com") {
                self.handle_consent_flow(&mut session, &location).await?;
            } else {
                response = self
                    .send_get(
                        &location,
                        html_headers(Some(&location), Some(QUOTE_PAGE_URL)),
                        &session,
                    )
                    .await?;
                capture_set_cookies(response.headers(), &mut session)?;
            }
        }

        if session.cookies.is_empty() {
            return Err(anyhow::anyhow!(
                "Yahoo session refresh did not return any cookies"
            ));
        }

        let crumb_response = self
            .send_get(GET_CRUMB_URL, crumb_headers(QUOTE_PAGE_URL), &session)
            .await?;
        capture_set_cookies(crumb_response.headers(), &mut session)?;

        if !crumb_response.status().is_success() {
            let status = crumb_response.status();
            let body = crumb_response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Yahoo crumb fetch failed with status {status}: {body}"
            ));
        }

        let crumb = crumb_response.text().await?;
        if crumb.trim().is_empty() {
            return Err(anyhow::anyhow!("Yahoo crumb response was empty"));
        }

        session.crumb = Some(crumb.trim().to_string());
        drop(session);
        self.persist_session().await?;
        Ok(())
    }

    async fn handle_consent_flow(
        &self,
        session: &mut YahooSession,
        consent_url: &str,
    ) -> Result<(), Error> {
        let consent_response = self
            .send_get(
                consent_url,
                html_headers(Some(consent_url), Some(QUOTE_PAGE_URL)),
                session,
            )
            .await?;
        capture_set_cookies(consent_response.headers(), session)?;

        let Some(collect_url) = header_location(consent_response.headers(), consent_url)? else {
            return Err(anyhow::anyhow!(
                "Yahoo consent redirect did not provide a collectConsent location"
            ));
        };

        let collect_response = self
            .send_get(
                &collect_url,
                html_headers(Some(&collect_url), Some(consent_url)),
                session,
            )
            .await?;
        capture_set_cookies(collect_response.headers(), session)?;
        let collect_body = collect_response.text().await?;
        let form_body = build_consent_form_body(&collect_body)?;

        let submit_response = self
            .send_post_form(
                &collect_url,
                form_body,
                form_headers(&collect_url, Some(consent_url)),
                session,
            )
            .await?;
        capture_set_cookies(submit_response.headers(), session)?;

        let Some(copy_url) = header_location(submit_response.headers(), &collect_url)? else {
            return Err(anyhow::anyhow!(
                "Yahoo consent submit did not provide a copyConsent redirect"
            ));
        };

        let copy_response = self
            .send_get(
                &copy_url,
                html_headers(Some(&copy_url), Some(&collect_url)),
                session,
            )
            .await?;
        capture_set_cookies(copy_response.headers(), session)?;

        if let Some(final_url) = header_location(copy_response.headers(), &copy_url)? {
            let final_response = self
                .send_get(
                    &final_url,
                    html_headers(Some(&final_url), Some(&copy_url)),
                    session,
                )
                .await?;
            capture_set_cookies(final_response.headers(), session)?;
        }

        Ok(())
    }

    async fn fetch_chart_internal(&self, symbol: &str) -> Result<Option<ChartResult>, Error> {
        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?interval=1d&range=1d&includePrePost=true"
        );
        let response = self
            .client
            .get(&url)
            .header(ACCEPT, "application/json")
            .send()
            .await?
            .error_for_status()?;
        let envelope = response.json::<ChartEnvelope>().await?;

        if let Some(result) = envelope.chart.result.and_then(|mut values| values.pop()) {
            return Ok(Some(result));
        }

        if let Some(error) = envelope.chart.error.and_then(|value| value.description) {
            warn!(symbol, error = %error, "Yahoo chart request returned an application error");
        }

        Ok(None)
    }

    async fn fetch_quote_summary_internal(
        &self,
        symbol: &str,
        allow_retry: bool,
    ) -> Result<Option<QuoteSummaryResult>, Error> {
        self.ensure_loaded_from_repo().await?;

        let first_attempt = self.fetch_quote_summary_once(symbol).await;

        match first_attempt {
            Ok(result) => Ok(result),
            Err(error) if allow_retry && looks_like_auth_error(&error) => {
                self.refresh_session().await?;
                self.fetch_quote_summary_once(symbol).await
            }
            Err(error) => Err(error),
        }
    }

    async fn fetch_quote_summary_once(
        &self,
        symbol: &str,
    ) -> Result<Option<QuoteSummaryResult>, Error> {
        {
            let session = self.session.lock().await;
            if session.crumb.is_none() || session.cookies.is_empty() {
                drop(session);
                self.refresh_session().await?;
            }
        }

        let session = self.session.lock().await;
        let crumb = session
            .crumb
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Yahoo crumb is not available"))?;
        let url = format!(
            "https://query2.finance.yahoo.com/v10/finance/quoteSummary/{symbol}?formatted=false&modules={QUOTE_SUMMARY_MODULES}&crumb={crumb}"
        );
        let response = self.send_get(&url, json_headers(symbol), &session).await?;
        let status = response.status();
        let body = response.text().await?;
        drop(session);

        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Yahoo quoteSummary request failed with status {}: {}",
                status,
                body
            ));
        }

        let envelope = serde_json::from_str::<QuoteSummaryEnvelope>(&body)?;
        if let Some(error) = envelope.quote_summary.error {
            return Err(anyhow::anyhow!(
                "Yahoo quoteSummary returned {}: {}",
                error.code.unwrap_or_else(|| "UnknownError".to_string()),
                error
                    .description
                    .unwrap_or_else(|| "unknown description".to_string())
            ));
        }

        Ok(envelope
            .quote_summary
            .result
            .and_then(|mut values| values.pop()))
    }
}

#[async_trait]
impl StockQuoteService for YahooFinanceClient {
    async fn fetch_quote(&self, symbol: &str) -> Result<Option<StockQuote>, Error> {
        let Some(chart) = self.fetch_chart_internal(symbol).await? else {
            return Ok(None);
        };

        let summary = match self.fetch_quote_summary_internal(symbol, true).await {
            Ok(value) => value,
            Err(error) => {
                warn!(symbol, error = %error, "Yahoo quoteSummary enrichment failed; returning chart-only quote");
                None
            }
        };

        Ok(Some(merge_quote(chart, summary)))
    }

    async fn fetch_quotes(
        &self,
        symbols: &[String],
    ) -> Result<Vec<Result<StockQuote, String>>, Error> {
        let mut quotes = Vec::with_capacity(symbols.len());
        for symbol in symbols {
            match self.fetch_quote(symbol).await {
                Ok(Some(quote)) => quotes.push(Ok(quote)),
                Ok(None) => quotes.push(Err("Invalid Ticker".to_string())),
                Err(error) => quotes.push(Err(error.to_string())),
            }
        }
        Ok(quotes)
    }
}
