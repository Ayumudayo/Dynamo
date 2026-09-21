use std::collections::BTreeMap;

use anyhow::{Context, anyhow};
use dynamo_service_stock::Error;
use reqwest::Method;

use crate::{
    TossInvestClient, TossInvestResponse,
    models::{ApiEnvelope, CandlePageResponse, TossCandleRaw, TossPriceRaw, TossStockRaw},
};

use super::{
    CANDLE_COUNT, CANDLE_GROUP, PRICE_GROUP, STOCK_METADATA_GROUP,
    types::{FetchedStockPrice, StockMetadata},
};
pub(super) async fn fetch_price_batch(
    client: &TossInvestClient,
    symbols: &[String],
) -> Result<BTreeMap<String, FetchedStockPrice>, Error> {
    let response = client
        .send_authenticated(PRICE_GROUP, Method::GET, &prices_path(symbols))
        .await?;
    build_price_map(response)
}

pub(super) async fn fetch_stock_metadata_batch(
    client: &TossInvestClient,
    symbols: &[String],
) -> Result<BTreeMap<String, StockMetadata>, Error> {
    let response = client
        .send_authenticated(STOCK_METADATA_GROUP, Method::GET, &stocks_path(symbols))
        .await?;
    build_stock_metadata_map(response)
}

pub(super) async fn fetch_candles(
    client: &TossInvestClient,
    symbol: &str,
) -> Result<Vec<TossCandleRaw>, Error> {
    let response = client
        .send_authenticated(CANDLE_GROUP, Method::GET, &candles_path(symbol))
        .await?;
    build_candles(response)
}

pub(super) fn build_price_map(
    response: TossInvestResponse,
) -> Result<BTreeMap<String, FetchedStockPrice>, Error> {
    if !response.status().is_success() {
        return Err(build_stock_request_error("prices", &response));
    }

    let payload = response
        .json::<ApiEnvelope<Vec<TossPriceRaw>>>()
        .context("failed to deserialize Toss Invest prices response")?;
    payload
        .result
        .into_iter()
        .map(|raw| {
            let symbol = normalize_symbol(&raw.symbol);
            let price = raw
                .last_price
                .parse::<f64>()
                .map_err(|error| anyhow!("Toss Invest price for {symbol} was invalid: {error}"))?;
            Ok((
                symbol.clone(),
                FetchedStockPrice {
                    symbol,
                    price,
                    currency: normalize_symbol(&raw.currency),
                },
            ))
        })
        .collect()
}

pub(super) fn build_stock_metadata_map(
    response: TossInvestResponse,
) -> Result<BTreeMap<String, StockMetadata>, Error> {
    if !response.status().is_success() {
        return Err(build_stock_request_error("stocks", &response));
    }

    let payload = response
        .json::<ApiEnvelope<Vec<TossStockRaw>>>()
        .context("failed to deserialize Toss Invest stocks response")?;
    Ok(payload
        .result
        .into_iter()
        .map(StockMetadata::from)
        .map(|metadata| (metadata.symbol.clone(), metadata))
        .collect())
}

pub(super) fn build_candles(response: TossInvestResponse) -> Result<Vec<TossCandleRaw>, Error> {
    if !response.status().is_success() {
        return Err(build_stock_request_error("candles", &response));
    }

    response
        .json::<ApiEnvelope<CandlePageResponse>>()
        .map(|payload| payload.result.candles)
        .context("failed to deserialize Toss Invest candles response")
}

pub(super) fn build_stock_request_error(endpoint: &str, response: &TossInvestResponse) -> Error {
    response.request_error(endpoint).into()
}

pub(super) fn prices_path(symbols: &[String]) -> String {
    format!("/api/v1/prices?symbols={}", symbols.join(","))
}

pub(super) fn stocks_path(symbols: &[String]) -> String {
    format!("/api/v1/stocks?symbols={}", symbols.join(","))
}

pub(super) fn candles_path(symbol: &str) -> String {
    format!(
        "/api/v1/candles?symbol={}&interval=1d&count={CANDLE_COUNT}&adjusted=true",
        normalize_symbol(symbol)
    )
}

pub(super) fn normalize_symbol(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

pub(super) fn normalize_toss_symbol(value: &str) -> Option<String> {
    let symbol = normalize_symbol(value);
    (!symbol.is_empty()
        && symbol
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-'))
    .then_some(symbol)
}

pub(super) fn clean_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
