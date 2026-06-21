use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct CurrencySpec {
    pub code: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeRateSourceKind {
    Live,
    Cache,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeRateQuote {
    pub from: String,
    pub to: String,
    pub rate: f64,
    pub source_kind: ExchangeRateSourceKind,
    pub source_timestamp: DateTime<Utc>,
    pub source_timestamp_text: String,
    pub fetched_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExchangeRateCacheStatus {
    pub target_currency_count: usize,
    pub cached_currency_count: usize,
    pub uses_persisted_cache: bool,
    pub last_refresh_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExchangeRateRefreshResult {
    pub target_currency_count: usize,
    pub refreshed_currency_count: usize,
    pub failed_currency_count: usize,
    pub last_refresh_at: Option<DateTime<Utc>>,
}

pub const CACHED_EXCHANGE_CURRENCIES: [&str; 2] = ["USD", "KRW"];

pub const SUPPORTED_CURRENCY_SPECS: &[CurrencySpec] = &[
    CurrencySpec {
        code: "KRW",
        label: "🇰🇷 KRW",
    },
    CurrencySpec {
        code: "USD",
        label: "🇺🇸 USD",
    },
];

pub fn cached_exchange_currencies() -> &'static [&'static str] {
    &CACHED_EXCHANGE_CURRENCIES
}

pub fn supported_currency_specs() -> &'static [CurrencySpec] {
    SUPPORTED_CURRENCY_SPECS
}

pub fn currency_display_label(code: &str) -> Option<&'static str> {
    let normalized = code.trim().to_ascii_uppercase();
    SUPPORTED_CURRENCY_SPECS
        .iter()
        .find(|spec| spec.code == normalized)
        .map(|spec| spec.label)
}

#[cfg(test)]
mod tests {
    use super::{cached_exchange_currencies, supported_currency_specs};

    #[test]
    fn supported_currency_specs_are_limited_to_krw_and_usd() {
        let codes = supported_currency_specs()
            .iter()
            .map(|spec| spec.code)
            .collect::<Vec<_>>();

        assert_eq!(codes, vec!["KRW", "USD"]);
    }

    #[test]
    fn cached_exchange_currencies_are_limited_to_krw_and_usd() {
        assert_eq!(cached_exchange_currencies(), &["USD", "KRW"]);
    }
}
