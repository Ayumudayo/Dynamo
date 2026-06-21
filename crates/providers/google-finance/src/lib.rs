use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use dynamo_domain_currency::{
    ExchangeRateCacheStatus, ExchangeRateQuote, ExchangeRateRefreshResult, ExchangeRateSourceKind,
    cached_exchange_currencies,
};
use dynamo_repositories::ProviderStateRepository;
use dynamo_service_exchange::{Error, ExchangeRateService};
use futures_util::stream::{self, StreamExt};
use reqwest::{Client, header::ACCEPT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

const PROVIDER_ID: &str = "google_finance_exchange";
const GOOGLE_FINANCE_BASE: &str = "https://www.google.com/finance/quote";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) dynamo-rs/0.1 Safari/537.36";
const CACHE_REFRESH_INTERVAL_SECONDS: u64 = 30 * 60;
#[derive(Clone)]
pub struct GoogleFinanceExchangeService {
    client: Client,
    session_repo: Option<Arc<dyn ProviderStateRepository>>,
    cache: Arc<RwLock<ExchangeRateCache>>,
    loaded_from_repo: Arc<AtomicBool>,
    load_guard: Arc<Mutex<()>>,
}

impl GoogleFinanceExchangeService {
    pub fn new(session_repo: Option<Arc<dyn ProviderStateRepository>>) -> Result<Self, Error> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        Ok(Self {
            client,
            session_repo,
            cache: Arc::new(RwLock::new(ExchangeRateCache::default())),
            loaded_from_repo: Arc::new(AtomicBool::new(false)),
            load_guard: Arc::new(Mutex::new(())),
        })
    }

    async fn ensure_loaded_from_repo(&self) -> Result<(), Error> {
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

        if let Some(value) = repo.load_json(PROVIDER_ID).await?
            && let Ok(state) = serde_json::from_value::<PersistedExchangeCache>(value)
        {
            let mut cache = self.cache.write().await;
            cache.entries = state.entries;
            cache.last_refresh_at = state.last_refresh_at;
        }

        self.loaded_from_repo.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn persist_cache(&self) -> Result<(), Error> {
        let Some(repo) = &self.session_repo else {
            return Ok(());
        };

        let cache = self.cache.read().await;
        let value = serde_json::to_value(PersistedExchangeCache {
            entries: cache.entries.clone(),
            last_refresh_at: cache.last_refresh_at,
        })?;
        drop(cache);

        repo.save_json(PROVIDER_ID, value).await
    }

    fn pair_url(from: &str, to: &str) -> String {
        format!("{GOOGLE_FINANCE_BASE}/{from}-{to}?hl=en")
    }

    async fn fetch_live_pair_internal(
        &self,
        from: &str,
        to: &str,
    ) -> Result<ExchangeRateQuote, Error> {
        let url = Self::pair_url(from, to);
        let html = self
            .client
            .get(&url)
            .header(ACCEPT, "text/html,application/xhtml+xml")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        parse_google_finance_pair_html(&html, from, to)
    }

    async fn cached_quote(&self, from: &str, to: &str) -> Result<ExchangeRateQuote, Error> {
        self.ensure_loaded_from_repo().await?;
        let cache = self.cache.read().await;
        build_cached_cross_quote(&cache, from, to)
    }

    async fn refresh_cache_with<F, Fut>(
        &self,
        fetch_pair: F,
    ) -> Result<ExchangeRateRefreshResult, Error>
    where
        F: Fn(String) -> Fut + Clone + Send,
        Fut: Future<Output = Result<ExchangeRateQuote, Error>> + Send,
    {
        self.ensure_loaded_from_repo().await?;

        let mut next_entries = {
            let cache = self.cache.read().await;
            cache.entries.clone()
        };

        let refresh_started_at = Utc::now();
        let mut refreshed = 0usize;
        let mut failed = 0usize;

        let usd_now = CachedUsdRate {
            usd_rate: 1.0,
            source_timestamp: refresh_started_at,
            source_timestamp_text: format_timestamp(refresh_started_at),
            updated_at: refresh_started_at,
        };
        next_entries.insert("USD".to_string(), usd_now);

        let currencies = cached_exchange_currencies()
            .iter()
            .copied()
            .filter(|value| *value != "USD")
            .map(str::to_string)
            .collect::<Vec<_>>();
        let results = stream::iter(currencies.into_iter())
            .map(|currency| {
                let fetch_pair = fetch_pair.clone();
                async move { (currency.clone(), fetch_pair(currency).await) }
            })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;

        for (currency, result) in results {
            match result {
                Ok(quote) => {
                    next_entries.insert(
                        currency,
                        CachedUsdRate {
                            usd_rate: quote.rate,
                            source_timestamp: quote.source_timestamp,
                            source_timestamp_text: quote.source_timestamp_text,
                            updated_at: refresh_started_at,
                        },
                    );
                    refreshed += 1;
                }
                Err(_) => {
                    failed += 1;
                }
            }
        }

        {
            let mut cache = self.cache.write().await;
            cache.entries = next_entries;
            if refreshed > 0 {
                cache.last_refresh_at = Some(refresh_started_at);
            }
        }

        if refreshed > 0 {
            self.persist_cache().await?;
        }

        Ok(ExchangeRateRefreshResult {
            target_currency_count: cached_exchange_currencies().len(),
            refreshed_currency_count: refreshed,
            failed_currency_count: failed,
            last_refresh_at: if refreshed > 0 {
                Some(refresh_started_at)
            } else {
                self.cache.read().await.last_refresh_at
            },
        })
    }
}

