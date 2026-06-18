use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use dynamo_domain_stock::StockQuote;
use dynamo_service_stock::{Error, StockQuoteService};
use reqwest::Method;
use tokio::sync::{Mutex, oneshot};
use tracing::warn;

use crate::{
    TossInvestClient, TossInvestMarketCalendarService, TossInvestResponse, TossMarketSessionPhase,
    TossRateLimitGroup,
    models::{
        ApiEnvelope, CandlePageResponse, TossCandleRaw, TossErrorEnvelope, TossPriceRaw,
        TossStockRaw,
    },
};

const PRICE_GROUP: TossRateLimitGroup = TossRateLimitGroup::MarketData;
const STOCK_METADATA_GROUP: TossRateLimitGroup = TossRateLimitGroup::Stock;
const CANDLE_GROUP: TossRateLimitGroup = TossRateLimitGroup::MarketDataChart;
const PRICE_BATCH_LIMIT: usize = 200;
const STOCK_METADATA_BATCH_LIMIT: usize = 200;
const PRICE_BATCH_DELAY: Duration = Duration::from_millis(10);
const MARKET_CALENDAR_TTL: TimeDelta = TimeDelta::seconds(30);
const INVALID_TICKER: &str = "Invalid Ticker";

#[derive(Clone)]
pub struct TossInvestStockQuoteService {
    client: TossInvestClient,
    market_calendar: TossInvestMarketCalendarService,
    calendar_cache: MarketCalendarCache,
    price_batcher: PriceBatcher,
    metadata_cache: StockMetadataCache,
    baseline_cache: BaselineCloseCache,
}

impl TossInvestStockQuoteService {
    pub fn new(client: TossInvestClient) -> Self {
        Self {
            market_calendar: TossInvestMarketCalendarService::new(client.clone()),
            client,
            calendar_cache: MarketCalendarCache::default(),
            price_batcher: PriceBatcher::new(PRICE_BATCH_DELAY),
            metadata_cache: StockMetadataCache::default(),
            baseline_cache: BaselineCloseCache::default(),
        }
    }

    pub fn client(&self) -> &TossInvestClient {
        &self.client
    }
}

#[async_trait]
impl StockQuoteService for TossInvestStockQuoteService {
    async fn fetch_quote(&self, symbol: &str) -> Result<Option<StockQuote>, Error> {
        let mut results = self.fetch_quotes(&[symbol.to_string()]).await?;
        let Some(result) = results.pop() else {
            return Ok(None);
        };

        match result {
            Ok(quote) => Ok(Some(quote)),
            Err(error) if error == INVALID_TICKER => Ok(None),
            Err(error) => Err(anyhow!(error)),
        }
    }

    async fn fetch_quotes(
        &self,
        symbols: &[String],
    ) -> Result<Vec<Result<StockQuote, String>>, Error> {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }

        let price_results = self
            .price_batcher
            .fetch(
                symbols
                    .iter()
                    .map(|symbol| normalize_symbol(symbol))
                    .collect(),
                {
                    let client = self.client.clone();
                    move |symbols| {
                        let client = client.clone();
                        async move { fetch_price_batch(&client, &symbols).await }
                    }
                },
            )
            .await?;

        if price_results.iter().all(Result::is_err) {
            return Ok(price_results
                .into_iter()
                .map(|result| match result {
                    Ok(_) => unreachable!("all price results were checked as errors"),
                    Err(error) => Err(error),
                })
                .collect());
        }

        let now = Utc::now();
        let calendar = self
            .calendar_cache
            .fetch_at(now, &self.market_calendar)
            .await?;
        let phase = calendar.classify_at(now);
        let baseline_date = baseline_date_for_phase(&calendar, phase, now);

        let priced_symbols = price_results
            .iter()
            .filter_map(|result| result.as_ref().ok().map(|price| price.symbol.clone()))
            .collect::<Vec<_>>();
        let metadata_by_symbol = match self
            .metadata_cache
            .metadata_for(priced_symbols, self.client.clone())
            .await
        {
            Ok(metadata) => metadata,
            Err(error) => {
                warn!(
                    error = %error,
                    "Toss Invest stock metadata request failed; returning quote without metadata"
                );
                BTreeMap::new()
            }
        };

        let mut quotes = Vec::with_capacity(price_results.len());
        for result in price_results {
            let price = match result {
                Ok(price) => price,
                Err(error) => {
                    quotes.push(Err(error));
                    continue;
                }
            };

            let baseline = match self
                .baseline_cache
                .close_for(&price.symbol, baseline_date, {
                    let client = self.client.clone();
                    move |symbol| {
                        let client = client.clone();
                        async move { fetch_candles(&client, &symbol).await }
                    }
                })
                .await
            {
                Ok(baseline) => baseline,
                Err(error) => {
                    quotes.push(Err(error.to_string()));
                    continue;
                }
            };
            let metadata = metadata_by_symbol
                .get(&price.symbol)
                .cloned()
                .unwrap_or(None);

            quotes.push(Ok(build_stock_quote(price, metadata, phase, baseline)));
        }

