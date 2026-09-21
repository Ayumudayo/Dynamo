use chrono::NaiveDate;

use crate::{
    models::TossStockRaw,
    stock::api::{clean_optional_string, normalize_symbol},
};
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FetchedStockPrice {
    pub(super) symbol: String,
    pub(super) price: f64,
    pub(super) currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StockMetadata {
    pub(super) symbol: String,
    pub(super) short_name: Option<String>,
    pub(super) long_name: Option<String>,
    pub(super) quote_type: Option<String>,
    pub(super) currency: Option<String>,
}

impl From<TossStockRaw> for StockMetadata {
    fn from(value: TossStockRaw) -> Self {
        let name = clean_optional_string(value.name);
        Self {
            symbol: normalize_symbol(&value.symbol),
            short_name: clean_optional_string(value.short_name).or_else(|| name.clone()),
            long_name: clean_optional_string(value.long_name).or(name),
            quote_type: clean_optional_string(value.quote_type),
            currency: clean_optional_string(value.currency)
                .map(|currency| normalize_symbol(&currency)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PriceChange {
    pub(super) change: Option<f64>,
    pub(super) change_percent: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum BaselineCloseTarget {
    Exact(NaiveDate),
    Before(NaiveDate),
}
