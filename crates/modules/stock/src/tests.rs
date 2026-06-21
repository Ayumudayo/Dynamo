use crate::{
    constants::{DOWN_EMOJI, UP_EMOJI},
    render::{
        build_etf_embed, build_stock_embed, current_market_data, format_money,
        primary_stock_market_data, refresh_footer_text, representative_phase,
        stock_embed_color_change, stop_reason_for_phase,
    },
    settings::{normalize_symbol, normalize_symbols},
    state::total_updates,
};
use dynamo_domain_stock::StockQuote;
use poise::serenity_prelude::CreateEmbed;
use serde_json::Value;

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
fn skips_blank_tickers_in_symbol_lists() {
    let normalized = normalize_symbols(vec![
        "soxl".to_string(),
        " ".to_string(),
        "tqqq".to_string(),
        "".to_string(),
    ]);
    assert_eq!(normalized, vec!["SOXL".to_string(), "TQQQ".to_string()]);
}

#[test]
fn computes_total_updates_from_refresh_window() {
    assert_eq!(total_updates(), 24);
}

#[test]
fn footer_marks_initial_refresh_as_started() {
    assert_eq!(
        refresh_footer_text(0, total_updates(), None),
        "Toss Invest · Active"
    );
}

#[test]
fn footer_marks_final_refresh_as_complete() {
    let total = total_updates();
    assert_eq!(
        refresh_footer_text(total, total, None),
        "Toss Invest · Done 24/24"
    );
}

#[test]
fn footer_explains_market_closed_stop_reason() {
    assert_eq!(
        refresh_footer_text(0, total_updates(), Some("market_closed")),
        "Toss Invest · Stopped"
    );
}

#[test]
fn active_toss_market_phases_do_not_stop_refresh() {
    for phase in ["Day Market", "Pre Market", "Regular Market", "After Market"] {
        assert_eq!(
            stop_reason_for_phase(phase),
            None,
            "{phase} should stay active"
        );
    }
}

#[test]
fn closed_and_unknown_market_phases_stop_refresh() {
    assert_eq!(stop_reason_for_phase("Closed"), Some("market_closed"));
    assert_eq!(
        stop_reason_for_phase("Unknown"),
        Some("market_state_unknown")
    );
}

#[test]
fn representative_phase_prefers_active_toss_sessions_over_closed() {
    let snapshots = vec![
        Ok(quote_with_phase("Closed")),
        Ok(quote_with_phase("After Market")),
        Ok(quote_with_phase("Unknown")),
    ];

    assert_eq!(representative_phase(&snapshots), "After Market");
}

#[test]
fn representative_phase_uses_regular_market_when_available() {
    let snapshots = vec![
        Ok(quote_with_phase("After Market")),
        Ok(quote_with_phase("Pre Market")),
        Ok(quote_with_phase("Regular Market")),
        Ok(quote_with_phase("Day Market")),
    ];

    assert_eq!(representative_phase(&snapshots), "Regular Market");
}

