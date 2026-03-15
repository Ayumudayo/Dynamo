use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use dynamo_core::{Error, ProviderStateRepository, StockQuote, StockQuoteService};
use reqwest::{
    Client, Response,
    header::{
        ACCEPT, CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, LOCATION, ORIGIN, REFERER, SET_COOKIE,
    },
    redirect::Policy,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::warn;
use url::Url;

const PROVIDER_ID: &str = "yahoo_finance";
const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) dynamo-rs/0.1 Safari/537.36";
const QUOTE_PAGE_URL: &str = "https://finance.yahoo.com/quote/AAPL";
const GET_CRUMB_URL: &str = "https://query1.finance.yahoo.com/v1/test/getcrumb";
const QUOTE_SUMMARY_MODULES: &str =
    "price,summaryDetail,defaultKeyStatistics,financialData,quoteType,summaryProfile";

#[derive(Clone)]
pub struct YahooFinanceClient {
    client: Client,
    session_repo: Option<Arc<dyn ProviderStateRepository>>,
    session: Arc<Mutex<YahooSession>>,
    loaded_from_repo: Arc<AtomicBool>,
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
        })
    }

    async fn ensure_loaded_from_repo(&self) -> Result<(), Error> {
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

    async fn fetch_chart_internal(&self, symbol: &str) -> Result<Option<ChartMeta>, Error> {
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
            return Ok(Some(result.meta));
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
            match self.fetch_chart_internal(symbol).await {
                Ok(Some(chart)) => quotes.push(Ok(merge_quote(chart, None))),
                Ok(None) => quotes.push(Err("Invalid Ticker".to_string())),
                Err(error) => quotes.push(Err(error.to_string())),
            }
        }
        Ok(quotes)
    }
}

