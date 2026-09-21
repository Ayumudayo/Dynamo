use anyhow::{Error, anyhow};
use chrono::{DateTime, NaiveDate};
use dynamo_domain_stock::StockQuote;

use crate::{TossMarketSessionPhase, models::TossCandleRaw};

use super::types::{BaselineCloseTarget, FetchedStockPrice, PriceChange, StockMetadata};
pub(super) fn build_stock_quote(
    price: FetchedStockPrice,
    metadata: Option<StockMetadata>,
    phase: TossMarketSessionPhase,
    baseline_close: Option<f64>,
) -> StockQuote {
    let change = calculate_change(price.price, baseline_close);
    let mut quote = StockQuote {
        symbol: price.symbol,
        short_name: metadata.as_ref().and_then(|value| value.short_name.clone()),
        long_name: metadata.as_ref().and_then(|value| value.long_name.clone()),
        quote_type: metadata.as_ref().and_then(|value| value.quote_type.clone()),
        currency_label: metadata
            .and_then(|value| value.currency)
            .filter(|currency| !currency.is_empty())
            .unwrap_or(price.currency),
        phase: phase.as_str().to_string(),
        ..StockQuote::default()
    };

    match active_price_field(phase) {
        ActivePriceField::Pre => {
            quote.pre_market_price = Some(price.price);
            quote.pre_market_change = change.change;
            quote.pre_market_change_percent = change.change_percent;
        }
        ActivePriceField::Regular => {
            quote.regular_market_price = Some(price.price);
            quote.regular_market_change = change.change;
            quote.regular_market_change_percent = change.change_percent;
        }
        ActivePriceField::Post => {
            quote.post_market_price = Some(price.price);
            quote.post_market_change = change.change;
            quote.post_market_change_percent = change.change_percent;
        }
    }

    quote
}

pub(super) fn calculate_change(price: f64, baseline_close: Option<f64>) -> PriceChange {
    let change = baseline_close.map(|baseline| price - baseline);
    let change_percent = baseline_close
        .filter(|baseline| *baseline != 0.0)
        .map(|baseline| (price - baseline) / baseline);

    PriceChange {
        change,
        change_percent,
    }
}

pub(super) fn active_price_field(phase: TossMarketSessionPhase) -> ActivePriceField {
    match phase {
        TossMarketSessionPhase::DayMarket | TossMarketSessionPhase::PreMarket => {
            ActivePriceField::Pre
        }
        TossMarketSessionPhase::AfterMarket => ActivePriceField::Post,
        TossMarketSessionPhase::RegularMarket
        | TossMarketSessionPhase::Closed
        | TossMarketSessionPhase::Unknown => ActivePriceField::Regular,
    }
}

pub(super) enum ActivePriceField {
    Pre,
    Regular,
    Post,
}

pub(super) fn close_for_target(
    candles: &[TossCandleRaw],
    target: BaselineCloseTarget,
) -> Result<Option<f64>, Error> {
    let matched = candles
        .iter()
        .filter_map(|candle| {
            let date = candle
                .timestamp
                .as_deref()
                .and_then(candle_date)
                .filter(|date| target.matches(*date))?;
            Some((date, candle))
        })
        .max_by_key(|(date, _)| *date);

    let Some((date, candle)) = matched else {
        return Ok(None);
    };

    let close = candle
        .close_price
        .parse::<f64>()
        .map_err(|error| anyhow!("Toss Invest candle close for {date} was invalid: {error}"))?;
    Ok(Some(close))
}

impl BaselineCloseTarget {
    fn matches(self, candle_date: NaiveDate) -> bool {
        match self {
            Self::Exact(date) => candle_date == date,
            Self::Before(date) => candle_date < date,
        }
    }
}

pub(super) fn candle_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .or_else(|| {
            DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|value| value.date_naive())
        })
}
