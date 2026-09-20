use chrono::Utc;
use dynamo_domain_currency::{ExchangeRateQuote, ExchangeRateSourceKind, supported_currency_specs};
use dynamo_module_kit::{Module, SettingsFieldKind};

use super::{
    CurrencyModule,
    commands::TOSS_EXCHANGE_PROVIDER_FOOTER,
    currency::{
        DEFAULT_EXCHANGE_AMOUNT, TOSS_EXCHANGE_SUPPORT_ERROR, resolve_explicit_exchange_pair,
        resolve_rate_base_currency, sanitize_exchange_pair, sanitize_rate_targets,
        supported_toss_currency,
    },
    render::{
        currency_display_label, currency_option_label, format_decimal, format_rate_board_value,
        normalize_currency,
    },
    settings::{ResolvedExchangeDefaults, currency_select_options},
};

#[test]
fn formats_grouped_decimals_like_js_locale_output() {
    assert_eq!(format_decimal(12345.678), "12,345.68");
    assert_eq!(format_decimal(12345.6), "12,345.6");
    assert_eq!(format_decimal(12345.0), "12,345");
}

#[test]
fn normalizes_currency_to_uppercase() {
    assert_eq!(normalize_currency(" krw "), "KRW");
}

#[test]
fn all_supported_rate_currencies_have_display_labels() {
    for currency in supported_currency_specs() {
        let code = currency.code;
        let label = currency_display_label(code);
        assert!(
            label.ends_with(code),
            "display label should end with currency code for {code}: {label}"
        );
        assert_ne!(
            label, code,
            "supported currency should not fall back to bare code: {code}"
        );
    }
}

#[test]
fn dropdown_labels_include_human_readable_currency_names() {
    assert_eq!(currency_option_label("KRW"), "South Korean Won (KRW)");
    assert_eq!(currency_option_label("USD"), "United States Dollar (USD)");
    assert_eq!(currency_option_label("EUR"), "Euro (EUR)");
}

#[test]
fn rate_row_does_not_render_cached_fallback_text() {
    let quote = ExchangeRateQuote {
        from: "USD".to_string(),
        to: "KRW".to_string(),
        rate: 1_450.0,
        source_kind: ExchangeRateSourceKind::Cache,
        source_timestamp: Utc::now(),
        source_timestamp_text: "now".to_string(),
        fetched_at_utc: Utc::now(),
    };

    assert_eq!(format_rate_board_value(&quote, 1.0), "1,450");
}

#[test]
fn exchange_manifest_mentions_toss_invest_without_cached_fallback_copy() {
    let manifest = CurrencyModule.manifest();

    assert!(manifest.description.contains("Toss Invest"));
    assert!(!manifest.description.contains("cached fallback"));
}

#[test]
fn exchange_footer_text_uses_provider_name_only() {
    assert_eq!(TOSS_EXCHANGE_PROVIDER_FOOTER, "Toss Invest");
    assert!(!TOSS_EXCHANGE_PROVIDER_FOOTER.contains("midRate"));
}

#[test]
fn dropdown_contains_only_krw_and_usd_with_blank_only_where_requested() {
    let exchange_options = currency_select_options(false);
    let rate_options = currency_select_options(true);

    assert_eq!(
        exchange_options
            .iter()
            .map(|option| option.value)
            .collect::<Vec<_>>(),
        vec!["KRW", "USD"]
    );
    assert_eq!(rate_options.first().map(|option| option.value), Some(""));
    assert_eq!(
        rate_options
            .iter()
            .skip(1)
            .map(|option| option.value)
            .collect::<Vec<_>>(),
        vec!["KRW", "USD"]
    );
}

#[test]
fn rate_settings_schema_selects_only_krw_and_usd_targets() {
    let schema = CurrencyModule.command_settings_schema("rate");
    let fields = &schema.sections[0].fields;

    assert_eq!(
        schema.sections[0].description,
        Some(
            "Only KRW and USD are supported. Blank or matching values fall back to the opposite currency for the selected base."
        )
    );

    for field in fields {
        let SettingsFieldKind::Select { options } = &field.kind else {
            panic!("rate field should be a select");
        };

        assert_eq!(
            field.help_text,
            Some(
                "Only KRW and USD are supported. Leave blank or repeat the base currency to use the opposite currency."
            )
        );
        assert_eq!(
            options
                .iter()
                .map(|option| option.value)
                .collect::<Vec<_>>(),
            vec!["", "KRW", "USD"]
        );
    }
}

