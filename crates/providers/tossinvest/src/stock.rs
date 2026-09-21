//! Stock quote orchestration for Toss Invest.
//!
//! Protocol decoding, batching, caches and quote construction live in sibling
//! modules; this file is the public service boundary.

use std::{collections::BTreeMap, time::Duration};

use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use dynamo_domain_stock::StockQuote;
use dynamo_service_stock::{Error, StockQuoteService};
use tracing::warn;

use crate::{TossInvestClient, TossInvestMarketCalendarService, TossRateLimitGroup};

mod api;
mod batcher;
mod cache;
mod calendar;
mod quote;
#[cfg(test)]
mod tests;
mod types;

#[cfg(test)]
use api::{candles_path, prices_path, stocks_path};
use api::{fetch_candles, fetch_price_batch, normalize_symbol};
use batcher::PriceBatcher;
use cache::{BaselineCloseCache, StockMetadataCache};
use calendar::MarketCalendarCache;
use calendar::baseline_close_target_for_phase;
use quote::build_stock_quote;
#[cfg(test)]
use quote::{calculate_change, close_for_target};
#[cfg(test)]
use types::{BaselineCloseTarget, FetchedStockPrice, StockMetadata};

const PRICE_GROUP: TossRateLimitGroup = TossRateLimitGroup::MarketData;
const STOCK_METADATA_GROUP: TossRateLimitGroup = TossRateLimitGroup::Stock;
const CANDLE_GROUP: TossRateLimitGroup = TossRateLimitGroup::MarketDataChart;
const PRICE_BATCH_LIMIT: usize = 200;
const STOCK_METADATA_BATCH_LIMIT: usize = 200;
const CANDLE_COUNT: usize = 10;
const PRICE_BATCH_DELAY: Duration = Duration::from_millis(10);
const MARKET_CALENDAR_TTL: TimeDelta = TimeDelta::seconds(30);
const REGULAR_CLOSE_SETTLE_GRACE: TimeDelta = TimeDelta::minutes(10);
const INVALID_TICKER: &str = "Invalid Ticker";

#[derive(Clone)]
pub struct TossInvestStockQuoteService {
    client: TossInvestClient,
    market_calendar: TossInvestMarketCalendarService,
    calendar_cache: MarketCalendarCache,
    price_batcher: PriceBatcher,
    metadata_cache: StockMetadataCache,
    baseline_cache: BaselineCloseCache,
}

impl TossInvestStockQuoteService {
    pub fn new(client: TossInvestClient) -> Self {
        Self {
            market_calendar: TossInvestMarketCalendarService::new(client.clone()),
            client,
            calendar_cache: MarketCalendarCache::default(),
            price_batcher: PriceBatcher::new(PRICE_BATCH_DELAY),
            metadata_cache: StockMetadataCache::default(),
            baseline_cache: BaselineCloseCache::default(),
        }
    }

    pub fn client(&self) -> &TossInvestClient {
        &self.client
    }
}

#[async_trait]
impl StockQuoteService for TossInvestStockQuoteService {
    async fn fetch_quote(&self, symbol: &str) -> Result<Option<StockQuote>, Error> {
        let mut results = self.fetch_quotes(&[symbol.to_string()]).await?;
        let Some(result) = results.pop() else {
            return Ok(None);
        };
        match result {
            Ok(quote) => Ok(Some(quote)),
            Err(error) if error == INVALID_TICKER => Ok(None),
            Err(error) => Err(anyhow!(error)),
        }
    }

    async fn fetch_quotes(
        &self,
        symbols: &[String],
    ) -> Result<Vec<Result<StockQuote, String>>, Error> {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }
        let price_results = self
            .price_batcher
            .fetch(
                symbols
                    .iter()
                    .map(|symbol| normalize_symbol(symbol))
                    .collect(),
                {
                    let client = self.client.clone();
                    move |symbols| {
                        let client = client.clone();
                        async move { fetch_price_batch(&client, &symbols).await }
                    }
                },
            )
            .await?;

        // Preserve the provider-wide failure fan-out: callers receive one
        // identical error per requested ticker and no secondary requests run.
        if price_results.iter().all(Result::is_err) {
            return Ok(price_results
                .into_iter()
                .map(|result| match result {
                    Ok(_) => unreachable!("all price results were checked as errors"),
                    Err(error) => Err(error),
                })
                .collect());
        }

        let now = Utc::now();
        let calendar = self
            .calendar_cache
            .fetch_at(now, &self.market_calendar)
            .await?;
        let phase = calendar.classify_at(now);
        let baseline_target = baseline_close_target_for_phase(&calendar, phase, now);
        let priced_symbols = price_results
            .iter()
            .filter_map(|result| result.as_ref().ok().map(|price| price.symbol.clone()))
            .collect::<Vec<_>>();
        let metadata_by_symbol = match self
            .metadata_cache
            .metadata_for(priced_symbols, self.client.clone())
            .await
        {
            Ok(metadata) => metadata,
            Err(error) => {
                warn!(error = %error, "Toss Invest stock metadata request failed; returning quote without metadata");
                BTreeMap::new()
            }
        };

        let mut quotes = Vec::with_capacity(price_results.len());
        for result in price_results {
            let price = match result {
                Ok(price) => price,
                Err(error) => {
                    quotes.push(Err(error));
                    continue;
                }
            };
            let baseline = match self
                .baseline_cache
                .close_for(&price.symbol, baseline_target, {
                    let client = self.client.clone();
                    move |symbol| {
                        let client = client.clone();
                        async move { fetch_candles(&client, &symbol).await }
                    }
                })
                .await
            {
                Ok(baseline) => baseline,
                Err(error) => {
                    quotes.push(Err(error.to_string()));
                    continue;
                }
            };
            let metadata = metadata_by_symbol
                .get(&price.symbol)
                .cloned()
                .unwrap_or(None);
            quotes.push(Ok(build_stock_quote(price, metadata, phase, baseline)));
        }
        Ok(quotes)
    }
}
