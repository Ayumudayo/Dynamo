use std::{
    collections::BTreeMap,
    future::Future,
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
use futures_util::stream::{self, StreamExt};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct YahooSessionSnapshot {
    crumb: Option<String>,
    cookies: BTreeMap<String, String>,
}

impl YahooSessionSnapshot {
    fn is_usable(&self) -> bool {
        self.crumb
            .as_deref()
            .is_some_and(|crumb| !crumb.trim().is_empty())
            && !self.cookies.is_empty()
    }
}

impl From<&YahooSession> for YahooSessionSnapshot {
    fn from(session: &YahooSession) -> Self {
        Self {
            crumb: session.crumb.clone(),
            cookies: session.cookies.clone(),
        }
    }
}

#[derive(Clone)]
pub struct YahooFinanceClient {
    client: Client,
    session_repo: Option<Arc<dyn ProviderStateRepository>>,
    session: Arc<Mutex<YahooSession>>,
    refresh_guard: Arc<Mutex<()>>,
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
            refresh_guard: Arc::new(Mutex::new(())),
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
        if let Some(value) = loaded
            && let Ok(state) = serde_json::from_value::<PersistedYahooSession>(value)
        {
            let mut session = self.session.lock().await;
            session.crumb = state.crumb;
            session.cookies = state.cookies;
        }

        self.loaded_from_repo.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn persist_session_snapshot(&self, session: &YahooSessionSnapshot) -> Result<(), Error> {
        let Some(repo) = &self.session_repo else {
            return Ok(());
        };

        let value = serde_json::to_value(PersistedYahooSession {
            crumb: session.crumb.clone(),
            cookies: session.cookies.clone(),
        })?;
        repo.save_json(PROVIDER_ID, value).await
    }

    async fn send_get(
        &self,
        url: &str,
        extra_headers: HeaderMap,
        cookies: &BTreeMap<String, String>,
    ) -> Result<Response, Error> {
        let mut request = self.client.get(url).headers(extra_headers);
        if !cookies.is_empty() {
            request = request.header(COOKIE, build_cookie_header(cookies));
        }
        Ok(request.send().await?)
    }

    async fn send_post_form(
        &self,
        url: &str,
        form_body: String,
        extra_headers: HeaderMap,
        cookies: &BTreeMap<String, String>,
    ) -> Result<Response, Error> {
        let mut request = self.client.post(url).headers(extra_headers).body(form_body);
        if !cookies.is_empty() {
            request = request.header(COOKIE, build_cookie_header(cookies));
        }
        Ok(request.send().await?)
    }

    async fn session_snapshot(&self) -> YahooSessionSnapshot {
        let session = self.session.lock().await;
        YahooSessionSnapshot::from(&*session)
    }

    fn refresh_needed(
        current: &YahooSessionSnapshot,
        expected_stale: Option<&YahooSessionSnapshot>,
    ) -> bool {
        if !current.is_usable() {
            return true;
        }

        expected_stale.is_some_and(|stale| current == stale)
    }

    async fn refresh_session_with<F, Fut>(
        &self,
        expected_stale: Option<&YahooSessionSnapshot>,
        refresh: F,
    ) -> Result<YahooSessionSnapshot, Error>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<YahooSessionSnapshot, Error>>,
    {
        self.ensure_loaded_from_repo().await?;

        let current = self.session_snapshot().await;
        if !Self::refresh_needed(&current, expected_stale) {
            return Ok(current);
        }

        let _refresh_guard = self.refresh_guard.lock().await;
        let current = self.session_snapshot().await;
        if !Self::refresh_needed(&current, expected_stale) {
            return Ok(current);
        }

        let refreshed = refresh().await?;
        if !refreshed.is_usable() {
            return Err(anyhow::anyhow!(
                "Yahoo session refresh did not produce a usable crumb/cookie snapshot"
            ));
        }

        {
            let mut session = self.session.lock().await;
            session.crumb = refreshed.crumb.clone();
            session.cookies = refreshed.cookies.clone();
        }
        self.persist_session_snapshot(&refreshed).await?;
        Ok(refreshed)
    }

    async fn perform_session_refresh(&self) -> Result<YahooSessionSnapshot, Error> {
        let mut session = YahooSession::default();
        let mut response = self
            .send_get(QUOTE_PAGE_URL, html_headers(None, None), &session.cookies)
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
                        &session.cookies,
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
            .send_get(
                GET_CRUMB_URL,
                crumb_headers(QUOTE_PAGE_URL),
                &session.cookies,
            )
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
        Ok(YahooSessionSnapshot::from(&session))
    }

    async fn refresh_session(
        &self,
        expected_stale: Option<&YahooSessionSnapshot>,
    ) -> Result<YahooSessionSnapshot, Error> {
        self.refresh_session_with(expected_stale, || self.perform_session_refresh())
            .await
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
                &session.cookies,
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
                &session.cookies,
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
                &session.cookies,
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
                &session.cookies,
            )
            .await?;
        capture_set_cookies(copy_response.headers(), session)?;

        if let Some(final_url) = header_location(copy_response.headers(), &copy_url)? {
            let final_response = self
                .send_get(
                    &final_url,
                    html_headers(Some(&final_url), Some(&copy_url)),
                    &session.cookies,
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

        let session = self.usable_session_snapshot().await?;
        let first_attempt = self
            .fetch_quote_summary_with_snapshot(symbol, &session)
            .await;

        match first_attempt {
            Ok(result) => Ok(result),
            Err(error) if allow_retry && looks_like_auth_error(&error) => {
                self.refresh_session(Some(&session)).await?;
                self.fetch_quote_summary_once(symbol).await
            }
            Err(error) => Err(error),
        }
    }

    async fn usable_session_snapshot(&self) -> Result<YahooSessionSnapshot, Error> {
        let snapshot = self.session_snapshot().await;
        if snapshot.is_usable() {
            return Ok(snapshot);
        }

        self.refresh_session(None).await
    }

    async fn fetch_quote_summary_with_snapshot(
        &self,
        symbol: &str,
        session: &YahooSessionSnapshot,
    ) -> Result<Option<QuoteSummaryResult>, Error> {
        let crumb = session
            .crumb
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Yahoo crumb is not available"))?;
        let url = format!(
            "https://query2.finance.yahoo.com/v10/finance/quoteSummary/{symbol}?formatted=false&modules={QUOTE_SUMMARY_MODULES}&crumb={crumb}"
        );
        let response = self
            .send_get(&url, json_headers(symbol), &session.cookies)
            .await?;
        let status = response.status();
        let body = response.text().await?;

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

    async fn fetch_quote_summary_once(
        &self,
        symbol: &str,
    ) -> Result<Option<QuoteSummaryResult>, Error> {
        let session = self.usable_session_snapshot().await?;
        self.fetch_quote_summary_with_snapshot(symbol, &session)
            .await
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
        let client = self.clone();
        Ok(collect_quotes_bounded(symbols, 4, move |symbol| {
            let client = client.clone();
            async move { client.fetch_quote(&symbol).await }
        })
        .await)
    }
}

async fn collect_quotes_bounded<F, Fut>(
    symbols: &[String],
    concurrency_limit: usize,
    fetch_one: F,
) -> Vec<Result<StockQuote, String>>
where
    F: Fn(String) -> Fut + Clone + Send,
    Fut: Future<Output = Result<Option<StockQuote>, Error>> + Send,
{
    let limit = concurrency_limit.max(1);
    let mut results = stream::iter(symbols.iter().cloned().enumerate())
        .map(|(index, symbol)| {
            let fetch_one = fetch_one.clone();
            async move {
                let result = match fetch_one(symbol).await {
                    Ok(Some(quote)) => Ok(quote),
                    Ok(None) => Err("Invalid Ticker".to_string()),
                    Err(error) => Err(error.to_string()),
                };
                (index, result)
            }
        })
        .buffer_unordered(limit)
        .collect::<Vec<_>>()
        .await;
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
impl YahooFinanceClient {
    pub(crate) async fn test_set_session(&self, crumb: Option<&str>, cookies: &[(&str, &str)]) {
        let mut session = self.session.lock().await;
        session.crumb = crumb.map(str::to_string);
        session.cookies = cookies
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
    }

    pub(crate) async fn test_session_snapshot(&self) -> (Option<String>, BTreeMap<String, String>) {
        let session = self.session_snapshot().await;
        (session.crumb, session.cookies)
    }

    pub(crate) async fn test_refresh_session_with<F, Fut>(
        &self,
        expected_snapshot: (Option<String>, BTreeMap<String, String>),
        refresh: F,
    ) -> Result<(Option<String>, BTreeMap<String, String>), Error>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(Option<String>, Vec<(String, String)>), Error>>,
    {
        let expected_snapshot = YahooSessionSnapshot {
            crumb: expected_snapshot.0,
            cookies: expected_snapshot.1,
        };
        let refreshed = self
            .refresh_session_with(Some(&expected_snapshot), || async move {
                let (crumb, cookies) = refresh().await?;
                Ok(YahooSessionSnapshot {
                    crumb,
                    cookies: cookies.into_iter().collect(),
                })
            })
            .await?;
        Ok((refreshed.crumb, refreshed.cookies))
    }

    pub(crate) async fn test_collect_quotes_bounded<F, Fut>(
        symbols: &[String],
        concurrency_limit: usize,
        fetch_one: F,
    ) -> Vec<Result<StockQuote, String>>
    where
        F: Fn(String) -> Fut + Clone + Send,
        Fut: Future<Output = Result<Option<StockQuote>, Error>> + Send,
    {
        collect_quotes_bounded(symbols, concurrency_limit, fetch_one).await
    }
}
