use std::time::{SystemTime, UNIX_EPOCH};

use dynamo_domain_stock::StockQuote;
use dynamo_service_stock::Error;

use crate::models::{
    ChartResult, CurrentTradingPeriod, NumericField, QuoteSummaryResult, TradingPeriod,
};

pub(crate) fn looks_like_auth_error(error: &Error) -> bool {
    let message = error.to_string();
    message.contains("401")
        || message.contains("403")
        || message.contains("Invalid Crumb")
        || message.contains("Invalid Cookie")
}

pub(crate) fn merge_quote(chart: ChartResult, summary: Option<QuoteSummaryResult>) -> StockQuote {
    let derived = derive_chart_metrics(&chart);
    let meta = &chart.meta;
    let regular_market_change = match (meta.regular_market_price, meta.chart_previous_close) {
        (Some(price), Some(previous_close)) => Some(price - previous_close),
        _ => None,
    };
    let regular_market_change_percent = match (regular_market_change, meta.chart_previous_close) {
        (Some(change), Some(previous_close)) if previous_close != 0.0 => {
            Some(change / previous_close)
        }
        _ => None,
    };

    let price = summary.as_ref().and_then(|value| value.price.as_ref());
    let detail = summary
        .as_ref()
        .and_then(|value| value.summary_detail.as_ref());
    let stats = summary
        .as_ref()
        .and_then(|value| value.default_key_statistics.as_ref());
    let quote_type = summary.as_ref().and_then(|value| value.quote_type.as_ref());
    let profile = summary
        .as_ref()
        .and_then(|value| value.summary_profile.as_ref());
    let financial = summary
        .as_ref()
        .and_then(|value| value.financial_data.as_ref());

    StockQuote {
        symbol: meta.symbol.clone(),
        short_name: price
            .and_then(|value| value.short_name.clone())
            .or_else(|| meta.short_name.clone()),
        long_name: price
            .and_then(|value| value.long_name.clone())
            .or_else(|| meta.long_name.clone()),
        quote_type: price
            .and_then(|value| value.quote_type.clone())
            .or_else(|| quote_type.and_then(|value| value.quote_type.clone())),
        exchange_name: price
            .and_then(|value| value.exchange_name.clone())
            .or_else(|| quote_type.and_then(|value| value.exchange.clone())),
        currency_label: price
            .and_then(|value| value.currency.clone())
            .or_else(|| meta.currency.clone())
            .unwrap_or_default(),
        phase: price
            .and_then(|value| value.market_state.as_ref())
            .map(|value| normalize_market_phase(value))
            .unwrap_or_else(|| infer_market_phase(meta.current_trading_period.as_ref())),
        regular_market_price: meta
            .regular_market_price
            .or(derived.regular_market_price)
            .or_else(|| price.and_then(|value| numeric(value.regular_market_price.as_ref())))
            .or_else(|| financial.and_then(|value| numeric(value.current_price.as_ref()))),
        regular_market_change: regular_market_change
            .or_else(|| price.and_then(|value| numeric(value.regular_market_change.as_ref()))),
        regular_market_change_percent: regular_market_change_percent.or_else(|| {
            price.and_then(|value| numeric(value.regular_market_change_percent.as_ref()))
        }),
        pre_market_price: price
            .and_then(|value| numeric(value.pre_market_price.as_ref()))
            .or(derived.pre_market_price),
        pre_market_change: price
            .and_then(|value| numeric(value.pre_market_change.as_ref()))
            .or(derived.pre_market_change),
        pre_market_change_percent: price
            .and_then(|value| numeric(value.pre_market_change_percent.as_ref()))
            .or(derived.pre_market_change_percent),
        post_market_price: price
            .and_then(|value| numeric(value.post_market_price.as_ref()))
            .or(derived.post_market_price),
        post_market_change: price
            .and_then(|value| numeric(value.post_market_change.as_ref()))
            .or(derived.post_market_change),
        post_market_change_percent: price
            .and_then(|value| numeric(value.post_market_change_percent.as_ref()))
            .or(derived.post_market_change_percent),
        regular_market_day_high: meta
            .regular_market_day_high
            .or(derived.regular_market_day_high),
        regular_market_day_low: meta
            .regular_market_day_low
            .or(derived.regular_market_day_low),
        regular_market_volume: meta.regular_market_volume.or(derived.regular_market_volume),
        market_cap: detail
            .and_then(|value| numeric(value.market_cap.as_ref()))
            .or_else(|| price.and_then(|value| numeric(value.market_cap.as_ref()))),
        trailing_pe: detail.and_then(|value| numeric(value.trailing_pe.as_ref())),
        forward_pe: detail
            .and_then(|value| numeric(value.forward_pe.as_ref()))
            .or_else(|| stats.and_then(|value| numeric(value.forward_pe.as_ref()))),
        trailing_eps: stats.and_then(|value| numeric(value.trailing_eps.as_ref())),
        dividend_yield: detail.and_then(|value| numeric(value.dividend_yield.as_ref())),
        fifty_two_week_high: detail.and_then(|value| numeric(value.fifty_two_week_high.as_ref())),
        fifty_two_week_low: detail.and_then(|value| numeric(value.fifty_two_week_low.as_ref())),
        sector: profile.and_then(|value| value.sector.clone()),
        industry: profile.and_then(|value| value.industry.clone()),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DerivedChartMetrics {
    pub(crate) regular_market_price: Option<f64>,
    pub(crate) pre_market_price: Option<f64>,
    pub(crate) pre_market_change: Option<f64>,
    pub(crate) pre_market_change_percent: Option<f64>,
    pub(crate) post_market_price: Option<f64>,
    pub(crate) post_market_change: Option<f64>,
    pub(crate) post_market_change_percent: Option<f64>,
    pub(crate) regular_market_day_high: Option<f64>,
    pub(crate) regular_market_day_low: Option<f64>,
    pub(crate) regular_market_volume: Option<f64>,
}

pub(crate) fn derive_chart_metrics(chart: &ChartResult) -> DerivedChartMetrics {
    let Some(periods) = chart.meta.current_trading_period.as_ref() else {
        return DerivedChartMetrics::default();
    };
    let Some(series) = chart.indicators.quote.first() else {
        return DerivedChartMetrics::default();
    };

    let regular_market_price =
        latest_close_in_period(&chart.timestamp, &series.close, &periods.regular);
    let pre_market_price = latest_close_in_period(&chart.timestamp, &series.close, &periods.pre);
    let post_market_price = latest_close_in_period(&chart.timestamp, &series.close, &periods.post);
    let regular_market_day_high = max_in_period(&chart.timestamp, &series.high, &periods.regular);
    let regular_market_day_low = min_in_period(&chart.timestamp, &series.low, &periods.regular);
    let regular_market_volume = sum_in_period(&chart.timestamp, &series.volume, &periods.regular);
    let previous_close = chart.meta.chart_previous_close;

    let pre_market_change = match (pre_market_price, previous_close) {
        (Some(price), Some(previous_close)) => Some(price - previous_close),
        _ => None,
    };
    let pre_market_change_percent = match (pre_market_change, previous_close) {
        (Some(change), Some(previous_close)) if previous_close != 0.0 => {
            Some(change / previous_close)
        }
        _ => None,
    };
    let post_market_change = match (post_market_price, regular_market_price) {
        (Some(price), Some(regular_price)) => Some(price - regular_price),
        _ => None,
    };
    let post_market_change_percent = match (post_market_change, regular_market_price) {
        (Some(change), Some(regular_price)) if regular_price != 0.0 => Some(change / regular_price),
        _ => None,
    };

    DerivedChartMetrics {
        regular_market_price,
        pre_market_price,
        pre_market_change,
        pre_market_change_percent,
        post_market_price,
        post_market_change,
        post_market_change_percent,
        regular_market_day_high,
        regular_market_day_low,
        regular_market_volume,
    }
}

pub(crate) fn normalize_market_phase(value: &str) -> String {
    match value {
        "PRE" => "Pre Market".to_string(),
        "REGULAR" => "Regular Market".to_string(),
        _ => "Closed".to_string(),
    }
}

fn latest_close_in_period(
    timestamps: &[i64],
    closes: &[Option<f64>],
    period: &TradingPeriod,
) -> Option<f64> {
    timestamps
        .iter()
        .zip(closes.iter())
        .filter_map(|(timestamp, close)| {
            ((*timestamp >= period.start) && (*timestamp <= period.end))
                .then_some(*close)
                .flatten()
        })
        .next_back()
}

fn max_in_period(
    timestamps: &[i64],
    values: &[Option<f64>],
    period: &TradingPeriod,
) -> Option<f64> {
    timestamps
        .iter()
        .zip(values.iter())
        .filter_map(|(timestamp, value)| {
            ((*timestamp >= period.start) && (*timestamp <= period.end))
                .then_some(*value)
                .flatten()
        })
        .reduce(f64::max)
}

fn min_in_period(
    timestamps: &[i64],
    values: &[Option<f64>],
    period: &TradingPeriod,
) -> Option<f64> {
    timestamps
        .iter()
        .zip(values.iter())
        .filter_map(|(timestamp, value)| {
            ((*timestamp >= period.start) && (*timestamp <= period.end))
                .then_some(*value)
                .flatten()
        })
        .reduce(f64::min)
}

fn sum_in_period(
    timestamps: &[i64],
    values: &[Option<f64>],
    period: &TradingPeriod,
) -> Option<f64> {
    let mut total = 0.0;
    let mut count = 0usize;
    for (timestamp, value) in timestamps.iter().zip(values.iter()) {
        if *timestamp >= period.start
            && *timestamp <= period.end
            && let Some(value) = value
        {
            total += *value;
            count += 1;
        }
    }
    (count > 0).then_some(total)
}

fn numeric(value: Option<&NumericField>) -> Option<f64> {
    value.and_then(NumericField::as_f64)
}

fn infer_market_phase(periods: Option<&CurrentTradingPeriod>) -> String {
    let Some(periods) = periods else {
        return "Unknown".to_string();
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    if now >= periods.regular.start && now <= periods.regular.end {
        return "Regular Market".to_string();
    }
    if now >= periods.pre.start && now <= periods.pre.end {
        return "Pre Market".to_string();
    }
    "Closed".to_string()
}
