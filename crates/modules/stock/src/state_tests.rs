use crate::{
    refresh::fetch_response_for_session,
    session::{ManualRestartStart, SessionKind, StockSession, try_begin_manual_restart},
    settings::RefreshSchedule,
    state::SessionRegistry,
};
use dynamo_domain_stock::StockQuote;
use dynamo_service_stock::{Error as StockServiceError, StockQuoteService};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Mutex, Notify};

#[tokio::test]
async fn session_response_uses_session_refresh_schedule_total() {
    let schedule = RefreshSchedule {
        interval_seconds: 4,
        duration_seconds: 120,
    };
    let session = Arc::new(Mutex::new(StockSession::new(
        SessionKind::Stock {
            symbol: "SOXL".to_string(),
        },
        Arc::new(FakeStockQuoteService::active_quote("SOXL")),
        schedule,
    )));

    let response = fetch_response_for_session(&session, 1)
        .await
        .expect("fetch response")
        .expect("response");

    let value = serde_json::to_value(response.embed).expect("serialize embed");
    assert_eq!(
        value
            .get("footer")
            .and_then(|footer| footer.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("Toss Invest · 1/30")
    );
}

#[tokio::test]
async fn session_registry_remove_and_readd_cancels_old_worker_before_returning() {
    let registry = SessionRegistry::new(4);
    let old_effects = Arc::new(AtomicUsize::new(0));
    let old_session = test_session("OLD");
    let old_entry = registry.register(7, old_session).await;
    let old_effects_for_worker = old_effects.clone();

    registry
        .start_worker(&old_entry, async move {
            loop {
                tokio::task::yield_now().await;
                old_effects_for_worker.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;

    while old_effects.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }

    registry.remove(7).await;
    let effects_after_remove = old_effects.load(Ordering::SeqCst);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(old_effects.load(Ordering::SeqCst), effects_after_remove);

    let new_entry = registry.register(7, test_session("NEW")).await;
    assert!(registry.is_current(&new_entry).await);
    assert!(!registry.is_current(&old_entry).await);

    let new_effects = Arc::new(AtomicUsize::new(0));
    let new_effects_for_worker = new_effects.clone();
    assert!(
        registry
            .start_worker(&new_entry, async move {
                loop {
                    tokio::task::yield_now().await;
                    new_effects_for_worker.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await
    );
    while new_effects.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(old_effects.load(Ordering::SeqCst), effects_after_remove);
    registry.remove(7).await;
}

#[tokio::test]
async fn session_registry_replacement_cancels_blocked_worker_without_stale_effect() {
    let registry = SessionRegistry::new(4);
    let fetch_started = Arc::new(Notify::new());
    let release_fetch = Arc::new(Notify::new());
    let stale_effects = Arc::new(AtomicUsize::new(0));
    let old_entry = registry.register(9, test_session("OLD")).await;
    let fetch_started_for_worker = fetch_started.clone();
    let release_fetch_for_worker = release_fetch.clone();
    let stale_effects_for_worker = stale_effects.clone();

    registry
        .start_worker(&old_entry, async move {
            fetch_started_for_worker.notify_one();
            release_fetch_for_worker.notified().await;
            stale_effects_for_worker.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    fetch_started.notified().await;
    let new_entry = registry.register(9, test_session("NEW")).await;
    release_fetch.notify_waiters();
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }

    assert_eq!(stale_effects.load(Ordering::SeqCst), 0);
    assert!(registry.is_current(&new_entry).await);
    assert!(!registry.is_current(&old_entry).await);
}

#[tokio::test]
async fn session_registry_eviction_cancels_worker_before_register_returns() {
    let registry = SessionRegistry::new(1);
    let effects = Arc::new(AtomicUsize::new(0));
    let old_entry = registry.register(11, test_session("OLD")).await;
    let effects_for_worker = effects.clone();
    registry
        .start_worker(&old_entry, async move {
            loop {
                tokio::task::yield_now().await;
                effects_for_worker.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;

    while effects.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }

    let new_entry = registry.register(12, test_session("NEW")).await;
    let effects_after_eviction = effects.load(Ordering::SeqCst);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }

    assert_eq!(effects.load(Ordering::SeqCst), effects_after_eviction);
    assert!(!registry.is_current(&old_entry).await);
    assert!(registry.is_current(&new_entry).await);
}

#[tokio::test]
async fn old_worker_self_removal_does_not_remove_replacement_entry() {
    let registry = SessionRegistry::new(4);
    let old_entry = registry.register(13, test_session("OLD")).await;
    let new_entry = registry.register(13, test_session("NEW")).await;

    registry.remove_if_current(&old_entry).await;

    assert!(!registry.is_current(&old_entry).await);
    assert!(registry.is_current(&new_entry).await);
}

#[test]
fn manual_restart_gate_rejects_second_concurrent_start() {
    let mut session = StockSession::new(
        SessionKind::Stock {
            symbol: "SOXL".to_string(),
        },
        Arc::new(FakeStockQuoteService::active_quote("SOXL")),
        RefreshSchedule {
            interval_seconds: 3,
            duration_seconds: 120,
        },
    );

    assert_eq!(
        try_begin_manual_restart(&mut session),
        ManualRestartStart::Started
    );
    assert_eq!(
        try_begin_manual_restart(&mut session),
        ManualRestartStart::AlreadyInProgress
    );
}

fn test_session(symbol: &str) -> Arc<Mutex<StockSession>> {
    Arc::new(Mutex::new(StockSession::new(
        SessionKind::Stock {
            symbol: symbol.to_string(),
        },
        Arc::new(FakeStockQuoteService::active_quote(symbol)),
        RefreshSchedule {
            interval_seconds: 3,
            duration_seconds: 60,
        },
    )))
}

#[derive(Debug, Clone)]
struct FakeStockQuoteService {
    quote: StockQuote,
}

impl FakeStockQuoteService {
    fn active_quote(symbol: &str) -> Self {
        Self {
            quote: StockQuote {
                symbol: symbol.to_string(),
                phase: "Regular Market".to_string(),
                regular_market_price: Some(100.0),
                regular_market_change: Some(1.0),
                regular_market_change_percent: Some(0.01),
                ..StockQuote::default()
            },
        }
    }
}

#[async_trait::async_trait]
impl StockQuoteService for FakeStockQuoteService {
    async fn fetch_quote(&self, symbol: &str) -> Result<Option<StockQuote>, StockServiceError> {
        if symbol.eq_ignore_ascii_case(&self.quote.symbol) {
            Ok(Some(self.quote.clone()))
        } else {
            Ok(None)
        }
    }

    async fn fetch_quotes(
        &self,
        symbols: &[String],
    ) -> Result<Vec<Result<StockQuote, String>>, StockServiceError> {
        Ok(symbols
            .iter()
            .map(|symbol| {
                if symbol.eq_ignore_ascii_case(&self.quote.symbol) {
                    Ok(self.quote.clone())
                } else {
                    Err("not found".to_string())
                }
            })
            .collect())
    }
}
