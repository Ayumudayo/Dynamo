use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::Arc,
};

use anyhow::anyhow;
use dynamo_service_stock::Error;
use tokio::sync::{Mutex, oneshot};

use crate::{TossInvestClient, models::TossCandleRaw};

use super::{
    STOCK_METADATA_BATCH_LIMIT,
    api::{fetch_stock_metadata_batch, normalize_symbol, normalize_toss_symbol},
    quote::close_for_target,
    types::{BaselineCloseTarget, StockMetadata},
};

#[derive(Clone, Default)]
pub(super) struct StockMetadataCache {
    state: Arc<Mutex<StockMetadataCacheState>>,
}

#[derive(Default)]
pub(super) struct StockMetadataCacheState {
    entries: BTreeMap<String, StockMetadata>,
    in_flight: BTreeMap<String, Vec<oneshot::Sender<MetadataCacheResult>>>,
}

pub(super) type MetadataCacheResult = Result<Option<StockMetadata>, String>;

impl StockMetadataCache {
    pub(super) async fn metadata_for(
        &self,
        symbols: Vec<String>,
        client: TossInvestClient,
    ) -> Result<BTreeMap<String, Option<StockMetadata>>, Error> {
        self.metadata_for_with(symbols, move |symbols| {
            let client = client.clone();
            async move { fetch_stock_metadata_batch(&client, &symbols).await }
        })
        .await
    }

    pub(super) async fn metadata_for_with<F, Fut>(
        &self,
        symbols: Vec<String>,
        mut fetch_batch: F,
    ) -> Result<BTreeMap<String, Option<StockMetadata>>, Error>
    where
        F: FnMut(Vec<String>) -> Fut,
        Fut: Future<Output = Result<BTreeMap<String, StockMetadata>, Error>>,
    {
        let unique_symbols = symbols
            .into_iter()
            .filter_map(|symbol| normalize_toss_symbol(&symbol))
            .collect::<BTreeSet<_>>();

        let mut output = BTreeMap::new();
        let mut receivers = Vec::new();
        let mut to_fetch = Vec::new();
        {
            let mut state = self.state.lock().await;
            for symbol in unique_symbols {
                if let Some(cached) = state.entries.get(&symbol) {
                    output.insert(symbol, Some(cached.clone()));
                } else {
                    let (sender, receiver) = oneshot::channel();
                    let waiters = state.in_flight.entry(symbol.clone()).or_default();
                    if waiters.is_empty() {
                        to_fetch.push(symbol.clone());
                    }
                    waiters.push(sender);
                    receivers.push((symbol, receiver));
                }
            }
        }

        for chunk in to_fetch.chunks(STOCK_METADATA_BATCH_LIMIT) {
            let chunk_symbols = chunk.to_vec();
            let fetch_result = fetch_batch(chunk_symbols.clone())
                .await
                .map_err(|error| error.to_string());
            self.complete_fetch(chunk_symbols, fetch_result).await;
        }

        for (symbol, receiver) in receivers {
            let metadata = receiver
                .await
                .map_err(|_| anyhow!("Toss Invest stock metadata request was cancelled"))?
                .map_err(|error| anyhow!(error))?;
            output.insert(symbol, metadata);
        }

        Ok(output)
    }

    pub(super) async fn complete_fetch(
        &self,
        symbols: Vec<String>,
        fetch_result: Result<BTreeMap<String, StockMetadata>, String>,
    ) {
        let mut state = self.state.lock().await;
        for symbol in symbols {
            let result = match &fetch_result {
                Ok(fetched) => {
                    let metadata = fetched.get(&symbol).cloned();
                    if let Some(metadata) = metadata.as_ref() {
                        state.entries.insert(symbol.clone(), metadata.clone());
                    }
                    Ok(metadata)
                }
                Err(error) => Err(error.clone()),
            };

            if let Some(waiters) = state.in_flight.remove(&symbol) {
                for waiter in waiters {
                    let _ = waiter.send(result.clone());
                }
            }
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct BaselineCloseCache {
    state: Arc<Mutex<BaselineCloseCacheState>>,
}

#[derive(Default)]
pub(super) struct BaselineCloseCacheState {
    closes: BTreeMap<BaselineCloseCacheKey, f64>,
    in_flight: BTreeMap<BaselineCloseCacheKey, Vec<oneshot::Sender<BaselineCloseCacheResult>>>,
}

pub(super) type BaselineCloseCacheResult = Result<Option<f64>, String>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct BaselineCloseCacheKey {
    pub(super) symbol: String,
    pub(super) target: BaselineCloseTarget,
}

impl BaselineCloseCache {
    pub(super) async fn close_for<F, Fut>(
        &self,
        symbol: &str,
        target: BaselineCloseTarget,
        fetch_candles: F,
    ) -> Result<Option<f64>, Error>
    where
        F: FnOnce(String) -> Fut + Send,
        Fut: Future<Output = Result<Vec<TossCandleRaw>, Error>> + Send,
    {
        let key = BaselineCloseCacheKey {
            symbol: normalize_symbol(symbol),
            target,
        };

        let (receiver, should_fetch) = {
            let mut state = self.state.lock().await;
            if let Some(close) = state.closes.get(&key) {
                return Ok(Some(*close));
            }

            let (sender, receiver) = oneshot::channel();
            let waiters = state.in_flight.entry(key.clone()).or_default();
            let should_fetch = waiters.is_empty();
            waiters.push(sender);
            (receiver, should_fetch)
        };

        if should_fetch {
            let result = match fetch_candles(key.symbol.clone()).await {
                Ok(candles) => {
                    close_for_target(&candles, target).map_err(|error| error.to_string())
                }
                Err(error) => Err(error.to_string()),
            };
            self.complete_fetch(key.clone(), result).await;
        }

        receiver
            .await
            .map_err(|_| anyhow!("Toss Invest candle baseline request was cancelled"))?
            .map_err(|error| anyhow!(error))
    }

    pub(super) async fn complete_fetch(
        &self,
        key: BaselineCloseCacheKey,
        result: BaselineCloseCacheResult,
    ) {
        let mut state = self.state.lock().await;
        if let Ok(Some(close)) = result.as_ref() {
            state.closes.insert(key.clone(), *close);
        }

        if let Some(waiters) = state.in_flight.remove(&key) {
            for waiter in waiters {
                let _ = waiter.send(result.clone());
            }
        }
    }
}