#[async_trait]
impl ExchangeRateService for GoogleFinanceExchangeService {
    async fn fetch_pair(&self, from: &str, to: &str) -> Result<ExchangeRateQuote, Error> {
        self.ensure_loaded_from_repo().await?;
        match self.fetch_live_pair_internal(from, to).await {
            Ok(mut quote) => {
                quote.source_kind = ExchangeRateSourceKind::Live;
                Ok(quote)
            }
            Err(_) => {
                let mut quote = self.cached_quote(from, to).await?;
                quote.source_kind = ExchangeRateSourceKind::Cache;
                Ok(quote)
            }
        }
    }

    async fn refresh_cache(&self) -> Result<ExchangeRateRefreshResult, Error> {
        let service = self.clone();
        self.refresh_cache_with(move |currency| {
            let service = service.clone();
            async move { service.fetch_live_pair_internal("USD", &currency).await }
        })
        .await
    }

    async fn cache_status(&self) -> Result<ExchangeRateCacheStatus, Error> {
        self.ensure_loaded_from_repo().await?;
        let cache = self.cache.read().await;
        Ok(ExchangeRateCacheStatus {
            target_currency_count: cached_exchange_currencies().len(),
            cached_currency_count: cache.entries.len(),
            uses_persisted_cache: self.session_repo.is_some(),
            last_refresh_at: cache.last_refresh_at,
        })
    }

    fn cache_target_count(&self) -> usize {
        cached_exchange_currencies().len()
    }