#[test]
fn persisted_exchange_defaults_sanitize_unsupported_currencies_to_usd_krw() {
    assert_eq!(
        sanitize_exchange_pair("eur".to_string(), "jpy".to_string()),
        ("USD".to_string(), "KRW".to_string())
    );
}

#[test]
fn explicit_unsupported_exchange_input_returns_support_error() {
    let defaults = ResolvedExchangeDefaults::default();

    assert_eq!(
        resolve_explicit_exchange_pair(Some("EUR"), Some("JPY"), &defaults),
        Err(TOSS_EXCHANGE_SUPPORT_ERROR)
    );
}

#[test]
fn explicit_same_currency_exchange_returns_support_error() {
    let defaults = ResolvedExchangeDefaults::default();

    assert_eq!(
        resolve_explicit_exchange_pair(Some("USD"), Some("USD"), &defaults),
        Err(TOSS_EXCHANGE_SUPPORT_ERROR)
    );
}

#[test]
fn explicit_exchange_with_omitted_side_uses_opposite_of_resolved_base() {
    let defaults = ResolvedExchangeDefaults {
        default_from: "USD".to_string(),
        default_to: "USD".to_string(),
        default_amount: DEFAULT_EXCHANGE_AMOUNT,
    };

    assert_eq!(
        resolve_explicit_exchange_pair(Some("USD"), None, &defaults),
        Ok(("USD".to_string(), "KRW".to_string()))
    );
}

#[test]
fn persisted_rate_targets_sanitize_to_opposite_supported_currency() {
    assert_eq!(
        sanitize_rate_targets(
            "USD",
            vec!["usd".to_string(), "krw".to_string(), "usd".to_string()]
        ),
        vec!["KRW".to_string()]
    );
    assert_eq!(
        sanitize_rate_targets("KRW", vec!["krw".to_string(), "usd".to_string()]),
        vec!["USD".to_string()]
    );
}

#[test]
fn blank_or_unsupported_rate_targets_fall_back_to_valid_opposite_currency() {
    assert_eq!(
        sanitize_rate_targets("USD", vec![]),
        vec!["KRW".to_string()]
    );
    assert_eq!(
        sanitize_rate_targets("USD", vec!["".to_string(), "eur".to_string()]),
        vec!["KRW".to_string()]
    );
    assert_eq!(
        sanitize_rate_targets("KRW", vec!["krw".to_string()]),
        vec!["USD".to_string()]
    );
}

#[test]
fn explicit_unsupported_rate_from_fails_validation() {
    let defaults = ResolvedExchangeDefaults::default();

    assert_eq!(
        resolve_rate_base_currency(Some("EUR"), &defaults),
        Err(TOSS_EXCHANGE_SUPPORT_ERROR)
    );
}

#[test]
fn persisted_rate_from_still_sanitizes_to_default_supported_currency() {
    let defaults = ResolvedExchangeDefaults {
        default_from: "EUR".to_string(),
        default_to: "KRW".to_string(),
        default_amount: DEFAULT_EXCHANGE_AMOUNT,
    };

    assert_eq!(
        resolve_rate_base_currency(None, &defaults),
        Ok("USD".to_string())
    );
}

#[test]
fn supported_toss_currency_accepts_only_krw_and_usd() {
    assert_eq!(supported_toss_currency("krw"), Some("KRW".to_string()));
    assert_eq!(supported_toss_currency("usd"), Some("USD".to_string()));
    assert_eq!(supported_toss_currency("eur"), None);
}

#[test]
fn toss_support_error_mentions_only_krw_and_usd() {
    assert!(TOSS_EXCHANGE_SUPPORT_ERROR.contains("KRW"));
    assert!(TOSS_EXCHANGE_SUPPORT_ERROR.contains("USD"));
    assert!(!TOSS_EXCHANGE_SUPPORT_ERROR.contains("EUR"));
}
