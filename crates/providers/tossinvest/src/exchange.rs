use std::{future::Future, sync::Arc};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dynamo_domain_currency::{
    ExchangeRateCacheStatus, ExchangeRateQuote, ExchangeRateRefreshResult,
    ExchangeRateSourceKind,
};
use dynamo_service_exchange::{Error, ExchangeRateService};
use reqwest::Method;
use tokio::sync::RwLock;

use crate::{
    TossInvestClient, TossInvestResponse, TossRateLimitGroup,
    models::{ApiEnvelope, TossErrorEnvelope, TossExchangeRateRaw},
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
}

impl TossInvestMarketDataService {
    pub fn new(client: TossInvestClient) -> Self {
        Self {
            client,
            last_refresh_at: Arc::new(RwLock::new(None)),
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

        if from == to {
            let fetched_at = Utc::now();
            return Ok(ExchangeRateQuote {
                from,
                to,
                rate: 1.0,
                source_kind: ExchangeRateSourceKind::Live,
                source_timestamp: fetched_at,
                source_timestamp_text: format_timestamp(fetched_at),
                fetched_at_utc: fetched_at,
            });
        }

        if !is_supported_pair(&from, &to) {
            return Err(anyhow!(UNSUPPORTED_PAIR_ERROR));
        }

        let fetched = fetch_usd_krw(EXCHANGE_RATE_PATH.to_string()).await?;
        let usd_krw = parse_mid_rate(&fetched.mid_rate)?;
        let source_timestamp = parse_source_timestamp(fetched.valid_from.as_deref(), fetched.fetched_at);
        let rate = if from == USD && to == KRW {
            usd_krw
        } else {
            1.0 / usd_krw
        };

        Ok(ExchangeRateQuote {
            from,
            to,
            rate,
            source_kind: ExchangeRateSourceKind::Live,
            source_timestamp,
            source_timestamp_text: format_timestamp(source_timestamp),
            fetched_at_utc: fetched.fetched_at,
        })
    }

    async fn refresh_cache_with<F, Fut>(
        &self,
        mut fetch_usd_krw: F,
    ) -> Result<ExchangeRateRefreshResult, Error>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<FetchedExchangeRate, Error>>,
    {
        let fetched = fetch_usd_krw(EXCHANGE_RATE_PATH.to_string())
            .await
            .context("failed to refresh Toss Invest USD/KRW exchange rate")?;
        parse_mid_rate(&fetched.mid_rate)?;

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
        Ok(ExchangeRateCacheStatus {
            target_currency_count: SUPPORTED_PAIR_COUNT,
            cached_currency_count: 0,
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
    if let Ok(error) = response.json::<TossErrorEnvelope>() {
        return anyhow!(
            "Toss Invest exchange-rate request failed with status {} (request_id: {}, code: {}, message: {})",
            response.status(),
            error.error.request_id.as_deref().unwrap_or("unknown"),
            error.error.code,
            error.error.message,
        );
    }

    anyhow!(
        "Toss Invest exchange-rate request failed with status {}",
        response.status()
    )
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

    use chrono::{TimeDelta, TimeZone, Utc};
    use dynamo_domain_currency::ExchangeRateSourceKind;
    use crate::TossRateLimitGroup;

    use super::{EXCHANGE_RATE_GROUP, FetchedExchangeRate, TossInvestMarketDataService};

    fn test_service() -> TossInvestMarketDataService {
        let client = crate::TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                ("TOSSINVEST_CLIENT_ID".to_string(), "test-client-id".to_string()),
                (
                    "TOSSINVEST_CLIENT_SECRET".to_string(),
                    "test-client-secret".to_string(),
                ),
                (
                    "TOSSINVEST_BASE_URL".to_string(),
                    "https://sandbox.openapi.tossinvest.example".to_string(),
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
    async fn exchange_same_currency_returns_one_without_external_call() {
        let service = test_service();
        let external_calls = Arc::new(AtomicUsize::new(0));
        let external_calls_for_call = external_calls.clone();

        let quote = service
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
            .unwrap();

        assert_eq!(quote.from, "USD");
        assert_eq!(quote.to, "USD");
        assert_eq!(quote.rate, 1.0);
        assert_eq!(quote.source_kind, ExchangeRateSourceKind::Live);
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
}
