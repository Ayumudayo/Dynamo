use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
use dynamo_repositories::ProviderStateRepository;
use dynamo_service_stock::StockQuoteService;
use tokio::time::{Duration, sleep};

use crate::{
    PROVIDER_ID,
    client::YahooFinanceClient,
    consent::{build_consent_form_body, decode_html_entities},
    models::{
        ChartIndicators, ChartMeta, ChartQuoteSeries, ChartResult, CurrentTradingPeriod,
        TradingPeriod,
    },
    quote::{derive_chart_metrics, normalize_market_phase},
    session::PersistedYahooSession,
};

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

#[test]
fn derives_pre_market_and_regular_session_values_from_chart() {
    let chart = ChartResult {
        meta: ChartMeta {
            symbol: "NVDA".to_string(),
            short_name: None,
            long_name: None,
            currency: Some("USD".to_string()),
            regular_market_price: None,
            regular_market_day_high: None,
            regular_market_day_low: None,
            regular_market_volume: None,
            chart_previous_close: Some(180.0),
            current_trading_period: Some(CurrentTradingPeriod {
                pre: TradingPeriod {
                    start: 100,
                    end: 199,
                },
                regular: TradingPeriod {
                    start: 200,
                    end: 299,
                },
                post: TradingPeriod {
                    start: 300,
                    end: 399,
                },
            }),
        },
        timestamp: vec![110, 120, 210, 220, 310],
        indicators: ChartIndicators {
            quote: vec![ChartQuoteSeries {
                close: vec![
                    Some(181.0),
                    Some(182.0),
                    Some(183.0),
                    Some(184.0),
                    Some(185.0),
                ],
                high: vec![
                    Some(181.5),
                    Some(182.5),
                    Some(183.5),
                    Some(184.5),
                    Some(185.5),
                ],
                low: vec![
                    Some(180.5),
                    Some(181.5),
                    Some(182.5),
                    Some(183.5),
                    Some(184.5),
                ],
                volume: vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0), Some(50.0)],
            }],
        },
    };

    let derived = derive_chart_metrics(&chart);
    assert_eq!(derived.pre_market_price, Some(182.0));
    assert_eq!(derived.regular_market_price, Some(184.0));
    assert_eq!(derived.post_market_price, Some(185.0));
    assert_eq!(derived.pre_market_change, Some(2.0));
    assert_eq!(derived.pre_market_change_percent, Some(2.0 / 180.0));
    assert_eq!(derived.post_market_change, Some(1.0));
    assert_eq!(derived.post_market_change_percent, Some(1.0 / 184.0));
    assert_eq!(derived.regular_market_day_high, Some(184.5));
    assert_eq!(derived.regular_market_day_low, Some(182.5));
    assert_eq!(derived.regular_market_volume, Some(70.0));
}

#[test]
fn normalizes_only_true_pre_market_as_pre_market() {
    assert_eq!(normalize_market_phase("REGULAR"), "Regular Market");
    assert_eq!(normalize_market_phase("PRE"), "Pre Market");
    assert_eq!(normalize_market_phase("PREPRE"), "Closed");
    assert_eq!(normalize_market_phase("POST"), "Closed");
    assert_eq!(normalize_market_phase("POSTPOST"), "Closed");
    assert_eq!(normalize_market_phase("CLOSED"), "Closed");
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
async fn loads_persisted_session_only_once_under_concurrency() {
    let repo = Arc::new(CountingRepo {
        load_calls: AtomicUsize::new(0),
    });
    let client = YahooFinanceClient::new(Some(repo.clone())).expect("client");

    let mut handles = Vec::new();
    for _ in 0..6 {
        let client = client.clone();
        handles.push(tokio::spawn(async move {
            client.ensure_loaded_from_repo().await
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
#[ignore = "live network smoke test"]
async fn live_quote_reports_extended_hours_fields_when_active() {
    let client = YahooFinanceClient::new(None).expect("client");
    let quote = client
        .fetch_quote("NVDA")
        .await
        .expect("request should succeed")
        .expect("nvda should resolve");

    println!(
        "phase={} regular={:?} pre={:?} pre_change={:?} pre_change_pct={:?} post={:?} post_change={:?} post_change_pct={:?}",
        quote.phase,
        quote.regular_market_price,
        quote.pre_market_price,
        quote.pre_market_change,
        quote.pre_market_change_percent,
        quote.post_market_price,
        quote.post_market_change,
        quote.post_market_change_percent,
    );

    match quote.phase.as_str() {
        "Pre Market" => {
            assert!(
                quote.pre_market_price.is_some(),
                "pre-market quote should include a pre-market price"
            );
            assert!(
                quote.pre_market_change.is_some(),
                "pre-market quote should include a pre-market change"
            );
            assert!(
                quote.pre_market_change_percent.is_some(),
                "pre-market quote should include a pre-market change percent"
            );
        }
        "Closed" => {
            assert!(
                quote.post_market_price.is_some() || quote.regular_market_price.is_some(),
                "closed quote should include a post-market or regular price"
            );
        }
        _ => {
            assert!(
                quote.regular_market_price.is_some(),
                "regular quote should still include a regular price"
            );
        }
    }
}

#[tokio::test]
#[ignore = "live network smoke test"]
async fn live_fetch_quotes_reports_extended_hours_fields_when_active() {
    let client = YahooFinanceClient::new(None).expect("client");
    let quotes = client
        .fetch_quotes(&["SOXL".to_string()])
        .await
        .expect("request should succeed");
    let quote = quotes
        .into_iter()
        .next()
        .expect("one result")
        .expect("soxl should resolve");

    println!(
        "phase={} regular={:?} pre={:?} pre_change={:?} pre_change_pct={:?} post={:?} post_change={:?} post_change_pct={:?}",
        quote.phase,
        quote.regular_market_price,
        quote.pre_market_price,
        quote.pre_market_change,
        quote.pre_market_change_percent,
        quote.post_market_price,
        quote.post_market_change,
        quote.post_market_change_percent,
    );

    match quote.phase.as_str() {
        "Pre Market" => {
            assert!(
                quote.pre_market_price.is_some(),
                "pre-market ETF quotes should include a pre-market price"
            );
            assert!(
                quote.pre_market_change.is_some(),
                "pre-market ETF quotes should include a pre-market change"
            );
            assert!(
                quote.pre_market_change_percent.is_some(),
                "pre-market ETF quotes should include a pre-market change percent"
            );
        }
        "Closed" => {
            assert!(
                quote.post_market_price.is_some() || quote.regular_market_price.is_some(),
                "closed ETF quote should include a post-market or regular price"
            );
        }
        _ => {
            assert!(
                quote.regular_market_price.is_some(),
                "regular ETF quote should still include a regular price"
            );
        }
    }
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

#[test]
fn thin_root_exports_expected_provider_surface() {
    let _ = crate::YahooFinanceClient::new;
    let _ = crate::client::YahooFinanceClient::new;
    let _ = crate::session::PersistedYahooSession::default;
    let provider_id: &str = crate::PROVIDER_ID;
    assert_eq!(provider_id, "yahoo_finance");
}