        Ok(quotes)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct FetchedStockPrice {
    symbol: String,
    price: f64,
    currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StockMetadata {
    symbol: String,
    short_name: Option<String>,
    long_name: Option<String>,
    quote_type: Option<String>,
    currency: Option<String>,
}

impl From<TossStockRaw> for StockMetadata {
    fn from(value: TossStockRaw) -> Self {
        let name = clean_optional_string(value.name);
        Self {
            symbol: normalize_symbol(&value.symbol),
            short_name: clean_optional_string(value.short_name).or_else(|| name.clone()),
            long_name: clean_optional_string(value.long_name).or(name),
            quote_type: clean_optional_string(value.quote_type),
            currency: clean_optional_string(value.currency)
                .map(|currency| normalize_symbol(&currency)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PriceChange {
    change: Option<f64>,
    change_percent: Option<f64>,
}

#[derive(Clone)]
struct PriceBatcher {
    state: Arc<Mutex<PriceBatchState>>,
    delay: Duration,
}

impl PriceBatcher {
    fn new(delay: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(PriceBatchState::default())),
            delay,
        }
    }

    async fn fetch<F, Fut>(
        &self,
        symbols: Vec<String>,
        fetch_batch: F,
    ) -> Result<Vec<Result<FetchedStockPrice, String>>, Error>
    where
        F: Fn(Vec<String>) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = Result<BTreeMap<String, FetchedStockPrice>, Error>> + Send + 'static,
    {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }

        let mut receivers = Vec::with_capacity(symbols.len());
        let mut should_schedule = false;

        {
            let mut state = self.state.lock().await;
            for symbol in symbols {
                let Some(symbol) = normalize_toss_symbol(&symbol) else {
                    receivers.push(PendingPriceReceiver::Immediate(Err(
                        INVALID_TICKER.to_string()
                    )));
                    continue;
                };

                let (sender, receiver) = oneshot::channel();
                state.pending.entry(symbol).or_default().push(sender);
                receivers.push(PendingPriceReceiver::Receiver(receiver));
            }

            if !state.scheduled && !state.pending.is_empty() {
                state.scheduled = true;
                should_schedule = true;
            }
        }

        if should_schedule {
            self.spawn_flush(fetch_batch);
        }

        let mut results = Vec::with_capacity(receivers.len());
        for receiver in receivers {
            let result = match receiver {
                PendingPriceReceiver::Immediate(result) => result,
                PendingPriceReceiver::Receiver(receiver) => receiver
                    .await
                    .map_err(|_| anyhow!("Toss Invest price batch was cancelled"))?,
            };
            results.push(result);
        }

        Ok(results)
    }

    fn spawn_flush<F, Fut>(&self, fetch_batch: F)
    where
        F: Fn(Vec<String>) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = Result<BTreeMap<String, FetchedStockPrice>, Error>> + Send + 'static,
    {
        let state = self.state.clone();
        let delay = self.delay;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            flush_price_batch(state, fetch_batch).await;
        });
    }
}

#[derive(Default)]
struct PriceBatchState {
    pending: BTreeMap<String, Vec<oneshot::Sender<Result<FetchedStockPrice, String>>>>,
    scheduled: bool,
}

enum PendingPriceReceiver {
    Immediate(Result<FetchedStockPrice, String>),
    Receiver(oneshot::Receiver<Result<FetchedStockPrice, String>>),
}

#[derive(Clone, Default)]
struct MarketCalendarCache {
    entry: Arc<Mutex<Option<CachedMarketCalendar>>>,
}

impl MarketCalendarCache {
    async fn fetch_at(
        &self,
        now: DateTime<Utc>,
        service: &TossInvestMarketCalendarService,
    ) -> Result<crate::models::TossMarketCalendarRaw, Error> {
        {
            let entry = self.entry.lock().await;
            if let Some(cached) = entry.as_ref().filter(|cached| cached.expires_at > now) {
                return Ok(cached.calendar.clone());
            }
        }

        let calendar = service.fetch_today().await?;
        let expires_at = now.checked_add_signed(MARKET_CALENDAR_TTL).unwrap_or(now);
        let mut entry = self.entry.lock().await;
        *entry = Some(CachedMarketCalendar {
            calendar: calendar.clone(),
            expires_at,
        });
        Ok(calendar)
    }
}

#[derive(Clone)]
struct CachedMarketCalendar {
    calendar: crate::models::TossMarketCalendarRaw,
    expires_at: DateTime<Utc>,
}

#[derive(Clone, Default)]
struct StockMetadataCache {
    state: Arc<Mutex<StockMetadataCacheState>>,
}

#[derive(Default)]
struct StockMetadataCacheState {
    entries: BTreeMap<String, StockMetadata>,
    in_flight: BTreeMap<String, Vec<oneshot::Sender<MetadataCacheResult>>>,
}

type MetadataCacheResult = Result<Option<StockMetadata>, String>;

impl StockMetadataCache {
    async fn metadata_for(
        &self,
        symbols: Vec<String>,
        client: TossInvestClient,
    ) -> Result<BTreeMap<String, Option<StockMetadata>>, Error> {
        self.metadata_for_with(symbols, move |symbols| {
            let client = client.clone();
            async move { fetch_stock_metadata_batch(&client, &symbols).await }
        })
        .await
    }