    fn uses_persisted_cache(&self) -> bool {
        self.session_repo.is_some()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ExchangeRateCache {
    #[serde(default)]
    entries: BTreeMap<String, CachedUsdRate>,
    last_refresh_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedUsdRate {
    usd_rate: f64,
    source_timestamp: DateTime<Utc>,
    source_timestamp_text: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedExchangeCache {
    #[serde(default)]
    entries: BTreeMap<String, CachedUsdRate>,
    last_refresh_at: Option<DateTime<Utc>>,
}

fn parse_google_finance_pair_html(
    html: &str,
    from: &str,
    to: &str,
) -> Result<ExchangeRateQuote, Error> {
    let selector = Selector::parse("div[data-last-price][data-source][data-target]")
        .map_err(|error| anyhow::anyhow!("Failed to build Google Finance selector: {error}"))?;
    let document = Html::parse_document(html);
    let normalized_from = from.trim().to_ascii_uppercase();
    let normalized_to = to.trim().to_ascii_uppercase();

    let node = document
        .select(&selector)
        .find(|element| {
            element
                .value()
                .attr("data-source")
                .map(|value| value.eq_ignore_ascii_case(&normalized_from))
                .unwrap_or(false)
                && element
                    .value()
                    .attr("data-target")
                    .map(|value| value.eq_ignore_ascii_case(&normalized_to))
                    .unwrap_or(false)
        })
        .ok_or_else(|| anyhow::anyhow!("Google Finance did not expose a live quote node"))?;

    let rate = node
        .value()
        .attr("data-last-price")
        .ok_or_else(|| {
            anyhow::anyhow!("Google Finance quote node did not include data-last-price")
        })?
        .parse::<f64>()
        .map_err(|error| anyhow::anyhow!("Google Finance data-last-price was invalid: {error}"))?;
    let timestamp_seconds = node
        .value()
        .attr("data-last-normal-market-timestamp")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Google Finance quote node did not include data-last-normal-market-timestamp"
            )
        })?
        .parse::<i64>()
        .map_err(|error| anyhow::anyhow!("Google Finance timestamp was invalid: {error}"))?;
    let source_timestamp = Utc
        .timestamp_opt(timestamp_seconds, 0)
        .single()
        .ok_or_else(|| anyhow::anyhow!("Google Finance timestamp was out of range"))?;

    Ok(ExchangeRateQuote {
        from: normalized_from,
        to: normalized_to,
        rate,
        source_kind: ExchangeRateSourceKind::Live,
        source_timestamp,
        source_timestamp_text: format_timestamp(source_timestamp),
        fetched_at_utc: Utc::now(),
    })
}

fn build_cached_cross_quote(
    cache: &ExchangeRateCache,
    from: &str,
    to: &str,
) -> Result<ExchangeRateQuote, Error> {
    let from = from.trim().to_ascii_uppercase();
    let to = to.trim().to_ascii_uppercase();
    let (from_rate, from_timestamp) = cached_usd_rate(cache, &from)?;
    let (to_rate, to_timestamp) = cached_usd_rate(cache, &to)?;
    if from_rate == 0.0 {
        return Err(anyhow::anyhow!("Cached USD base rate for {from} is zero"));
    }

    let source_timestamp = from_timestamp.min(to_timestamp);
    Ok(ExchangeRateQuote {
        from,
        to,
        rate: to_rate / from_rate,
        source_kind: ExchangeRateSourceKind::Cache,
        source_timestamp,
        source_timestamp_text: format_timestamp(source_timestamp),
        fetched_at_utc: Utc::now(),
    })
}

fn cached_usd_rate(
    cache: &ExchangeRateCache,
    currency: &str,
) -> Result<(f64, DateTime<Utc>), Error> {
    if currency == "USD" {
        let source_timestamp = cache.last_refresh_at.unwrap_or_else(Utc::now);
        return Ok((1.0, source_timestamp));
    }

    let entry = cache
        .entries
        .get(currency)
        .ok_or_else(|| anyhow::anyhow!("No cached exchange rate available for {currency}"))?;
    Ok((entry.usd_rate, entry.source_timestamp))
}

fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

pub fn cache_refresh_interval_seconds() -> u64 {
    CACHE_REFRESH_INTERVAL_SECONDS
}

pub fn supported_currencies() -> &'static [&'static str] {
    cached_exchange_currencies()
}

#[cfg(test)]
impl GoogleFinanceExchangeService {
    async fn test_refresh_cache_with<F, Fut>(
        &self,
        fetch_pair: F,
    ) -> Result<ExchangeRateRefreshResult, Error>
    where
        F: Fn(String) -> Fut + Clone + Send,
        Fut: Future<Output = Result<ExchangeRateQuote, Error>> + Send,
    {
        self.refresh_cache_with(fetch_pair).await
    }

    async fn test_cache_snapshot(
        &self,
    ) -> (BTreeMap<String, CachedUsdRate>, Option<DateTime<Utc>>) {
        let cache = self.cache.read().await;
        (cache.entries.clone(), cache.last_refresh_at)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CachedUsdRate, ExchangeRateCache, GoogleFinanceExchangeService, PersistedExchangeCache,
        build_cached_cross_quote, parse_google_finance_pair_html,
    };
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use dynamo_domain_currency::{ExchangeRateQuote, ExchangeRateSourceKind};
    use dynamo_repositories::ProviderStateRepository;
    use std::collections::BTreeMap;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::{Mutex, Semaphore};
    use tokio::time::{Duration, sleep};

    type RefreshJoinHandle =
        tokio::task::JoinHandle<anyhow::Result<super::ExchangeRateRefreshResult>>;

    #[test]
    fn parses_google_finance_quote_fixture() {
        let html = r#"
        <div data-source="USD" data-target="KRW" data-last-price="1488.52939" data-last-normal-market-timestamp="1773664280"></div>
        "#;

        let quote = parse_google_finance_pair_html(html, "USD", "KRW").expect("quote");
        assert_eq!(quote.from, "USD");
        assert_eq!(quote.to, "KRW");
        assert_eq!(quote.rate, 1488.52939);
        assert_eq!(quote.source_timestamp.timestamp(), 1773664280);
    }

