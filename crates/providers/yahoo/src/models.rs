use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ChartEnvelope {
    pub(crate) chart: ChartResponse,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChartResponse {
    pub(crate) result: Option<Vec<ChartResult>>,
    pub(crate) error: Option<YahooApiError>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ChartResult {
    pub(crate) meta: ChartMeta,
    #[serde(default)]
    pub(crate) timestamp: Vec<i64>,
    #[serde(default)]
    pub(crate) indicators: ChartIndicators,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct ChartIndicators {
    #[serde(default)]
    pub(crate) quote: Vec<ChartQuoteSeries>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub(crate) struct ChartQuoteSeries {
    #[serde(default)]
    pub(crate) close: Vec<Option<f64>>,
    #[serde(default)]
    pub(crate) high: Vec<Option<f64>>,
    #[serde(default)]
    pub(crate) low: Vec<Option<f64>>,
    #[serde(default)]
    pub(crate) volume: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChartMeta {
    pub(crate) symbol: String,
    pub(crate) short_name: Option<String>,
    pub(crate) long_name: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) regular_market_price: Option<f64>,
    pub(crate) regular_market_day_high: Option<f64>,
    pub(crate) regular_market_day_low: Option<f64>,
    pub(crate) regular_market_volume: Option<f64>,
    pub(crate) chart_previous_close: Option<f64>,
    pub(crate) current_trading_period: Option<CurrentTradingPeriod>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CurrentTradingPeriod {
    pub(crate) pre: TradingPeriod,
    pub(crate) regular: TradingPeriod,
    pub(crate) post: TradingPeriod,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TradingPeriod {
    pub(crate) start: i64,
    pub(crate) end: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QuoteSummaryEnvelope {
    #[serde(rename = "quoteSummary")]
    pub(crate) quote_summary: QuoteSummaryResponse,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QuoteSummaryResponse {
    pub(crate) result: Option<Vec<QuoteSummaryResult>>,
    pub(crate) error: Option<YahooApiError>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuoteSummaryResult {
    pub(crate) price: Option<PriceModule>,
    pub(crate) summary_detail: Option<SummaryDetailModule>,
    pub(crate) default_key_statistics: Option<DefaultKeyStatisticsModule>,
    pub(crate) financial_data: Option<FinancialDataModule>,
    pub(crate) quote_type: Option<QuoteTypeModule>,
    pub(crate) summary_profile: Option<SummaryProfileModule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PriceModule {
    pub(crate) short_name: Option<String>,
    pub(crate) long_name: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) quote_type: Option<String>,
    pub(crate) exchange_name: Option<String>,
    pub(crate) market_state: Option<String>,
    pub(crate) regular_market_price: Option<NumericField>,
    pub(crate) regular_market_change: Option<NumericField>,
    pub(crate) regular_market_change_percent: Option<NumericField>,
    pub(crate) pre_market_price: Option<NumericField>,
    pub(crate) pre_market_change: Option<NumericField>,
    pub(crate) pre_market_change_percent: Option<NumericField>,
    pub(crate) post_market_price: Option<NumericField>,
    pub(crate) post_market_change: Option<NumericField>,
    pub(crate) post_market_change_percent: Option<NumericField>,
    pub(crate) market_cap: Option<NumericField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SummaryDetailModule {
    pub(crate) market_cap: Option<NumericField>,
    pub(crate) trailing_pe: Option<NumericField>,
    pub(crate) forward_pe: Option<NumericField>,
    pub(crate) dividend_yield: Option<NumericField>,
    pub(crate) fifty_two_week_high: Option<NumericField>,
    pub(crate) fifty_two_week_low: Option<NumericField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DefaultKeyStatisticsModule {
    pub(crate) trailing_eps: Option<NumericField>,
    pub(crate) forward_pe: Option<NumericField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FinancialDataModule {
    pub(crate) current_price: Option<NumericField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuoteTypeModule {
    pub(crate) quote_type: Option<String>,
    pub(crate) exchange: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SummaryProfileModule {
    pub(crate) sector: Option<String>,
    pub(crate) industry: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct YahooApiError {
    pub(crate) code: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum NumericField {
    Raw { raw: Option<f64> },
    Value(f64),
}

impl NumericField {
    pub(crate) fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Raw { raw } => *raw,
            Self::Value(value) => Some(*value),
        }
    }
}