    async fn metadata_for_with<F, Fut>(
        &self,
        symbols: Vec<String>,
        mut fetch_batch: F,
    ) -> Result<BTreeMap<String, Option<StockMetadata>>, Error>
    where
        F: FnMut(Vec<String>) -> Fut,
        Fut: Future<Output = Result<BTreeMap<String, StockMetadata>, Error>>,
    {
        let unique_symbols = symbols
            .into_iter()
            .filter_map(|symbol| normalize_toss_symbol(&symbol))
            .collect::<BTreeSet<_>>();

        let mut output = BTreeMap::new();
        let mut receivers = Vec::new();
        let mut to_fetch = Vec::new();
        {
            let mut state = self.state.lock().await;
            for symbol in unique_symbols {
                if let Some(cached) = state.entries.get(&symbol) {
                    output.insert(symbol, Some(cached.clone()));
                } else {
                    let (sender, receiver) = oneshot::channel();
                    let waiters = state.in_flight.entry(symbol.clone()).or_default();
                    if waiters.is_empty() {
                        to_fetch.push(symbol.clone());
                    }
                    waiters.push(sender);
                    receivers.push((symbol, receiver));
                }
            }
        }

        for chunk in to_fetch.chunks(STOCK_METADATA_BATCH_LIMIT) {
            let chunk_symbols = chunk.to_vec();
            let fetch_result = fetch_batch(chunk_symbols.clone())
                .await
                .map_err(|error| error.to_string());
            self.complete_fetch(chunk_symbols, fetch_result).await;
        }

        for (symbol, receiver) in receivers {
            let metadata = receiver
                .await
                .map_err(|_| anyhow!("Toss Invest stock metadata request was cancelled"))?
                .map_err(|error| anyhow!(error))?;
            output.insert(symbol, metadata);
        }

        Ok(output)
    }