#[test]
fn current_data_prefers_pre_market_values_when_active() {
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
fn stock_primary_values_follow_active_phase() {
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
    assert_eq!(current.price, Some(101.0));
    assert_eq!(current.change, Some(1.0));
    assert_eq!(current.change_percent, Some(0.01));
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
fn stock_embed_color_uses_after_hours_change_when_after_market() {
    let quote = StockQuote {
        phase: "After Market".to_string(),
        post_market_change: Some(-1.5),
        regular_market_change: Some(2.0),
        ..StockQuote::default()
    };

    assert_eq!(stock_embed_color_change(&quote), Some(-1.5));
}

#[test]
fn after_market_phase_prefers_after_hours_values_when_available() {
    let quote = StockQuote {
        phase: "After Market".to_string(),
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
fn closed_phase_uses_regular_values_even_when_after_hours_values_exist() {
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
    assert_eq!(current.price, Some(53.03));
    assert_eq!(current.change, Some(1.89));
    assert_eq!(current.change_percent, Some(0.0370));
}

#[test]
fn stock_embed_uses_pre_market_pair_without_duplicate_extended_fields() {
    let quote = StockQuote {
        symbol: "NVDA".to_string(),
        currency_label: "USD".to_string(),
        phase: "Pre Market".to_string(),
        pre_market_price: Some(101.0),
        pre_market_change: Some(1.0),
        pre_market_change_percent: Some(0.01),
        regular_market_price: Some(100.0),
        regular_market_change: Some(-0.5),
        regular_market_change_percent: Some(-0.005),
        ..StockQuote::default()
    };

    let fields = embed_fields(&build_stock_embed(&quote, "Toss Invest · Active"));
    let expected_change = format!("1.00 (1.00%) {UP_EMOJI}");
    assert_eq!(field_value(&fields, "Price"), Some("$101.00"));
    assert_eq!(
        field_value(&fields, "Change"),
        Some(expected_change.as_str())
    );
    assert_eq!(field_name_count(&fields, "Price"), 1);
    assert_eq!(field_name_count(&fields, "Change"), 1);
    assert_no_extended_price_fields(&fields);
    assert_no_auxiliary_price_fields(&fields);
}

#[test]
fn stock_embed_uses_after_market_pair_without_duplicate_extended_fields() {
    let quote = StockQuote {
        symbol: "NVDA".to_string(),
        currency_label: "USD".to_string(),
        phase: "After Market".to_string(),
        post_market_price: Some(52.31),
        post_market_change: Some(-0.72),
        post_market_change_percent: Some(-0.0136),
        regular_market_price: Some(53.03),
        regular_market_change: Some(1.89),
        regular_market_change_percent: Some(0.0370),
        ..StockQuote::default()
    };

    let fields = embed_fields(&build_stock_embed(&quote, "Toss Invest · Active"));
    let expected_change = format!("-0.72 (-1.36%) {DOWN_EMOJI}");
    assert_eq!(field_value(&fields, "Price"), Some("$52.31"));
    assert_eq!(
        field_value(&fields, "Change"),
        Some(expected_change.as_str())
    );
    assert_eq!(field_name_count(&fields, "Price"), 1);
    assert_eq!(field_name_count(&fields, "Change"), 1);
    assert_no_extended_price_fields(&fields);
    assert_no_auxiliary_price_fields(&fields);
}

#[test]
fn etf_embed_uses_after_market_pair_without_duplicate_extended_fields() {
    let tickers = vec!["SOXL".to_string()];
    let snapshots = vec![Ok(StockQuote {
        symbol: "SOXL".to_string(),
        currency_label: "USD".to_string(),
        phase: "After Market".to_string(),
        post_market_price: Some(42.25),
        post_market_change: Some(-0.42),
        post_market_change_percent: Some(-0.0098),
        regular_market_price: Some(42.67),
        regular_market_change: Some(1.15),
        regular_market_change_percent: Some(0.0277),
        ..StockQuote::default()
    })];

    let fields = embed_fields(&build_etf_embed(
        &tickers,
        &snapshots,
        "After Market",
        "Toss Invest · Active",
    ));
    let expected_change = format!("-0.42 (-0.98%) {DOWN_EMOJI}");
    assert_eq!(field_value(&fields, "SOXL"), Some("$42.25"));
    assert_eq!(
        field_value(&fields, "Change"),
        Some(expected_change.as_str())
    );
    assert_eq!(field_name_count(&fields, "SOXL"), 1);
    assert_eq!(field_name_count(&fields, "Change"), 1);
    assert_no_extended_price_fields(&fields);
}

#[test]
fn renders_usd_with_dollar_symbol() {
    assert_eq!(format_money("USD", Some(50.72)), "$50.72");
}

fn quote_with_phase(phase: &str) -> StockQuote {
    StockQuote {
        phase: phase.to_string(),
        ..StockQuote::default()
    }
}

fn embed_fields(embed: &CreateEmbed) -> Vec<(String, String)> {
    let value = serde_json::to_value(embed).expect("serialize embed");
    value
        .get("fields")
        .and_then(Value::as_array)
        .expect("embed fields")
        .iter()
        .map(|field| {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .expect("field name")
                .to_string();
            let value = field
                .get("value")
                .and_then(Value::as_str)
                .expect("field value")
                .to_string();
            (name, value)
        })
        .collect()
}

fn field_value<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(field_name, _)| field_name == name)
        .map(|(_, value)| value.as_str())
}

fn field_name_count(fields: &[(String, String)], name: &str) -> usize {
    fields
        .iter()
        .filter(|(field_name, _)| field_name == name)
        .count()
}

fn assert_no_extended_price_fields(fields: &[(String, String)]) {
    let field_names = fields
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(
        !field_names.iter().any(|name| name.contains("Pre")),
        "unexpected pre-market duplicate fields: {field_names:?}"
    );
    assert!(
        !field_names.iter().any(|name| name.contains("After Hours")),
        "unexpected after-hours duplicate fields: {field_names:?}"
    );
}

fn assert_no_auxiliary_price_fields(fields: &[(String, String)]) {
    let field_names = fields
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(
        !field_names
            .iter()
            .any(|name| matches!(*name, "Day High" | "Day Low" | "Volume")),
        "unexpected auxiliary stock fields: {field_names:?}"
    );
}
