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
        .close_for("NVDA", BaselineCloseTarget::Exact(baseline_date), {
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
        .close_for("NVDA", BaselineCloseTarget::Exact(baseline_date), {
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
        .close_for(
            "NVDA",
            BaselineCloseTarget::Exact(NaiveDate::from_ymd_opt(2026, 6, 16).unwrap()),
            {
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
            },
        )
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
        .close_for("NVDA", BaselineCloseTarget::Exact(baseline_date), {
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
        .close_for("NVDA", BaselineCloseTarget::Exact(baseline_date), {
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

    let first = cache.close_for("NVDA", BaselineCloseTarget::Exact(baseline_date), {
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
    let second = cache.close_for("NVDA", BaselineCloseTarget::Exact(baseline_date), {
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
fn stock_quote_fields_follow_each_market_session_phase() {
    let phases = [
        (
            TossMarketSessionPhase::DayMarket,
            Some(100.5),
            None,
            None,
            Some(0.5),
            None,
            None,
        ),
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
        (
            TossMarketSessionPhase::Unknown,
            None,
            Some(105.0),
            None,
            None,
            Some(5.0),
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
        "/api/v1/candles?symbol=SOXL&interval=1d&count=10&adjusted=true"
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
fn baseline_target_uses_previous_candle_before_regular_and_recent_close_afterwards() {
    let mut calendar = closed_calendar();
    calendar.today.regular_market = Some(crate::models::TossMarketSessionRaw {
        start_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T22:30:00+09:00").unwrap(),
        end_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T05:00:00+09:00").unwrap(),
    });

    assert_eq!(
        baseline_close_target_for_phase(
            &calendar,
            TossMarketSessionPhase::RegularMarket,
            Utc.with_ymd_and_hms(2026, 6, 18, 14, 0, 0).unwrap(),
        ),
        BaselineCloseTarget::Before(NaiveDate::from_ymd_opt(2026, 6, 18).unwrap())
    );
    assert_eq!(
        baseline_close_target_for_phase(
            &calendar,
            TossMarketSessionPhase::AfterMarket,
            Utc.with_ymd_and_hms(2026, 6, 18, 21, 0, 0).unwrap(),
        ),
        BaselineCloseTarget::Exact(NaiveDate::from_ymd_opt(2026, 6, 18).unwrap())
    );
    assert_eq!(
        baseline_close_target_for_phase(
            &calendar,
            TossMarketSessionPhase::Closed,
            Utc.with_ymd_and_hms(2026, 6, 19, 1, 0, 0).unwrap(),
        ),
        BaselineCloseTarget::Exact(NaiveDate::from_ymd_opt(2026, 6, 18).unwrap())
    );
}

#[test]
fn regular_market_baseline_uses_candle_before_active_market_day_after_kst_midnight() {
    let mut calendar = closed_calendar();
    calendar.previous_business_day = TossMarketDayRaw {
        date: NaiveDate::from_ymd_opt(2026, 6, 18).unwrap(),
        day_market: Some(crate::models::TossMarketSessionRaw {
            start_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T10:00:00+09:00").unwrap(),
            end_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T16:00:00+09:00").unwrap(),
        }),
        pre_market: Some(crate::models::TossMarketSessionRaw {
            start_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T17:00:00+09:00").unwrap(),
            end_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T22:30:00+09:00").unwrap(),
        }),
        regular_market: Some(crate::models::TossMarketSessionRaw {
            start_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T22:30:00+09:00").unwrap(),
            end_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T05:00:00+09:00").unwrap(),
        }),
        after_market: Some(crate::models::TossMarketSessionRaw {
            start_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T05:00:00+09:00").unwrap(),
            end_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T09:00:00+09:00").unwrap(),
        }),
    };
    calendar.today = TossMarketDayRaw {
        date: NaiveDate::from_ymd_opt(2026, 6, 19).unwrap(),
        day_market: None,
        pre_market: None,
        regular_market: None,
        after_market: None,
    };

    let at = Utc.with_ymd_and_hms(2026, 6, 18, 16, 0, 0).unwrap();
    assert_eq!(
        calendar.classify_at(at),
        TossMarketSessionPhase::RegularMarket
    );
    assert_eq!(
        baseline_close_target_for_phase(&calendar, TossMarketSessionPhase::RegularMarket, at),
        BaselineCloseTarget::Before(NaiveDate::from_ymd_opt(2026, 6, 18).unwrap())
    );
}

#[test]
fn after_market_baseline_waits_for_regular_close_settlement_grace() {
    let mut calendar = closed_calendar();
    calendar.today.regular_market = Some(crate::models::TossMarketSessionRaw {
        start_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T22:30:00+09:00").unwrap(),
        end_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T05:00:00+09:00").unwrap(),
    });
    calendar.today.after_market = Some(crate::models::TossMarketSessionRaw {
        start_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T05:00:00+09:00").unwrap(),
        end_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T09:00:00+09:00").unwrap(),
    });

    assert_eq!(
        baseline_close_target_for_phase(
            &calendar,
            TossMarketSessionPhase::AfterMarket,
            chrono::DateTime::parse_from_rfc3339("2026-06-19T05:05:00+09:00")
                .unwrap()
                .to_utc(),
        ),
        BaselineCloseTarget::Before(NaiveDate::from_ymd_opt(2026, 6, 18).unwrap())
    );
    assert_eq!(
        baseline_close_target_for_phase(
            &calendar,
            TossMarketSessionPhase::AfterMarket,
            chrono::DateTime::parse_from_rfc3339("2026-06-19T05:10:00+09:00")
                .unwrap()
                .to_utc(),
        ),
        BaselineCloseTarget::Exact(NaiveDate::from_ymd_opt(2026, 6, 18).unwrap())
    );
}

#[test]
fn closed_post_close_gap_also_waits_for_regular_close_settlement_grace() {
    let mut calendar = closed_calendar();
    calendar.today.regular_market = Some(crate::models::TossMarketSessionRaw {
        start_time: chrono::DateTime::parse_from_rfc3339("2026-06-18T22:30:00+09:00").unwrap(),
        end_time: chrono::DateTime::parse_from_rfc3339("2026-06-19T05:00:00+09:00").unwrap(),
    });
    calendar.today.after_market = None;

    assert_eq!(
        calendar.classify_at(
            chrono::DateTime::parse_from_rfc3339("2026-06-19T05:05:00+09:00")
                .unwrap()
                .to_utc()
        ),
        TossMarketSessionPhase::Closed
    );
    assert_eq!(
        baseline_close_target_for_phase(
            &calendar,
            TossMarketSessionPhase::Closed,
            chrono::DateTime::parse_from_rfc3339("2026-06-19T05:05:00+09:00")
                .unwrap()
                .to_utc(),
        ),
        BaselineCloseTarget::Before(NaiveDate::from_ymd_opt(2026, 6, 18).unwrap())
    );
}

#[test]
fn baseline_close_target_before_active_day_ignores_live_current_day_candle() {
    let candles = vec![
        TossCandleRaw {
            symbol: Some("SOXL".to_string()),
            close_price: "278.96".to_string(),
            timestamp: Some("2026-06-18T13:00:00+09:00".to_string()),
        },
        TossCandleRaw {
            symbol: Some("SOXL".to_string()),
            close_price: "233.86".to_string(),
            timestamp: Some("2026-06-17T13:00:00+09:00".to_string()),
        },
    ];

    assert_eq!(
        close_for_target(
            &candles,
            BaselineCloseTarget::Before(NaiveDate::from_ymd_opt(2026, 6, 18).unwrap())
        )
        .unwrap(),
        Some(233.86)
    );
}
