use crate::{
    constants::{DOWN_EMOJI, STOCK_REFRESH_BUTTON_ID, UP_EMOJI},
    render::{
        build_etf_embed, build_stock_embed, current_market_data, format_money,
        primary_stock_market_data, provider_failure_message, refresh_components,
        refresh_footer_text, representative_phase, shared_provider_failure,
        stock_embed_color_change, stop_reason_for_phase,
    },
    settings::{
        StockSettings, normalize_symbol, normalize_symbols, parse_stock_settings, settings_schema,
    },
};
use dynamo_domain_stock::StockQuote;
use dynamo_settings::GuildModuleSettings;
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
fn groups_a_repeated_provider_maintenance_error_once() {
    let snapshots: Vec<Result<StockQuote, String>> = vec![
        Err("Toss Invest exchange-rate request failed with status 500 Internal Server Error (code: maintenance, message: 점검 중입니다. 잠시 후 다시 시도해 주세요.)".to_string()),
        Err("Toss Invest exchange-rate request failed with status 500 Internal Server Error (code: maintenance, message: 점검 중입니다. 잠시 후 다시 시도해 주세요.)".to_string()),
    ];

    assert_eq!(
        shared_provider_failure(&snapshots),
        Some(
            "Toss Invest exchange-rate request failed with status 500 Internal Server Error (code: maintenance, message: 점검 중입니다. 잠시 후 다시 시도해 주세요.)"
        )
    );
    assert_eq!(
        provider_failure_message(snapshots[0].as_ref().expect_err("fixture error")),
        "Toss Invest is under maintenance. Please try again later."
    );
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
fn computes_total_updates_from_default_refresh_schedule() {
    assert_eq!(default_total_updates(), 40);
}

#[test]
fn default_refresh_schedule_is_three_seconds_for_two_minutes() {
    let settings = StockSettings::default();
    let schedule = settings.refresh_schedule();

    assert_eq!(schedule.interval_seconds, 3);
    assert_eq!(schedule.duration_seconds, 120);
    assert_eq!(schedule.total_updates(), 40);
}

#[test]
fn refresh_schedule_clamps_interval_below_minimum() {
    let settings = StockSettings {
        refresh_interval_seconds: 1,
        ..StockSettings::default()
    };

    let schedule = settings.refresh_schedule();

    assert_eq!(schedule.interval_seconds, 3);
    assert_eq!(schedule.duration_seconds, 120);
    assert_eq!(schedule.total_updates(), 40);
}

#[test]
fn refresh_schedule_allows_slower_intervals() {
    let settings = StockSettings {
        refresh_interval_seconds: 5,
        ..StockSettings::default()
    };

    let schedule = settings.refresh_schedule();

    assert_eq!(schedule.interval_seconds, 5);
    assert_eq!(schedule.duration_seconds, 120);
    assert_eq!(schedule.total_updates(), 24);
}

#[test]
fn refresh_schedule_clamps_duration_to_minimum() {
    let settings = StockSettings {
        refresh_duration_seconds: 30,
        ..StockSettings::default()
    };

    let schedule = settings.refresh_schedule();

    assert_eq!(schedule.duration_seconds, 60);
    assert_eq!(schedule.interval_seconds, 3);
    assert_eq!(schedule.total_updates(), 20);
}

#[test]
fn refresh_schedule_clamps_duration_to_maximum() {
    let settings = StockSettings {
        refresh_duration_seconds: 300,
        ..StockSettings::default()
    };

    let schedule = settings.refresh_schedule();

    assert_eq!(schedule.duration_seconds, 180);
    assert_eq!(schedule.interval_seconds, 3);
    assert_eq!(schedule.total_updates(), 60);
}

#[test]
fn refresh_schedule_caps_interval_to_effective_duration() {
    let settings = StockSettings {
        refresh_interval_seconds: 999,
        refresh_duration_seconds: 60,
        ..StockSettings::default()
    };

    let schedule = settings.refresh_schedule();

    assert_eq!(schedule.duration_seconds, 60);
    assert_eq!(schedule.interval_seconds, 60);
    assert_eq!(schedule.total_updates(), 1);
}

#[test]
fn refresh_interval_deserialization_defaults_malformed_values() {
    let settings = serde_json::from_value::<StockSettings>(serde_json::json!({
        "default_symbol": "NVDA",
        "etf_tickers": ["SOXL"],
        "refresh_interval_seconds": "not-a-number"
    }))
    .expect("stock settings should tolerate malformed refresh interval only");

    assert_eq!(settings.refresh_schedule().interval_seconds, 3);
}

#[test]
fn refresh_duration_deserialization_defaults_malformed_values() {
    let settings = serde_json::from_value::<StockSettings>(serde_json::json!({
        "default_symbol": "NVDA",
        "etf_tickers": ["SOXL"],
        "refresh_duration_seconds": "not-a-number"
    }))
    .expect("stock settings should tolerate malformed refresh duration only");

    assert_eq!(settings.refresh_schedule().duration_seconds, 120);
}

#[test]
fn refresh_interval_deserialization_accepts_numeric_strings() {
    let settings = serde_json::from_value::<StockSettings>(serde_json::json!({
        "default_symbol": "NVDA",
        "etf_tickers": ["SOXL"],
        "refresh_interval_seconds": "5"
    }))
    .expect("numeric string refresh interval should parse");

    assert_eq!(settings.refresh_schedule().interval_seconds, 5);
}

#[test]
fn refresh_duration_deserialization_accepts_numeric_strings() {
    let settings = serde_json::from_value::<StockSettings>(serde_json::json!({
        "default_symbol": "NVDA",
        "etf_tickers": ["SOXL"],
        "refresh_duration_seconds": "180"
    }))
    .expect("numeric string refresh duration should parse");

    assert_eq!(settings.refresh_schedule().duration_seconds, 180);
}

#[test]
fn refresh_interval_deserialization_defaults_null_values() {
    let settings = serde_json::from_value::<StockSettings>(serde_json::json!({
        "default_symbol": "NVDA",
        "etf_tickers": ["SOXL"],
        "refresh_interval_seconds": null
    }))
    .expect("null refresh interval should default");

    assert_eq!(settings.refresh_schedule().interval_seconds, 3);
}

#[test]
fn refresh_duration_deserialization_defaults_null_values() {
    let settings = serde_json::from_value::<StockSettings>(serde_json::json!({
        "default_symbol": "NVDA",
        "etf_tickers": ["SOXL"],
        "refresh_duration_seconds": null
    }))
    .expect("null refresh duration should default");

    assert_eq!(settings.refresh_schedule().duration_seconds, 120);
}

#[test]
fn null_stock_module_configuration_loads_defaults() {
    let module = GuildModuleSettings {
        enabled: true,
        configuration: serde_json::Value::Null,
    };

    let settings =
        parse_stock_settings(&module).expect("null stock module configuration should use defaults");

    assert_eq!(settings.default_symbol, "NVDA");
    assert_eq!(settings.refresh_schedule().interval_seconds, 3);
    assert_eq!(settings.refresh_schedule().duration_seconds, 120);
}

#[test]
fn stock_settings_reject_unrelated_invalid_configuration() {
    let module = GuildModuleSettings {
        enabled: true,
        configuration: serde_json::json!({
            "default_symbol": 123,
            "etf_tickers": ["SOXL"],
            "refresh_interval_seconds": 3,
            "refresh_duration_seconds": 120
        }),
    };

    assert!(parse_stock_settings(&module).is_err());
}

#[test]
fn stock_settings_schema_exposes_refresh_bounds() {
    let schema = settings_schema();
    let fields = schema
        .sections
        .iter()
        .flat_map(|section| section.fields.iter())
        .map(|field| (field.key, &field.kind))
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_integer_bounds(
        fields
            .get("refresh_interval_seconds")
            .expect("refresh interval field"),
        Some(3),
        None,
    );
    assert_integer_bounds(
        fields
            .get("refresh_duration_seconds")
            .expect("refresh duration field"),
        Some(60),
        Some(180),
    );
}

#[test]
fn refresh_button_renders_disabled_when_requested() {
    let components = refresh_components(STOCK_REFRESH_BUTTON_ID, true);
    let value = serde_json::to_value(&components).expect("serialize components");

    assert!(
        value.to_string().contains("\"disabled\":true"),
        "serialized components should mark refresh button disabled: {value}"
    );
}

#[test]
fn footer_marks_initial_refresh_as_started() {
    assert_eq!(
        refresh_footer_text(0, default_total_updates(), None),
        "Toss Invest · Active"
    );
}

#[test]
fn footer_marks_final_refresh_as_complete() {
    let total = default_total_updates();
    assert_eq!(
        refresh_footer_text(total, total, None),
        "Toss Invest · Done 40/40"
    );
}

#[test]
fn footer_explains_market_closed_stop_reason() {
    assert_eq!(
        refresh_footer_text(0, default_total_updates(), Some("market_closed")),
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
fn current_data_prefers_day_market_values_when_active() {
    let quote = StockQuote {
        phase: "Day Market".to_string(),
        pre_market_price: Some(101.0),
        pre_market_change: Some(1.0),
        pre_market_change_percent: Some(0.01),
        regular_market_price: None,
        regular_market_change: None,
        regular_market_change_percent: None,
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
fn stock_embed_color_uses_day_market_change_when_active() {
    let quote = StockQuote {
        phase: "Day Market".to_string(),
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

fn default_total_updates() -> u32 {
    StockSettings::default().refresh_schedule().total_updates()
}

fn assert_integer_bounds(
    kind: &dynamo_module_kit::SettingsFieldKind,
    expected_min: Option<i64>,
    expected_max: Option<i64>,
) {
    match kind {
        dynamo_module_kit::SettingsFieldKind::Integer { min, max } => {
            assert_eq!(*min, expected_min);
            assert_eq!(*max, expected_max);
        }
        other => panic!("expected integer field kind, got {other:?}"),
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