#[derive(Debug, Default)]
struct YahooSession {
    crumb: Option<String>,
    cookies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedYahooSession {
    crumb: Option<String>,
    #[serde(default)]
    cookies: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ChartEnvelope {
    chart: ChartResponse,
}

#[derive(Debug, Deserialize)]
struct ChartResponse {
    result: Option<Vec<ChartResult>>,
    error: Option<YahooApiError>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChartResult {
    meta: ChartMeta,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChartMeta {
    symbol: String,
    short_name: Option<String>,
    long_name: Option<String>,
    currency: Option<String>,
    regular_market_price: Option<f64>,
    regular_market_day_high: Option<f64>,
    regular_market_day_low: Option<f64>,
    regular_market_volume: Option<f64>,
    chart_previous_close: Option<f64>,
    current_trading_period: Option<CurrentTradingPeriod>,
}

#[derive(Debug, Clone, Deserialize)]
struct CurrentTradingPeriod {
    pre: TradingPeriod,
    regular: TradingPeriod,
    post: TradingPeriod,
}

#[derive(Debug, Clone, Deserialize)]
struct TradingPeriod {
    start: i64,
    end: i64,
}

#[derive(Debug, Deserialize)]
struct QuoteSummaryEnvelope {
    #[serde(rename = "quoteSummary")]
    quote_summary: QuoteSummaryResponse,
}

#[derive(Debug, Deserialize)]
struct QuoteSummaryResponse {
    result: Option<Vec<QuoteSummaryResult>>,
    error: Option<YahooApiError>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteSummaryResult {
    price: Option<PriceModule>,
    summary_detail: Option<SummaryDetailModule>,
    default_key_statistics: Option<DefaultKeyStatisticsModule>,
    financial_data: Option<FinancialDataModule>,
    quote_type: Option<QuoteTypeModule>,
    summary_profile: Option<SummaryProfileModule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PriceModule {
    short_name: Option<String>,
    long_name: Option<String>,
    currency: Option<String>,
    quote_type: Option<String>,
    exchange_name: Option<String>,
    market_state: Option<String>,
    regular_market_price: Option<NumericField>,
    regular_market_change: Option<NumericField>,
    regular_market_change_percent: Option<NumericField>,
    pre_market_price: Option<NumericField>,
    pre_market_change: Option<NumericField>,
    pre_market_change_percent: Option<NumericField>,
    post_market_price: Option<NumericField>,
    post_market_change: Option<NumericField>,
    post_market_change_percent: Option<NumericField>,
    market_cap: Option<NumericField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummaryDetailModule {
    market_cap: Option<NumericField>,
    trailing_pe: Option<NumericField>,
    forward_pe: Option<NumericField>,
    dividend_yield: Option<NumericField>,
    fifty_two_week_high: Option<NumericField>,
    fifty_two_week_low: Option<NumericField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefaultKeyStatisticsModule {
    trailing_eps: Option<NumericField>,
    forward_pe: Option<NumericField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinancialDataModule {
    current_price: Option<NumericField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteTypeModule {
    quote_type: Option<String>,
    exchange: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummaryProfileModule {
    sector: Option<String>,
    industry: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct YahooApiError {
    code: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum NumericField {
    Raw { raw: Option<f64> },
    Value(f64),
}

impl NumericField {
    fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Raw { raw } => *raw,
            Self::Value(value) => Some(*value),
        }
    }
}

fn build_cookie_header(cookies: &BTreeMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn capture_set_cookies(headers: &HeaderMap, session: &mut YahooSession) -> Result<(), Error> {
    for value in headers.get_all(SET_COOKIE) {
        let raw = value.to_str()?;
        if let Some((name, cookie_value)) = parse_set_cookie(raw) {
            session.cookies.insert(name, cookie_value);
        }
    }

    Ok(())
}

fn parse_set_cookie(header: &str) -> Option<(String, String)> {
    let pair = header.split(';').next()?.trim();
    let (name, value) = pair.split_once('=')?;
    if name.is_empty() {
        return None;
    }

    Some((name.to_string(), value.to_string()))
}

fn header_location(headers: &HeaderMap, base_url: &str) -> Result<Option<String>, Error> {
    let Some(location) = headers.get(LOCATION) else {
        return Ok(None);
    };

    let value = location.to_str()?;
    let absolute = Url::parse(base_url)?.join(value)?;
    Ok(Some(absolute.to_string()))
}

fn html_headers(current_url: Option<&str>, referer: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml"),
    );
    if let Some(referer) = referer.and_then(|value| HeaderValue::from_str(value).ok()) {
        headers.insert(REFERER, referer);
    }
    if let Some(origin) = current_url.and_then(origin_header) {
        headers.insert(ORIGIN, origin);
    }
    headers
}

fn crumb_headers(referer: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    let referer =
        HeaderValue::from_str(referer).unwrap_or_else(|_| HeaderValue::from_static(QUOTE_PAGE_URL));
    headers.insert(REFERER, referer);
    headers.insert(
        ORIGIN,
        HeaderValue::from_static("https://finance.yahoo.com"),
    );
    headers
}

fn json_headers(symbol: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        ORIGIN,
        HeaderValue::from_static("https://finance.yahoo.com"),
    );
    let referer = format!("https://finance.yahoo.com/quote/{symbol}");
    if let Ok(referer) = HeaderValue::from_str(&referer) {
        headers.insert(REFERER, referer);
    }
    headers
}

fn form_headers(current_url: &str, referer: Option<&str>) -> HeaderMap {
    let mut headers = html_headers(Some(current_url), referer);
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers
}

fn origin_header(url: &str) -> Option<HeaderValue> {
    let parsed = Url::parse(url).ok()?;
    let origin = format!("{}://{}", parsed.scheme(), parsed.host_str()?);
    HeaderValue::from_str(&origin).ok()
}

fn build_consent_form_body(body: &str) -> Result<String, Error> {
    let selector = Selector::parse(r#"input[type="hidden"]"#)
        .map_err(|error| anyhow::anyhow!("Failed to build Yahoo consent selector: {error}"))?;
    let document = Html::parse_document(body);
    let mut pairs = Vec::new();

    for input in document.select(&selector) {
        let Some(name) = input.value().attr("name") else {
            continue;
        };
        let Some(value) = input.value().attr("value") else {
            continue;
        };
        pairs.push((name.to_string(), decode_html_entities(value)));
    }
    pairs.push(("agree".to_string(), "agree".to_string()));
    pairs.push(("agree".to_string(), "agree".to_string()));

    if pairs.is_empty() {
        return Err(anyhow::anyhow!(
            "Yahoo consent page did not contain any hidden form inputs"
        ));
    }

    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(&name, &value);
    }
    Ok(serializer.finish())
}

fn decode_html_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '&' && chars.peek() == Some(&'#') {
            let _ = chars.next();
            if chars.peek() == Some(&'x') {
                let _ = chars.next();
                let mut hex = String::new();
                while let Some(next) = chars.peek() {
                    if *next == ';' {
                        let _ = chars.next();
                        break;
                    }
                    hex.push(*next);
                    let _ = chars.next();
                }
                if let Ok(codepoint) = u32::from_str_radix(&hex, 16) {
                    if let Some(decoded) = char::from_u32(codepoint) {
                        output.push(decoded);
                        continue;
                    }
                }
                output.push('&');
                output.push('#');
                output.push('x');
                output.push_str(&hex);
                output.push(';');
                continue;
            }
            output.push('&');
            output.push('#');
            continue;
        }

        output.push(ch);
    }

    output
}

fn looks_like_auth_error(error: &Error) -> bool {
    let message = error.to_string();
    message.contains("401")
        || message.contains("403")
        || message.contains("Invalid Crumb")
        || message.contains("Invalid Cookie")
}

fn merge_quote(chart: ChartMeta, summary: Option<QuoteSummaryResult>) -> StockQuote {
    let regular_market_change = match (chart.regular_market_price, chart.chart_previous_close) {
        (Some(price), Some(previous_close)) => Some(price - previous_close),
        _ => None,
    };
    let regular_market_change_percent = match (regular_market_change, chart.chart_previous_close) {
        (Some(change), Some(previous_close)) if previous_close != 0.0 => {
            Some(change / previous_close)
        }
        _ => None,
    };

    let price = summary.as_ref().and_then(|value| value.price.as_ref());
    let detail = summary
        .as_ref()
        .and_then(|value| value.summary_detail.as_ref());
    let stats = summary
        .as_ref()
        .and_then(|value| value.default_key_statistics.as_ref());
    let quote_type = summary.as_ref().and_then(|value| value.quote_type.as_ref());
    let profile = summary
        .as_ref()
        .and_then(|value| value.summary_profile.as_ref());
    let financial = summary
        .as_ref()
        .and_then(|value| value.financial_data.as_ref());

    StockQuote {
        symbol: chart.symbol,
        short_name: price
            .and_then(|value| value.short_name.clone())
            .or(chart.short_name),
        long_name: price
            .and_then(|value| value.long_name.clone())
            .or(chart.long_name),
        quote_type: price
            .and_then(|value| value.quote_type.clone())
            .or_else(|| quote_type.and_then(|value| value.quote_type.clone())),
        exchange_name: price
            .and_then(|value| value.exchange_name.clone())
            .or_else(|| quote_type.and_then(|value| value.exchange.clone())),
        currency_label: price
            .and_then(|value| value.currency.clone())
            .or(chart.currency)
            .unwrap_or_default(),
        phase: price
            .and_then(|value| value.market_state.as_ref())
            .map(|value| normalize_market_phase(value))
            .unwrap_or_else(|| infer_market_phase(chart.current_trading_period.as_ref())),
        regular_market_price: chart
            .regular_market_price
            .or_else(|| price.and_then(|value| numeric(value.regular_market_price.as_ref())))
            .or_else(|| financial.and_then(|value| numeric(value.current_price.as_ref()))),
        regular_market_change: regular_market_change
            .or_else(|| price.and_then(|value| numeric(value.regular_market_change.as_ref()))),
        regular_market_change_percent: regular_market_change_percent.or_else(|| {
            price.and_then(|value| numeric(value.regular_market_change_percent.as_ref()))
        }),
        pre_market_price: price.and_then(|value| numeric(value.pre_market_price.as_ref())),
        pre_market_change: price.and_then(|value| numeric(value.pre_market_change.as_ref())),
        pre_market_change_percent: price
            .and_then(|value| numeric(value.pre_market_change_percent.as_ref())),
        post_market_price: price.and_then(|value| numeric(value.post_market_price.as_ref())),
        post_market_change: price.and_then(|value| numeric(value.post_market_change.as_ref())),
        post_market_change_percent: price
            .and_then(|value| numeric(value.post_market_change_percent.as_ref())),
        regular_market_day_high: chart.regular_market_day_high,
        regular_market_day_low: chart.regular_market_day_low,
        regular_market_volume: chart.regular_market_volume,
        market_cap: detail
            .and_then(|value| numeric(value.market_cap.as_ref()))
            .or_else(|| price.and_then(|value| numeric(value.market_cap.as_ref()))),
        trailing_pe: detail.and_then(|value| numeric(value.trailing_pe.as_ref())),
        forward_pe: detail
            .and_then(|value| numeric(value.forward_pe.as_ref()))
            .or_else(|| stats.and_then(|value| numeric(value.forward_pe.as_ref()))),
        trailing_eps: stats.and_then(|value| numeric(value.trailing_eps.as_ref())),
        dividend_yield: detail.and_then(|value| numeric(value.dividend_yield.as_ref())),
        fifty_two_week_high: detail.and_then(|value| numeric(value.fifty_two_week_high.as_ref())),
        fifty_two_week_low: detail.and_then(|value| numeric(value.fifty_two_week_low.as_ref())),
        sector: profile.and_then(|value| value.sector.clone()),
        industry: profile.and_then(|value| value.industry.clone()),
    }
}

fn numeric(value: Option<&NumericField>) -> Option<f64> {
    value.and_then(NumericField::as_f64)
}

fn normalize_market_phase(value: &str) -> String {
    match value {
        "PRE" | "PREPRE" => "Pre Market".to_string(),
        "POST" | "POSTPOST" => "Post Market".to_string(),
        "REGULAR" => "Regular Market".to_string(),
        "CLOSED" => "Closed".to_string(),
        other => other.replace('_', " "),
    }
}

fn infer_market_phase(periods: Option<&CurrentTradingPeriod>) -> String {
    let Some(periods) = periods else {
        return "Unknown".to_string();
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    if now >= periods.regular.start && now <= periods.regular.end {
        return "Regular Market".to_string();
    }
    if now >= periods.pre.start && now <= periods.pre.end {
        return "Pre Market".to_string();
    }
    if now >= periods.post.start && now <= periods.post.end {
        return "Post Market".to_string();
    }
    "Closed".to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        PROVIDER_ID, PersistedYahooSession, YahooFinanceClient, build_consent_form_body,
        decode_html_entities,
    };
    use dynamo_core::StockQuoteService;
    use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[test]
    fn decodes_hex_entities() {
        assert_eq!(decode_html_entities("hello&#x20;world"), "hello world");
    }

    #[test]
    fn builds_consent_form_payload() {
        let body = r#"
        <html><body>
          <form>
            <input type="hidden" name="csrfToken" value="abc123">
            <input type="hidden" name="sessionId" value="session-1">
          </form>
        </body></html>
        "#;
        let form = build_consent_form_body(body).expect("form payload");
        assert!(form.contains("csrfToken=abc123"));
        assert!(form.contains("sessionId=session-1"));
        assert!(form.contains("agree=agree"));
    }

    #[test]
    fn persisted_session_round_trips() {
        let mut cookies = BTreeMap::new();
        cookies.insert("A1".to_string(), "cookie-value".to_string());
        let state = PersistedYahooSession {
            crumb: Some("crumb-value".to_string()),
            cookies,
        };

        let json = serde_json::to_value(&state).expect("serialize");
        let restored: PersistedYahooSession = serde_json::from_value(json).expect("deserialize");
        assert_eq!(restored.crumb.as_deref(), Some("crumb-value"));
        assert_eq!(
            restored.cookies.get("A1").map(String::as_str),
            Some("cookie-value")
        );
    }

    #[tokio::test]
    #[ignore = "live network smoke test"]
    async fn live_quote_summary_enrichment_returns_rich_nvda_quote() {
        let client = YahooFinanceClient::new(None).expect("client");
        let quote = client
            .fetch_quote("NVDA")
            .await
            .expect("request should succeed")
            .expect("nvda should resolve");

        assert_eq!(quote.symbol, "NVDA");
        assert!(quote.regular_market_price.is_some());
        assert!(quote.market_cap.is_some(), "market cap should be enriched");
        assert!(
            quote.quote_type.is_some() || quote.exchange_name.is_some(),
            "quoteSummary should contribute quote metadata"
        );
    }

    #[tokio::test]
    #[ignore = "live network and MongoDB smoke test"]
    async fn live_quote_summary_persists_yahoo_session_to_mongodb() {
        let _ = dotenvy::dotenv();
        let config = MongoPersistenceConfig::from_env().expect("MongoDB config from env");
        let store = Arc::new(
            MongoPersistence::connect(config)
                .await
                .expect("connect MongoDB"),
        );
        store.ensure_initialized().await.expect("bootstrap MongoDB");

        let client = YahooFinanceClient::new(Some(store.clone())).expect("client");
        let quote = client
            .fetch_quote("NVDA")
            .await
            .expect("request should succeed")
            .expect("nvda should resolve");

        assert!(quote.market_cap.is_some(), "market cap should be enriched");

        let persisted = store
            .load_provider_state(PROVIDER_ID)
            .await
            .expect("load provider state")
            .expect("provider state should exist");
        let crumb = persisted
            .get("crumb")
            .and_then(|value| value.as_str())
            .expect("crumb should be persisted");
        let cookies = persisted
            .get("cookies")
            .and_then(|value| value.as_object())
            .expect("cookies should be persisted");

        assert!(!crumb.is_empty());
        assert!(!cookies.is_empty());
    }
}