    #[test]
    fn computes_cross_rate_from_usd_cache() {
        let ts = Utc.with_ymd_and_hms(2026, 3, 16, 8, 0, 0).single().unwrap();
        let mut entries = BTreeMap::new();
        entries.insert(
            "KRW".to_string(),
            CachedUsdRate {
                usd_rate: 1488.5,
                source_timestamp: ts,
                source_timestamp_text: "2026-03-16 08:00:00 UTC".to_string(),
                updated_at: ts,
            },
        );
        entries.insert(
            "JPY".to_string(),
            CachedUsdRate {
                usd_rate: 149.5,
                source_timestamp: ts,
                source_timestamp_text: "2026-03-16 08:00:00 UTC".to_string(),
                updated_at: ts,
            },
        );

        let quote = build_cached_cross_quote(
            &ExchangeRateCache {
                entries,
                last_refresh_at: Some(ts),
            },
            "JPY",
            "KRW",
        )
        .expect("cross quote");

        assert!((quote.rate - (1488.5 / 149.5)).abs() < 0.000001);
        assert_eq!(quote.source_timestamp, ts);
    }

    #[test]
    fn persisted_cache_round_trips() {
        let ts = Utc.with_ymd_and_hms(2026, 3, 16, 8, 0, 0).single().unwrap();
        let mut entries = BTreeMap::new();
        entries.insert(
            "EUR".to_string(),
            CachedUsdRate {
                usd_rate: 0.92,
                source_timestamp: ts,
                source_timestamp_text: "2026-03-16 08:00:00 UTC".to_string(),
                updated_at: ts,
            },
        );
        let state = PersistedExchangeCache {
            entries,
            last_refresh_at: Some(ts),
        };

        let json = serde_json::to_value(&state).expect("serialize");
        let restored: PersistedExchangeCache = serde_json::from_value(json).expect("deserialize");
        assert_eq!(restored.entries.len(), 1);
        assert_eq!(restored.last_refresh_at, Some(ts));
    }

    struct CountingRepo {
        load_calls: AtomicUsize,
    }

    #[async_trait]
    impl ProviderStateRepository for CountingRepo {
        async fn load_json(
            &self,
            _provider_id: &str,
        ) -> Result<Option<serde_json::Value>, dynamo_repositories::Error> {
            self.load_calls.fetch_add(1, Ordering::SeqCst);
            sleep(Duration::from_millis(25)).await;
            Ok(None)
        }

        async fn save_json(
            &self,
            _provider_id: &str,
            _value: serde_json::Value,
        ) -> Result<(), dynamo_repositories::Error> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn loads_persisted_cache_only_once_under_concurrency() {
        let repo = Arc::new(CountingRepo {
            load_calls: AtomicUsize::new(0),
        });
        let service =
            GoogleFinanceExchangeService::new(Some(repo.clone())).expect("service should build");

        let mut handles = Vec::new();
        for _ in 0..6 {
            let service = service.clone();
            handles.push(tokio::spawn(async move {
                service.ensure_loaded_from_repo().await
            }));
        }

        for handle in handles {
            handle
                .await
                .expect("task should complete")
                .expect("initial load should succeed");
        }

        assert_eq!(repo.load_calls.load(Ordering::SeqCst), 1);
    }

    struct RecordingRepo {
        initial: serde_json::Value,
        save_calls: AtomicUsize,
        saved: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait]
    impl ProviderStateRepository for RecordingRepo {
        async fn load_json(
            &self,
            _provider_id: &str,
        ) -> Result<Option<serde_json::Value>, dynamo_repositories::Error> {
            Ok(Some(self.initial.clone()))
        }

        async fn save_json(
            &self,
            _provider_id: &str,
            value: serde_json::Value,
        ) -> Result<(), dynamo_repositories::Error> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            self.saved.lock().await.push(value);
            Ok(())
        }
    }

    #[tokio::test]
    async fn refresh_cache_collects_results_then_updates_and_persists_once() {
        let mut supported = super::supported_currencies()
            .iter()
            .copied()
            .filter(|currency| *currency != "USD");
        let success_currency = supported.next().expect("non-USD currency");
        let failed_currency = supported.next().expect("second non-USD currency");
        let target_currency_count = super::supported_currencies().len();
        let non_usd_count = target_currency_count - 1;
        let expected_concurrency = non_usd_count.min(4);

        let old_ts = Utc.with_ymd_and_hms(2026, 3, 16, 8, 0, 0).single().unwrap();
        let new_ts = Utc.with_ymd_and_hms(2026, 3, 16, 9, 0, 0).single().unwrap();
        let mut initial_entries = BTreeMap::new();
        initial_entries.insert(
            failed_currency.to_string(),
            CachedUsdRate {
                usd_rate: 9.9,
                source_timestamp: old_ts,
                source_timestamp_text: "2026-03-16 08:00:00 UTC".to_string(),
                updated_at: old_ts,
            },
        );
        let initial = serde_json::to_value(PersistedExchangeCache {
            entries: initial_entries.clone(),
            last_refresh_at: Some(old_ts),
        })
        .expect("serialize initial cache");
        let repo = Arc::new(RecordingRepo {
            initial,
            save_calls: AtomicUsize::new(0),
            saved: Mutex::new(Vec::new()),
        });
        let service =
            GoogleFinanceExchangeService::new(Some(repo.clone())).expect("service should build");

        let started = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Semaphore::new(0));

