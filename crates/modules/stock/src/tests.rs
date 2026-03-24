use crate::{
    render::{
        current_market_data, format_money, primary_stock_market_data, refresh_footer_text,
        stock_embed_color_change,
    },
    settings::{normalize_symbol, normalize_symbols},
    state::total_updates,
};
use dynamo_domain_stock::StockQuote;

#[test]
fn normalizes_symbols_to_uppercase() {
    assert_eq!(normalize_symbol(" nvda ".to_string()), "NVDA");
}

#[test]
fn removes_duplicate_tickers() {
    let normalized = normalize_symbols(vec![
        "soxl".to_string(),
        "SOXL".to_string(),
        "tqqq".to_string(),
    ]);
    assert_eq!(normalized, vec!["SOXL".to_string(), "TQQQ".to_string()]);
}

#[test]
fn computes_total_updates_from_refresh_window() {
    assert_eq!(total_updates(), 12);
}

#[test]
fn footer_marks_initial_refresh_as_started() {
    assert_eq!(
        refresh_footer_text(0, total_updates(), None),
        "Yahoo Finance · Active"
    );
}

#[test]
fn footer_marks_final_refresh_as_complete() {
    let total = total_updates();
    assert_eq!(
        refresh_footer_text(total, total, None),
        "Yahoo Finance · Done 12/12"
    );
}

#[test]
fn footer_explains_market_closed_stop_reason() {
    assert_eq!(
        refresh_footer_text(0, total_updates(), Some("market_closed")),
        "Yahoo Finance · Stopped"
    );
}

#[test]
fn prefers_pre_market_values_when_active() {
    let quote = StockQuote {
        phase: "Pre Market".to_string(),
        pre_market_price: Some(101.0),
        pre_market_change: Some(1.0),
        pre_market_change_percent: Some(0.01),
        regular_market_price: Some(100.0),
        regular_market_change: Some(0.5),
        regular_market_change_percent: Some(0.005),
        ..StockQuote::default()
    };

    let current = current_market_data(&quote, &quote.phase);
    assert_eq!(current.price, Some(101.0));
    assert_eq!(current.change, Some(1.0));
    assert_eq!(current.change_percent, Some(0.01));
}

#[test]
fn stock_primary_values_remain_regular_during_pre_market() {
    let quote = StockQuote {
        phase: "Pre Market".to_string(),
        pre_market_price: Some(101.0),
        pre_market_change: Some(1.0),
        pre_market_change_percent: Some(0.01),
        regular_market_price: Some(100.0),
        regular_market_change: Some(0.5),
        regular_market_change_percent: Some(0.005),
        ..StockQuote::default()
    };

    let current = primary_stock_market_data(&quote);
    assert_eq!(current.price, Some(100.0));
    assert_eq!(current.change, Some(0.5));
    assert_eq!(current.change_percent, Some(0.005));
}

#[test]
fn stock_embed_color_uses_pre_market_change_when_active() {
    let quote = StockQuote {
        phase: "Pre Market".to_string(),
        pre_market_change: Some(1.0),
        regular_market_change: Some(-2.0),
        ..StockQuote::default()
    };

    assert_eq!(stock_embed_color_change(&quote), Some(1.0));
}

#[test]
fn stock_embed_color_uses_after_hours_change_when_closed() {
    let quote = StockQuote {
        phase: "Closed".to_string(),
        post_market_change: Some(-1.5),
        regular_market_change: Some(2.0),
        ..StockQuote::default()
    };

    assert_eq!(stock_embed_color_change(&quote), Some(-1.5));
}

#[test]
fn closed_phase_prefers_after_hours_values_when_available() {
    let quote = StockQuote {
        phase: "Closed".to_string(),
        post_market_price: Some(52.31),
        post_market_change: Some(-0.72),
        post_market_change_percent: Some(-0.0136),
        regular_market_price: Some(53.03),
        regular_market_change: Some(1.89),
        regular_market_change_percent: Some(0.0370),
        ..StockQuote::default()
    };

    let current = current_market_data(&quote, &quote.phase);
    assert_eq!(current.price, Some(52.31));
    assert_eq!(current.change, Some(-0.72));
    assert_eq!(current.change_percent, Some(-0.0136));
}

#[test]
fn renders_usd_with_dollar_symbol() {
    assert_eq!(format_money("USD", Some(50.72)), "$50.72");
}