    async fn complete_fetch(
        &self,
        symbols: Vec<String>,
        fetch_result: Result<BTreeMap<String, StockMetadata>, String>,
    ) {
        let mut state = self.state.lock().await;
        for symbol in symbols {
            let result = match &fetch_result {
                Ok(fetched) => {
                    let metadata = fetched.get(&symbol).cloned();
                    if let Some(metadata) = metadata.as_ref() {
                        state.entries.insert(symbol.clone(), metadata.clone());
                    }
                    Ok(metadata)
                }
                Err(error) => Err(error.clone()),
            };

            if let Some(waiters) = state.in_flight.remove(&symbol) {
                for waiter in waiters {
                    let _ = waiter.send(result.clone());
                }
            }
        }
    }
}

#[derive(Clone, Default)]
struct BaselineCloseCache {
    state: Arc<Mutex<BaselineCloseCacheState>>,
}

#[derive(Default)]
struct BaselineCloseCacheState {
    closes: BTreeMap<BaselineCloseCacheKey, f64>,
    in_flight: BTreeMap<BaselineCloseCacheKey, Vec<oneshot::Sender<BaselineCloseCacheResult>>>,
}

type BaselineCloseCacheResult = Result<Option<f64>, String>;

impl BaselineCloseCache {
    async fn close_for<F, Fut>(
        &self,
        symbol: &str,
        date: NaiveDate,
        fetch_candles: F,
    ) -> Result<Option<f64>, Error>
    where
        F: FnOnce(String) -> Fut + Send,
        Fut: Future<Output = Result<Vec<TossCandleRaw>, Error>> + Send,
    {
        let key = BaselineCloseCacheKey {
            symbol: normalize_symbol(symbol),
            date,
        };

        let (receiver, should_fetch) = {
            let mut state = self.state.lock().await;
            if let Some(close) = state.closes.get(&key) {
                return Ok(Some(*close));
            }

            let (sender, receiver) = oneshot::channel();
            let waiters = state.in_flight.entry(key.clone()).or_default();
            let should_fetch = waiters.is_empty();
            waiters.push(sender);
            (receiver, should_fetch)
        };

        if should_fetch {
            let result = match fetch_candles(key.symbol.clone()).await {
                Ok(candles) => close_for_date(&candles, date).map_err(|error| error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            self.complete_fetch(key.clone(), result).await;
        }

        receiver
            .await
            .map_err(|_| anyhow!("Toss Invest candle baseline request was cancelled"))?
            .map_err(|error| anyhow!(error))
    }

    async fn complete_fetch(&self, key: BaselineCloseCacheKey, result: BaselineCloseCacheResult) {
        let mut state = self.state.lock().await;
        if let Ok(Some(close)) = result.as_ref() {
            state.closes.insert(key.clone(), *close);
        }

        if let Some(waiters) = state.in_flight.remove(&key) {
            for waiter in waiters {
                let _ = waiter.send(result.clone());
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BaselineCloseCacheKey {
    symbol: String,
    date: NaiveDate,
}

async fn flush_price_batch<F, Fut>(state: Arc<Mutex<PriceBatchState>>, fetch_batch: F)
where
    F: Fn(Vec<String>) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = Result<BTreeMap<String, FetchedStockPrice>, Error>> + Send + 'static,
{
    let pending = {
        let mut state = state.lock().await;
        state.scheduled = false;
        std::mem::take(&mut state.pending)
    };

    if pending.is_empty() {
        return;
    }

    let symbols = pending.keys().cloned().collect::<Vec<_>>();
    let mut resolved = BTreeMap::new();
    for chunk in symbols.chunks(PRICE_BATCH_LIMIT) {
        let chunk_symbols = chunk.to_vec();
        match fetch_batch(chunk_symbols.clone()).await {
            Ok(prices) => {
                for symbol in chunk_symbols {
                    let result = prices
                        .get(&symbol)
                        .cloned()
                        .ok_or_else(|| INVALID_TICKER.to_string());
                    resolved.insert(symbol, result);
                }
            }
            Err(error) => {
                let error = error.to_string();
                for symbol in chunk_symbols {
                    resolved.insert(symbol, Err(error.clone()));
                }
            }
        }
    }

    for (symbol, senders) in pending {
        let result = resolved
            .remove(&symbol)
            .unwrap_or_else(|| Err(INVALID_TICKER.to_string()));
        for sender in senders {
            let _ = sender.send(result.clone());
        }
    }
}

async fn fetch_price_batch(
    client: &TossInvestClient,
    symbols: &[String],
) -> Result<BTreeMap<String, FetchedStockPrice>, Error> {
    let response = client
        .send_authenticated(PRICE_GROUP, Method::GET, &prices_path(symbols))
        .await?;
    build_price_map(response)
}

async fn fetch_stock_metadata_batch(
    client: &TossInvestClient,
    symbols: &[String],
) -> Result<BTreeMap<String, StockMetadata>, Error> {
    let response = client
        .send_authenticated(STOCK_METADATA_GROUP, Method::GET, &stocks_path(symbols))
        .await?;
    build_stock_metadata_map(response)
}

async fn fetch_candles(
    client: &TossInvestClient,
    symbol: &str,
) -> Result<Vec<TossCandleRaw>, Error> {
    let response = client
        .send_authenticated(CANDLE_GROUP, Method::GET, &candles_path(symbol))
        .await?;
    build_candles(response)
}

fn build_price_map(
    response: TossInvestResponse,
) -> Result<BTreeMap<String, FetchedStockPrice>, Error> {
    if !response.status().is_success() {
        return Err(build_stock_request_error("prices", &response));
    }

    let payload = response
        .json::<ApiEnvelope<Vec<TossPriceRaw>>>()
        .context("failed to deserialize Toss Invest prices response")?;
    payload
        .result
        .into_iter()
        .map(|raw| {
            let symbol = normalize_symbol(&raw.symbol);
            let price = raw
                .last_price
                .parse::<f64>()
                .map_err(|error| anyhow!("Toss Invest price for {symbol} was invalid: {error}"))?;
            Ok((
                symbol.clone(),
                FetchedStockPrice {
                    symbol,
                    price,
                    currency: normalize_symbol(&raw.currency),
                },
            ))
        })
        .collect()
}

fn build_stock_metadata_map(
    response: TossInvestResponse,
) -> Result<BTreeMap<String, StockMetadata>, Error> {
    if !response.status().is_success() {
        return Err(build_stock_request_error("stocks", &response));
    }

    let payload = response
        .json::<ApiEnvelope<Vec<TossStockRaw>>>()
        .context("failed to deserialize Toss Invest stocks response")?;
    Ok(payload
        .result
        .into_iter()
        .map(StockMetadata::from)
        .map(|metadata| (metadata.symbol.clone(), metadata))
        .collect())
}

fn build_candles(response: TossInvestResponse) -> Result<Vec<TossCandleRaw>, Error> {
    if !response.status().is_success() {
        return Err(build_stock_request_error("candles", &response));
    }

    response
        .json::<ApiEnvelope<CandlePageResponse>>()
        .map(|payload| payload.result.candles)
        .context("failed to deserialize Toss Invest candles response")
}

fn build_stock_request_error(endpoint: &str, response: &TossInvestResponse) -> Error {
    if let Ok(error) = response.json::<TossErrorEnvelope>() {
        return anyhow!(
            "Toss Invest {endpoint} request failed with status {} (request_id: {}, code: {}, message: {})",
            response.status(),
            error.error.request_id.as_deref().unwrap_or("unknown"),
            error.error.code,
            error.error.message,
        );
    }

    anyhow!(
        "Toss Invest {endpoint} request failed with status {}",
        response.status()
    )
}

fn build_stock_quote(
    price: FetchedStockPrice,
    metadata: Option<StockMetadata>,
    phase: TossMarketSessionPhase,
    baseline_close: Option<f64>,
) -> StockQuote {
    let change = calculate_change(price.price, baseline_close);
    let mut quote = StockQuote {
        symbol: price.symbol,
        short_name: metadata.as_ref().and_then(|value| value.short_name.clone()),
        long_name: metadata.as_ref().and_then(|value| value.long_name.clone()),
        quote_type: metadata.as_ref().and_then(|value| value.quote_type.clone()),
        currency_label: metadata
            .and_then(|value| value.currency)
            .filter(|currency| !currency.is_empty())
            .unwrap_or(price.currency),
        phase: phase.as_str().to_string(),
        ..StockQuote::default()
    };

    match active_price_field(phase) {
        ActivePriceField::PreMarket => {
            quote.pre_market_price = Some(price.price);
            quote.pre_market_change = change.change;
            quote.pre_market_change_percent = change.change_percent;
        }
        ActivePriceField::RegularMarket => {
            quote.regular_market_price = Some(price.price);
            quote.regular_market_change = change.change;
            quote.regular_market_change_percent = change.change_percent;
        }
        ActivePriceField::PostMarket => {
            quote.post_market_price = Some(price.price);
            quote.post_market_change = change.change;
            quote.post_market_change_percent = change.change_percent;
        }
    }

    quote
}

fn calculate_change(price: f64, baseline_close: Option<f64>) -> PriceChange {
    let change = baseline_close.map(|baseline| price - baseline);
    let change_percent = baseline_close
        .filter(|baseline| *baseline != 0.0)
        .map(|baseline| (price - baseline) / baseline);

    PriceChange {
        change,
        change_percent,
    }
}

fn baseline_date_for_phase(
    calendar: &crate::models::TossMarketCalendarRaw,
    phase: TossMarketSessionPhase,
    at: DateTime<Utc>,
) -> NaiveDate {
    match phase {
        TossMarketSessionPhase::AfterMarket | TossMarketSessionPhase::Closed => {
            most_recent_regular_close_date(calendar, at)
                .unwrap_or(calendar.previous_business_day.date)
        }
        TossMarketSessionPhase::Unknown => most_recent_regular_close_date(calendar, at)
            .unwrap_or(calendar.previous_business_day.date),
        TossMarketSessionPhase::DayMarket
        | TossMarketSessionPhase::PreMarket
        | TossMarketSessionPhase::RegularMarket => calendar.previous_business_day.date,
    }
}

fn most_recent_regular_close_date(
    calendar: &crate::models::TossMarketCalendarRaw,
    at: DateTime<Utc>,
) -> Option<NaiveDate> {
    [
        &calendar.previous_business_day,
        &calendar.today,
        &calendar.next_business_day,
    ]
    .into_iter()
    .filter_map(|day| {
        let regular_end = day.regular_market.as_ref()?.end_time.to_utc();
        (regular_end <= at).then_some((regular_end, day.date))
    })
    .max_by_key(|(regular_end, _)| *regular_end)
    .map(|(_, date)| date)
}

fn active_price_field(phase: TossMarketSessionPhase) -> ActivePriceField {
    match phase {
        TossMarketSessionPhase::DayMarket | TossMarketSessionPhase::PreMarket => {
            ActivePriceField::PreMarket
        }
        TossMarketSessionPhase::AfterMarket => ActivePriceField::PostMarket,
        TossMarketSessionPhase::RegularMarket
        | TossMarketSessionPhase::Closed
        | TossMarketSessionPhase::Unknown => ActivePriceField::RegularMarket,
    }
}

enum ActivePriceField {
    PreMarket,
    RegularMarket,
    PostMarket,
}

fn close_for_date(candles: &[TossCandleRaw], date: NaiveDate) -> Result<Option<f64>, Error> {
    for candle in candles {
        if candle
            .timestamp
            .as_deref()
            .and_then(candle_date)
            .is_some_and(|candle_date| candle_date == date)
        {
            let close = candle.close_price.parse::<f64>().map_err(|error| {
                anyhow!("Toss Invest candle close for {date} was invalid: {error}")
            })?;
            return Ok(Some(close));
        }
    }

    Ok(None)
}

fn candle_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .or_else(|| {
            DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|value| value.date_naive())
        })
}

fn prices_path(symbols: &[String]) -> String {
    format!("/api/v1/prices?symbols={}", symbols.join(","))
}

fn stocks_path(symbols: &[String]) -> String {
    format!("/api/v1/stocks?symbols={}", symbols.join(","))
}

fn candles_path(symbol: &str) -> String {
    format!(
        "/api/v1/candles?symbol={}&interval=1d&count=5&adjusted=true",
        normalize_symbol(symbol)
    )
}

fn normalize_symbol(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn normalize_toss_symbol(value: &str) -> Option<String> {
    let symbol = normalize_symbol(value);
    (!symbol.is_empty()
        && symbol
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-'))
    .then_some(symbol)
}

fn clean_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use chrono::{NaiveDate, TimeZone, Utc};
    use dynamo_service_stock::StockQuoteService;
    use tokio::sync::Mutex;

    use super::*;
    use crate::{
        TossMarketSessionPhase, TossRateLimitGroup,
        models::{TossCandleRaw, TossMarketCalendarRaw, TossMarketDayRaw},
    };

    fn price(symbol: &str, value: f64) -> FetchedStockPrice {
        FetchedStockPrice {
            symbol: symbol.to_string(),
            price: value,
            currency: "USD".to_string(),
        }
    }

    fn metadata(symbol: &str) -> StockMetadata {
        StockMetadata {
            symbol: symbol.to_string(),
            short_name: Some(format!("{symbol} short")),
            long_name: Some(format!("{symbol} long")),
            quote_type: Some("EQUITY".to_string()),
            currency: Some("USD".to_string()),
        }
    }

    fn closed_calendar() -> TossMarketCalendarRaw {
        TossMarketCalendarRaw {
            previous_business_day: TossMarketDayRaw {
                date: NaiveDate::from_ymd_opt(2026, 6, 17).unwrap(),
                day_market: None,
                pre_market: None,
                regular_market: None,
                after_market: None,
            },
            today: TossMarketDayRaw {
                date: NaiveDate::from_ymd_opt(2026, 6, 18).unwrap(),
                day_market: None,
                pre_market: None,
                regular_market: None,
                after_market: None,
            },
            next_business_day: TossMarketDayRaw {
                date: NaiveDate::from_ymd_opt(2026, 6, 19).unwrap(),
                day_market: None,
                pre_market: None,
                regular_market: None,
                after_market: None,
            },
        }
    }

    #[tokio::test]
    async fn price_batcher_coalesces_concurrent_requests_and_preserves_order() {
        let batcher = PriceBatcher::new(std::time::Duration::from_millis(10));
        let seen_batches = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let call_count = Arc::new(AtomicUsize::new(0));

        let first = batcher.fetch(
            vec!["NVDA".to_string(), "SOXL".to_string(), "NVDA".to_string()],
            {
                let seen_batches = seen_batches.clone();
                let call_count = call_count.clone();
                move |symbols| {
                    let seen_batches = seen_batches.clone();
                    let call_count = call_count.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        seen_batches.lock().await.push(symbols.clone());
                        Ok(BTreeMap::from([
                            ("NVDA".to_string(), price("NVDA", 125.0)),
                            ("SOXL".to_string(), price("SOXL", 23.0)),
                            ("AAPL".to_string(), price("AAPL", 212.0)),
                        ]))
                    }
                }
            },
        );

        let second = batcher.fetch(vec!["AAPL".to_string(), "NVDA".to_string()], {
            let seen_batches = seen_batches.clone();
            let call_count = call_count.clone();
            move |symbols| {
                let seen_batches = seen_batches.clone();
                let call_count = call_count.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    seen_batches.lock().await.push(symbols.clone());
                    Ok(BTreeMap::from([
                        ("NVDA".to_string(), price("NVDA", 125.0)),
                        ("SOXL".to_string(), price("SOXL", 23.0)),
                        ("AAPL".to_string(), price("AAPL", 212.0)),
                    ]))
                }
            }
        });

        let (first, second) = tokio::join!(first, second);
        let first = first.unwrap();
        let second = second.unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            seen_batches.lock().await.as_slice(),
            [vec![
                "AAPL".to_string(),
                "NVDA".to_string(),
                "SOXL".to_string()
            ]]
        );
        assert_eq!(
            first
                .iter()
                .map(|result| result.as_ref().unwrap().symbol.as_str())
                .collect::<Vec<_>>(),
            ["NVDA", "SOXL", "NVDA"]
        );
        assert_eq!(
            second
                .iter()
                .map(|result| result.as_ref().unwrap().symbol.as_str())
                .collect::<Vec<_>>(),
            ["AAPL", "NVDA"]
        );
    }

    #[tokio::test]
    async fn price_batcher_chunks_price_requests_at_two_hundred_symbols() {
        let batcher = PriceBatcher::new(std::time::Duration::from_millis(1));
        let symbols = (0..201)
            .map(|index| format!("SYM{index:03}"))
            .collect::<Vec<_>>();
        let seen_batches = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));

        let results = batcher
            .fetch(symbols.clone(), {
                let seen_batches = seen_batches.clone();
                move |symbols| {
                    let seen_batches = seen_batches.clone();
                    async move {
                        seen_batches.lock().await.push(symbols.clone());
                        Ok(symbols
                            .into_iter()
                            .map(|symbol| (symbol.clone(), price(&symbol, 1.0)))
                            .collect())
                    }
                }
            })
            .await
            .unwrap();

        let seen_batches = seen_batches.lock().await;
        assert_eq!(seen_batches.len(), 2);
        assert_eq!(seen_batches[0].len(), 200);
        assert_eq!(seen_batches[1].len(), 1);
        assert_eq!(
            results
                .iter()
                .map(|result| result.as_ref().unwrap().symbol.as_str())
                .collect::<Vec<_>>(),
            symbols
        );
    }

