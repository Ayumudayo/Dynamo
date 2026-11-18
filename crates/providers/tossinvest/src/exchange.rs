use std::{future::Future, sync::Arc};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dynamo_domain_currency::{
    ExchangeRateCacheStatus, ExchangeRateQuote, ExchangeRateRefreshResult, ExchangeRateSourceKind,
};
use dynamo_service_exchange::{Error, ExchangeRateService};
use reqwest::Method;
use tokio::sync::{Mutex, RwLock};

use crate::{
    TossInvestClient, TossInvestResponse, TossRateLimitGroup,
    models::{ApiEnvelope, TossExchangeRateRaw},
};

const USD: &str = "USD";
const KRW: &str = "KRW";
const EXCHANGE_RATE_PATH: &str = "/api/v1/exchange-rate?baseCurrency=USD&quoteCurrency=KRW";
const EXCHANGE_RATE_GROUP: TossRateLimitGroup = TossRateLimitGroup::MarketInfo;
const SUPPORTED_PAIR_COUNT: usize = 2;
const WARMED_PAIR_COUNT: usize = 1;
const UNSUPPORTED_PAIR_ERROR: &str =
    "Only KRW and USD are supported by the current Toss Invest exchange-rate provider.";

#[derive(Debug, Clone)]
struct FetchedExchangeRate {
    mid_rate: String,
    valid_from: Option<String>,
    fetched_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct TossInvestMarketDataService {
    client: TossInvestClient,
    last_refresh_at: Arc<RwLock<Option<DateTime<Utc>>>>,
    cached_usd_krw: Arc<RwLock<Option<FetchedExchangeRate>>>,
    fetch_lock: Arc<Mutex<()>>,
}

pub fn exchange_refresh_interval_seconds() -> u64 {
    60 * 60 * 24
}

impl TossInvestMarketDataService {
    pub fn new(client: TossInvestClient) -> Self {
        Self {
            client,
            last_refresh_at: Arc::new(RwLock::new(None)),
            cached_usd_krw: Arc::new(RwLock::new(None)),
            fetch_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn client(&self) -> &TossInvestClient {
        &self.client
    }

    async fn fetch_pair_with<F, Fut>(
        &self,
        from: &str,
        to: &str,
        mut fetch_usd_krw: F,
    ) -> Result<ExchangeRateQuote, Error>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<FetchedExchangeRate, Error>>,
    {
        let from = normalize_currency(from);
        let to = normalize_currency(to);

        if !is_supported_pair(&from, &to) {
            return Err(anyhow!(UNSUPPORTED_PAIR_ERROR));
        }

        let _singleflight = self.fetch_lock.lock().await;
        match fetch_usd_krw(EXCHANGE_RATE_PATH.to_string()).await {
            Ok(fetched) => {
                let quote = build_quote(from, to, &fetched, ExchangeRateSourceKind::Live)?;
                *self.cached_usd_krw.write().await = Some(fetched.clone());
                *self.last_refresh_at.write().await = Some(fetched.fetched_at);
                Ok(quote)
            }
            Err(error) => {
                if let Some(cached) = self.cached_usd_krw.read().await.clone() {
                    return build_quote(from, to, &cached, ExchangeRateSourceKind::Cache);
                }
                Err(error)
            }
        }
    }

    async fn refresh_cache_with<F, Fut>(
        &self,
        mut fetch_usd_krw: F,
    ) -> Result<ExchangeRateRefreshResult, Error>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<FetchedExchangeRate, Error>>,
    {
        let _singleflight = self.fetch_lock.lock().await;
        let fetched = fetch_usd_krw(EXCHANGE_RATE_PATH.to_string())
            .await
            .context("failed to refresh Toss Invest USD/KRW exchange rate")?;
        parse_mid_rate(&fetched.mid_rate)?;

        *self.cached_usd_krw.write().await = Some(fetched.clone());
        let mut last_refresh_at = self.last_refresh_at.write().await;
        *last_refresh_at = Some(fetched.fetched_at);

        Ok(ExchangeRateRefreshResult {
            target_currency_count: SUPPORTED_PAIR_COUNT,
            refreshed_currency_count: WARMED_PAIR_COUNT,
            failed_currency_count: 0,
            last_refresh_at: *last_refresh_at,
        })
    }
}

fn build_quote(
    from: String,
    to: String,
    fetched: &FetchedExchangeRate,
    source_kind: ExchangeRateSourceKind,
) -> Result<ExchangeRateQuote, Error> {
    let usd_krw = parse_mid_rate(&fetched.mid_rate)?;
    let source_timestamp =
        parse_source_timestamp(fetched.valid_from.as_deref(), fetched.fetched_at);
    let rate = if from == USD && to == KRW {
        usd_krw
    } else {
        1.0 / usd_krw
    };

    Ok(ExchangeRateQuote {
        from,
        to,
        rate,
        source_kind,
        source_timestamp,
        source_timestamp_text: format_timestamp(source_timestamp),
        fetched_at_utc: fetched.fetched_at,
    })
}

#[async_trait]
impl ExchangeRateService for TossInvestMarketDataService {
    async fn fetch_pair(&self, from: &str, to: &str) -> Result<ExchangeRateQuote, Error> {
        let client = self.client.clone();
        self.fetch_pair_with(from, to, move |path| {
            let client = client.clone();
            async move { fetch_exchange_rate(&client, &path).await }
        })
        .await
    }

    async fn refresh_cache(&self) -> Result<ExchangeRateRefreshResult, Error> {
        let client = self.client.clone();
        self.refresh_cache_with(move |path| {
            let client = client.clone();
            async move { fetch_exchange_rate(&client, &path).await }
        })
        .await
    }

    async fn cache_status(&self) -> Result<ExchangeRateCacheStatus, Error> {
        let cached_currency_count = if self.cached_usd_krw.read().await.is_some() {
            SUPPORTED_PAIR_COUNT
        } else {
            0
        };
        Ok(ExchangeRateCacheStatus {
            target_currency_count: SUPPORTED_PAIR_COUNT,
            cached_currency_count,
            uses_persisted_cache: false,
            last_refresh_at: *self.last_refresh_at.read().await,
        })
    }

    fn cache_target_count(&self) -> usize {
        SUPPORTED_PAIR_COUNT
    }

    fn uses_persisted_cache(&self) -> bool {
        false
    }
}

async fn fetch_exchange_rate(
    client: &TossInvestClient,
    path: &str,
) -> Result<FetchedExchangeRate, Error> {
    let response = client
        .send_authenticated(EXCHANGE_RATE_GROUP, Method::GET, path)
        .await?;
    build_fetched_exchange_rate(response)
}

fn build_fetched_exchange_rate(response: TossInvestResponse) -> Result<FetchedExchangeRate, Error> {
    let fetched_at = Utc::now();
    if !response.status().is_success() {
        return Err(build_exchange_request_error(&response));
    }

    let payload = response
        .json::<ApiEnvelope<TossExchangeRateRaw>>()
        .context("failed to deserialize Toss Invest exchange-rate response")?;

    Ok(FetchedExchangeRate {
        mid_rate: payload.result.mid_rate,
        valid_from: payload.result.valid_from,
        fetched_at,
    })
}

fn build_exchange_request_error(response: &TossInvestResponse) -> Error {
    response.request_error("exchange-rate").into()
}

fn parse_mid_rate(value: &str) -> Result<f64, Error> {
    value
        .parse::<f64>()
        .map_err(|error| anyhow!("Toss Invest midRate was invalid: {error}"))
}

fn parse_source_timestamp(valid_from: Option<&str>, fetched_at: DateTime<Utc>) -> DateTime<Utc> {
    valid_from
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.to_utc())
        .unwrap_or(fetched_at)
}

fn normalize_currency(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn is_supported_pair(from: &str, to: &str) -> bool {
    matches!((from, to), (USD, KRW) | (KRW, USD))
}

fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()
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

    use crate::TossRateLimitGroup;
    use chrono::{TimeDelta, TimeZone, Utc};
    use dynamo_domain_currency::ExchangeRateSourceKind;

    use super::{
        EXCHANGE_RATE_GROUP, FetchedExchangeRate, TossInvestMarketDataService,
        UNSUPPORTED_PAIR_ERROR,
    };

    fn test_service() -> TossInvestMarketDataService {
        let client = crate::TossInvestClient::new(
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
        );
        TossInvestMarketDataService::new(client)
    }

    #[tokio::test]
    async fn exchange_usd_krw_uses_mid_rate_and_valid_from() {
        let service = test_service();
        let requested_paths = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let requested_paths_for_call = requested_paths.clone();
        let valid_from = "2026-06-18T00:00:00+09:00".to_string();
        let fetched_at = Utc.with_ymd_and_hms(2026, 6, 18, 1, 2, 3).single().unwrap();

        let quote = service
            .fetch_pair_with("usd", "krw", move |path| {
                let requested_paths_for_call = requested_paths_for_call.clone();
                let valid_from = valid_from.clone();
                async move {
                    requested_paths_for_call.lock().await.push(path);
                    Ok(FetchedExchangeRate {
                        mid_rate: "1378.2500".to_string(),
                        valid_from: Some(valid_from),
                        fetched_at,
                    })
                }
            })
            .await
            .unwrap();

        assert_eq!(
            requested_paths.lock().await.as_slice(),
            ["/api/v1/exchange-rate?baseCurrency=USD&quoteCurrency=KRW"]
        );
        assert_eq!(quote.from, "USD");
        assert_eq!(quote.to, "KRW");
        assert_eq!(quote.rate, 1378.25);
        assert_eq!(quote.source_kind, ExchangeRateSourceKind::Live);
        assert_eq!(
            quote.source_timestamp,
            chrono::FixedOffset::east_opt(9 * 60 * 60)
                .unwrap()
                .with_ymd_and_hms(2026, 6, 18, 0, 0, 0)
                .single()
                .unwrap()
                .to_utc()
        );
    }

    #[tokio::test]
    async fn exchange_krw_usd_uses_krw_usd_behavior_without_loss_of_mid_rate_semantics() {
        let service = test_service();
        let requested_paths = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let requested_paths_for_call = requested_paths.clone();
        let fetched_at = Utc.with_ymd_and_hms(2026, 6, 18, 1, 2, 3).single().unwrap();

        let quote = service
            .fetch_pair_with("KRW", "USD", move |path| {
                let requested_paths_for_call = requested_paths_for_call.clone();
                async move {
                    requested_paths_for_call.lock().await.push(path);
                    Ok(FetchedExchangeRate {
                        mid_rate: "1400.0000".to_string(),
                        valid_from: Some("2026-06-18T00:00:00+09:00".to_string()),
                        fetched_at,
                    })
                }
            })
            .await
            .unwrap();

        assert_eq!(
            requested_paths.lock().await.as_slice(),
            ["/api/v1/exchange-rate?baseCurrency=USD&quoteCurrency=KRW"]
        );
        assert_eq!(quote.from, "KRW");
        assert_eq!(quote.to, "USD");
        assert!((quote.rate - (1.0 / 1400.0)).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn exchange_unsupported_pairs_error_mentions_only_krw_and_usd_support() {
        let service = test_service();
        let external_calls = Arc::new(AtomicUsize::new(0));
        let external_calls_for_call = external_calls.clone();

        let error = service
            .fetch_pair_with("EUR", "JPY", move |_path| {
                external_calls_for_call.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(FetchedExchangeRate {
                        mid_rate: "0".to_string(),
                        valid_from: None,
                        fetched_at: Utc::now(),
                    })
                }
            })
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("KRW"));
        assert!(error.contains("USD"));
        assert!(!error.contains("EUR"));
        assert!(!error.contains("JPY"));
        assert_eq!(external_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exchange_same_currency_is_not_reported_as_toss_mid_rate() {
        let service = test_service();
        let external_calls = Arc::new(AtomicUsize::new(0));
        let external_calls_for_call = external_calls.clone();

        let error = service
            .fetch_pair_with("usd", "USD", move |_path| {
                external_calls_for_call.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(FetchedExchangeRate {
                        mid_rate: "9999".to_string(),
                        valid_from: None,
                        fetched_at: Utc::now() + TimeDelta::days(10),
                    })
                }
            })
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), UNSUPPORTED_PAIR_ERROR);
        assert_eq!(external_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exchange_invalid_valid_from_falls_back_to_fetch_time() {
        let service = test_service();
        let fetched_at = Utc.with_ymd_and_hms(2026, 6, 18, 1, 2, 3).single().unwrap();

        let quote = service
            .fetch_pair_with("USD", "KRW", move |_path| async move {
                Ok(FetchedExchangeRate {
                    mid_rate: "1378.2500".to_string(),
                    valid_from: Some("not-a-timestamp".to_string()),
                    fetched_at,
                })
            })
            .await
            .unwrap();

        assert_eq!(quote.source_timestamp, fetched_at);
    }

    #[test]
    fn exchange_endpoint_uses_market_info_rate_limit_group() {
        assert_eq!(EXCHANGE_RATE_GROUP, TossRateLimitGroup::MarketInfo);
    }

    #[tokio::test]
    async fn exchange_falls_back_to_cached_rate_when_upstream_errors() {
        use dynamo_service_exchange::ExchangeRateService;

        let service = test_service();
        let fetched_at = Utc.with_ymd_and_hms(2026, 6, 18, 1, 2, 3).single().unwrap();
        let refresh = service
            .refresh_cache_with(move |_path| async move {
                Ok(FetchedExchangeRate {
                    mid_rate: "1380.0".to_string(),
                    valid_from: None,
                    fetched_at,
                })
            })
            .await
            .expect("initial cache refresh should succeed");
        assert_eq!(refresh.refreshed_currency_count, 1);

        let status = service.cache_status().await.expect("cache status");
        assert_eq!(status.cached_currency_count, 2);

        let quote = service
            .fetch_pair_with("USD", "KRW", |_path| async {
                Err(anyhow::anyhow!("upstream down"))
            })
            .await
            .expect("cached quote should be returned when upstream fails");

        assert_eq!(quote.rate, 1380.0);
        assert_eq!(quote.source_kind, ExchangeRateSourceKind::Cache);
    }
}