        let refresh: RefreshJoinHandle = {
            let service = service.clone();
            let started = started.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            let gate = gate.clone();
            tokio::spawn(async move {
                service
                    .test_refresh_cache_with(move |currency| {
                        let started = started.clone();
                        let active = active.clone();
                        let max_active = max_active.clone();
                        let gate = gate.clone();
                        async move {
                            started.fetch_add(1, Ordering::SeqCst);
                            let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                            max_active.fetch_max(now_active, Ordering::SeqCst);
                            let _permit = gate
                                .acquire()
                                .await
                                .expect("gate should remain open during test");
                            let result = if currency == success_currency {
                                Ok(ExchangeRateQuote {
                                    from: "USD".to_string(),
                                    to: currency.clone(),
                                    rate: 123.45,
                                    source_kind: ExchangeRateSourceKind::Live,
                                    source_timestamp: new_ts,
                                    source_timestamp_text: "2026-03-16 09:00:00 UTC".to_string(),
                                    fetched_at_utc: new_ts,
                                })
                            } else {
                                Err(anyhow::anyhow!("synthetic fetch failure for {currency}"))
                            };
                            active.fetch_sub(1, Ordering::SeqCst);
                            result
                        }
                    })
                    .await
            })
        };

        for _ in 0..50 {
            if started.load(Ordering::SeqCst) >= expected_concurrency {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }

        assert_eq!(started.load(Ordering::SeqCst), expected_concurrency);
        assert_eq!(max_active.load(Ordering::SeqCst), expected_concurrency);

        let (mid_entries, mid_refresh_at) = service.test_cache_snapshot().await;
        assert_eq!(mid_entries.len(), initial_entries.len());
        assert_eq!(
            mid_entries.get(failed_currency).map(|entry| entry.usd_rate),
            Some(9.9)
        );
        assert_eq!(
            mid_entries
                .get(failed_currency)
                .map(|entry| entry.source_timestamp),
            Some(old_ts)
        );
        assert_eq!(mid_refresh_at, Some(old_ts));
        assert_eq!(repo.save_calls.load(Ordering::SeqCst), 0);

        gate.add_permits(non_usd_count);

        let result = refresh
            .await
            .expect("refresh task should complete")
            .expect("refresh should succeed");

        assert_eq!(result.target_currency_count, target_currency_count);
        assert_eq!(result.refreshed_currency_count, 1);
        assert_eq!(result.failed_currency_count, non_usd_count - 1);

        let (final_entries, final_refresh_at) = service.test_cache_snapshot().await;
        let refresh_at = final_refresh_at.expect("refresh timestamp");
        assert_eq!(result.last_refresh_at, Some(refresh_at));
        assert!(refresh_at >= old_ts);
        assert_eq!(repo.save_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            final_entries.get("USD").map(|entry| entry.usd_rate),
            Some(1.0)
        );
        assert_eq!(
            final_entries
                .get(success_currency)
                .map(|entry| entry.usd_rate),
            Some(123.45)
        );
        assert_eq!(
            final_entries
                .get(success_currency)
                .map(|entry| entry.source_timestamp),
            Some(new_ts)
        );
        assert_eq!(
            final_entries
                .get(failed_currency)
                .map(|entry| entry.usd_rate),
            Some(9.9)
        );
        assert_eq!(
            final_entries
                .get(failed_currency)
                .map(|entry| entry.source_timestamp),
            Some(old_ts)
        );

        let saved = repo.saved.lock().await;
        assert_eq!(saved.len(), 1);
        let persisted: PersistedExchangeCache =
            serde_json::from_value(saved[0].clone()).expect("deserialize persisted cache");
        assert_eq!(persisted.last_refresh_at, Some(refresh_at));
        assert_eq!(
            persisted
                .entries
                .get(success_currency)
                .map(|entry| entry.usd_rate),
            Some(123.45)
        );
        assert_eq!(
            persisted
                .entries
                .get(failed_currency)
                .map(|entry| entry.usd_rate),
            Some(9.9)
        );
    }
}
