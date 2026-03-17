use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use dynamo_core::{
    Error, ExchangeRateCacheStatus, ExchangeRateQuote, ExchangeRateRefreshResult,
    ExchangeRateService, ExchangeRateSourceKind, ProviderStateRepository,
};
use reqwest::{Client, header::ACCEPT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

const PROVIDER_ID: &str = "google_finance_exchange";
const GOOGLE_FINANCE_BASE: &str = "https://www.google.com/finance/quote";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) dynamo-rs/0.1 Safari/537.36";
const CACHE_REFRESH_INTERVAL_SECONDS: u64 = 30 * 60;
const SUPPORTED_CURRENCIES: [&str; 18] = [
    "USD", "KRW", "EUR", "GBP", "JPY", "CAD", "CHF", "HKD", "TWD", "AUD", "NZD", "INR", "BRL",
    "PLN", "RUB", "TRY", "CNY", "UAH",
];

#[derive(Clone)]
pub struct GoogleFinanceExchangeService {
    client: Client,
    session_repo: Option<Arc<dyn ProviderStateRepository>>,
    cache: Arc<RwLock<ExchangeRateCache>>,
    loaded_from_repo: Arc<AtomicBool>,
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

        if let Some(value) = repo.load_json(PROVIDER_ID).await? {
            if let Ok(state) = serde_json::from_value::<PersistedExchangeCache>(value) {
                let mut cache = self.cache.write().await;
                cache.entries = state.entries;
                cache.last_refresh_at = state.last_refresh_at;
            }
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

        for currency in SUPPORTED_CURRENCIES
            .iter()
            .copied()
            .filter(|value| *value != "USD")
        {
            match self.fetch_live_pair_internal("USD", currency).await {
                Ok(quote) => {
                    next_entries.insert(
                        currency.to_string(),
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
            target_currency_count: SUPPORTED_CURRENCIES.len(),
            refreshed_currency_count: refreshed,
            failed_currency_count: failed,
            last_refresh_at: if refreshed > 0 {
                Some(refresh_started_at)
            } else {
                self.cache.read().await.last_refresh_at
            },
        })
    }

    async fn cache_status(&self) -> Result<ExchangeRateCacheStatus, Error> {
        self.ensure_loaded_from_repo().await?;
        let cache = self.cache.read().await;
        Ok(ExchangeRateCacheStatus {
            target_currency_count: SUPPORTED_CURRENCIES.len(),
            cached_currency_count: cache.entries.len(),
            uses_persisted_cache: self.session_repo.is_some(),
            last_refresh_at: cache.last_refresh_at,
        })
    }

    fn cache_target_count(&self) -> usize {
        SUPPORTED_CURRENCIES.len()
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
    &SUPPORTED_CURRENCIES
}

#[cfg(test)]
mod tests {
    use super::{
        CachedUsdRate, ExchangeRateCache, PersistedExchangeCache, build_cached_cross_quote,
        parse_google_finance_pair_html,
    };
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;

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
}