    #[tokio::test]
    async fn stock_metadata_cache_retries_symbols_missing_from_prior_response() {
        let cache = StockMetadataCache::default();
        let call_count = Arc::new(AtomicUsize::new(0));

        let first = cache
            .metadata_for_with(vec!["NVDA".to_string()], {
                let call_count = call_count.clone();
                move |_symbols| {
                    let call_count = call_count.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        Ok(BTreeMap::new())
                    }
                }
            })
            .await
            .unwrap();

        let second = cache
            .metadata_for_with(vec!["NVDA".to_string()], {
                let call_count = call_count.clone();
                move |symbols| {
                    let call_count = call_count.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        Ok(symbols
                            .into_iter()
                            .map(|symbol| (symbol.clone(), metadata(&symbol)))
                            .collect())
                    }
                }
            })
            .await
            .unwrap();

        assert_eq!(first.get("NVDA"), Some(&None));
        assert_eq!(second.get("NVDA"), Some(&Some(metadata("NVDA"))));
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stock_metadata_cache_coalesces_concurrent_misses() {
        let cache = StockMetadataCache::default();
        let call_count = Arc::new(AtomicUsize::new(0));

        let first = cache.metadata_for_with(vec!["NVDA".to_string()], {
            let call_count = call_count.clone();
            move |symbols| {
                let call_count = call_count.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    Ok(symbols
                        .into_iter()
                        .map(|symbol| (symbol.clone(), metadata(&symbol)))
                        .collect())
                }
            }
        });
        let second = cache.metadata_for_with(vec!["NVDA".to_string()], {
            let call_count = call_count.clone();
            move |symbols| {
                let call_count = call_count.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    Ok(symbols
                        .into_iter()
                        .map(|symbol| (symbol.clone(), metadata(&symbol)))
                        .collect())
                }
            }
        });

        let (first, second) = tokio::join!(first, second);

        assert_eq!(first.unwrap().get("NVDA"), Some(&Some(metadata("NVDA"))));
        assert_eq!(second.unwrap().get("NVDA"), Some(&Some(metadata("NVDA"))));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn candle_baseline_caches_by_symbol_and_date() {
        let cache = BaselineCloseCache::default();
        let call_count = Arc::new(AtomicUsize::new(0));
        let baseline_date = NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();

        let first = cache
            .close_for("NVDA", baseline_date, {
                let call_count = call_count.clone();
                move |_symbol| {
                    let call_count = call_count.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![TossCandleRaw {
                            symbol: Some("NVDA".to_string()),
                            close_price: "100.00".to_string(),
                            timestamp: Some("2026-06-17T16:00:00-04:00".to_string()),
                        }])
                    }
                }
            })
            .await
            .unwrap();
        let second = cache
            .close_for("NVDA", baseline_date, {
                let call_count = call_count.clone();
                move |_symbol| {
                    let call_count = call_count.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        Ok(Vec::new())
                    }
                }
            })
            .await
            .unwrap();
        let third = cache
            .close_for("NVDA", NaiveDate::from_ymd_opt(2026, 6, 16).unwrap(), {
                let call_count = call_count.clone();
                move |_symbol| {
                    let call_count = call_count.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![TossCandleRaw {
                            symbol: Some("NVDA".to_string()),
                            close_price: "98.00".to_string(),
                            timestamp: Some("2026-06-16".to_string()),
                        }])
                    }
                }
            })
            .await
            .unwrap();

        assert_eq!(first, Some(100.0));
        assert_eq!(second, Some(100.0));
        assert_eq!(third, Some(98.0));
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn candle_baseline_cache_retries_missing_close_dates() {
        let cache = BaselineCloseCache::default();
        let call_count = Arc::new(AtomicUsize::new(0));
        let baseline_date = NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();

        let first = cache
            .close_for("NVDA", baseline_date, {
                let call_count = call_count.clone();
                move |_symbol| {
                    let call_count = call_count.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        Ok(Vec::new())
                    }
                }
            })
            .await
            .unwrap();
        let second = cache
            .close_for("NVDA", baseline_date, {
                let call_count = call_count.clone();
                move |_symbol| {
                    let call_count = call_count.clone();
                    async move {
                        call_count.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![TossCandleRaw {
                            symbol: Some("NVDA".to_string()),
                            close_price: "100.00".to_string(),
                            timestamp: Some("2026-06-17".to_string()),
                        }])
                    }
                }
            })
            .await
            .unwrap();

        assert_eq!(first, None);
        assert_eq!(second, Some(100.0));
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn candle_baseline_cache_coalesces_concurrent_misses() {
        let cache = BaselineCloseCache::default();
        let call_count = Arc::new(AtomicUsize::new(0));
        let baseline_date = NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();

        let first = cache.close_for("NVDA", baseline_date, {
            let call_count = call_count.clone();
            move |_symbol| {
                let call_count = call_count.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    Ok(vec![TossCandleRaw {
                        symbol: Some("NVDA".to_string()),
                        close_price: "100.00".to_string(),
                        timestamp: Some("2026-06-17".to_string()),
                    }])
                }
            }
        });
        let second = cache.close_for("NVDA", baseline_date, {
            let call_count = call_count.clone();
            move |_symbol| {
                let call_count = call_count.clone();
                async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![TossCandleRaw {
                        symbol: Some("NVDA".to_string()),
                        close_price: "101.00".to_string(),
                        timestamp: Some("2026-06-17".to_string()),
                    }])
                }
            }
        });

        let (first, second) = tokio::join!(first, second);

        assert_eq!(first.unwrap(), Some(100.0));
        assert_eq!(second.unwrap(), Some(100.0));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn change_percent_calculation_uses_fractional_percent() {
        let change = calculate_change(105.0, Some(100.0));
        assert_eq!(change.change, Some(5.0));
        assert_eq!(change.change_percent, Some(0.05));

        let zero_baseline = calculate_change(105.0, Some(0.0));
        assert_eq!(zero_baseline.change, Some(105.0));
        assert_eq!(zero_baseline.change_percent, None);
    }

    #[test]
    fn stock_quote_fields_follow_pre_regular_after_and_closed_phase() {
        let phases = [
            (
                TossMarketSessionPhase::PreMarket,
                Some(101.0),
                None,
                None,
                Some(1.0),
                None,
                None,
            ),
            (
                TossMarketSessionPhase::RegularMarket,
                None,
                Some(102.0),
                None,
                None,
                Some(2.0),
                None,
            ),
            (
                TossMarketSessionPhase::AfterMarket,
                None,
                None,
                Some(103.0),
                None,
                None,
                Some(3.0),
            ),
            (
                TossMarketSessionPhase::Closed,
                None,
                Some(104.0),
                None,
                None,
                Some(4.0),
                None,
            ),
        ];

        for (
            phase,
            expected_pre_price,
            expected_regular_price,
            expected_post_price,
            expected_pre_change,
            expected_regular_change,
            expected_post_change,
        ) in phases
        {
            let quote = build_stock_quote(
                price(
                    "NVDA",
                    expected_pre_price
                        .or(expected_regular_price)
                        .or(expected_post_price)
                        .unwrap(),
                ),
                Some(metadata("NVDA")),
                phase,
                Some(100.0),
            );

            assert_eq!(quote.symbol, "NVDA");
            assert_eq!(quote.short_name.as_deref(), Some("NVDA short"));
            assert_eq!(quote.long_name.as_deref(), Some("NVDA long"));
            assert_eq!(quote.quote_type.as_deref(), Some("EQUITY"));
            assert_eq!(quote.currency_label, "USD");
            assert_eq!(quote.phase, phase.as_str());
            assert_eq!(quote.pre_market_price, expected_pre_price);
            assert_eq!(quote.regular_market_price, expected_regular_price);
            assert_eq!(quote.post_market_price, expected_post_price);
            assert_eq!(quote.pre_market_change, expected_pre_change);
            assert_eq!(quote.regular_market_change, expected_regular_change);
            assert_eq!(quote.post_market_change, expected_post_change);
            assert!(quote.trailing_pe.is_none());
            assert!(quote.trailing_eps.is_none());
            assert!(quote.dividend_yield.is_none());
            assert!(quote.sector.is_none());
            assert!(quote.fifty_two_week_high.is_none());
        }
    }

    #[test]
    fn stock_endpoint_paths_and_rate_limit_groups_are_stable() {
        assert_eq!(PRICE_GROUP, TossRateLimitGroup::MarketData);
        assert_eq!(STOCK_METADATA_GROUP, TossRateLimitGroup::Stock);
        assert_eq!(CANDLE_GROUP, TossRateLimitGroup::MarketDataChart);
        assert_eq!(PRICE_BATCH_LIMIT, 200);
        assert_eq!(STOCK_METADATA_BATCH_LIMIT, 200);

        assert_eq!(
            prices_path(&["SOXL".to_string(), "NVDA".to_string()]),
            "/api/v1/prices?symbols=SOXL,NVDA"
        );
        assert_eq!(
            stocks_path(&["SOXL".to_string(), "NVDA".to_string()]),
            "/api/v1/stocks?symbols=SOXL,NVDA"
        );
        assert_eq!(
            candles_path("SOXL"),
            "/api/v1/candles?symbol=SOXL&interval=1d&count=5&adjusted=true"
        );
    }

    #[tokio::test]
    async fn public_service_implements_stock_quote_service() {
        let service = TossInvestStockQuoteService::new(crate::TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "test-client-id".to_string(),
                ),
                (
                    "TOSSINVEST_CLIENT_SECRET".to_string(),
                    "test-client-secret".to_string(),
                ),
                (
                    "TOSSINVEST_BASE_URL".to_string(),
                    "https://openapi.tossinvest.com".to_string(),
                ),
            ]))
            .unwrap(),
        ));

        fn assert_stock_service<T: StockQuoteService>(_service: &T) {}
        assert_stock_service(&service);
    }

    #[test]
    fn baseline_date_uses_recent_regular_close_for_after_and_closed() {
        let mut calendar = closed_calendar();
        calendar.today.regular_market = Some(crate::models::TossMarketSessionRaw {
            start_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T22:30:00+09:00").unwrap(),
            end_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T05:00:00+09:00").unwrap(),
        });

        assert_eq!(
            baseline_date_for_phase(
                &calendar,
                TossMarketSessionPhase::RegularMarket,
                Utc.with_ymd_and_hms(2026, 6, 18, 14, 0, 0).unwrap(),
            ),
            NaiveDate::from_ymd_opt(2026, 6, 17).unwrap()
        );
        assert_eq!(
            baseline_date_for_phase(
                &calendar,
                TossMarketSessionPhase::AfterMarket,
                Utc.with_ymd_and_hms(2026, 6, 18, 21, 0, 0).unwrap(),
            ),
            NaiveDate::from_ymd_opt(2026, 6, 18).unwrap()
        );
        assert_eq!(
            baseline_date_for_phase(
                &calendar,
                TossMarketSessionPhase::Closed,
                Utc.with_ymd_and_hms(2026, 6, 19, 1, 0, 0).unwrap(),
            ),
            NaiveDate::from_ymd_opt(2026, 6, 18).unwrap()
        );
    }
}
