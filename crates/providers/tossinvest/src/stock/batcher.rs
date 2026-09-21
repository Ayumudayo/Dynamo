use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration};

use anyhow::anyhow;
use dynamo_service_stock::Error;
use tokio::sync::{Mutex, oneshot};

use super::{
    INVALID_TICKER, PRICE_BATCH_LIMIT, api::normalize_toss_symbol, types::FetchedStockPrice,
};
#[derive(Clone)]
pub(super) struct PriceBatcher {
    state: Arc<Mutex<PriceBatchState>>,
    delay: Duration,
}

impl PriceBatcher {
    pub(super) fn new(delay: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(PriceBatchState::default())),
            delay,
        }
    }

    pub(super) async fn fetch<F, Fut>(
        &self,
        symbols: Vec<String>,
        fetch_batch: F,
    ) -> Result<Vec<Result<FetchedStockPrice, String>>, Error>
    where
        F: Fn(Vec<String>) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = Result<BTreeMap<String, FetchedStockPrice>, Error>> + Send + 'static,
    {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }

        let mut receivers = Vec::with_capacity(symbols.len());
        let mut should_schedule = false;

        {
            let mut state = self.state.lock().await;
            for symbol in symbols {
                let Some(symbol) = normalize_toss_symbol(&symbol) else {
                    receivers.push(PendingPriceReceiver::Immediate(Err(
                        INVALID_TICKER.to_string()
                    )));
                    continue;
                };

                let (sender, receiver) = oneshot::channel();
                state.pending.entry(symbol).or_default().push(sender);
                receivers.push(PendingPriceReceiver::Receiver(receiver));
            }

            if !state.scheduled && !state.pending.is_empty() {
                state.scheduled = true;
                should_schedule = true;
            }
        }

        if should_schedule {
            self.spawn_flush(fetch_batch);
        }

        let mut results = Vec::with_capacity(receivers.len());
        for receiver in receivers {
            let result = match receiver {
                PendingPriceReceiver::Immediate(result) => result,
                PendingPriceReceiver::Receiver(receiver) => receiver
                    .await
                    .map_err(|_| anyhow!("Toss Invest price batch was cancelled"))?,
            };
            results.push(result);
        }

        Ok(results)
    }

    pub(super) fn spawn_flush<F, Fut>(&self, fetch_batch: F)
    where
        F: Fn(Vec<String>) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = Result<BTreeMap<String, FetchedStockPrice>, Error>> + Send + 'static,
    {
        let state = self.state.clone();
        let delay = self.delay;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            flush_price_batch(state, fetch_batch).await;
        });
    }
}

#[derive(Default)]
pub(super) struct PriceBatchState {
    pending: BTreeMap<String, Vec<oneshot::Sender<Result<FetchedStockPrice, String>>>>,
    scheduled: bool,
}

pub(super) enum PendingPriceReceiver {
    Immediate(Result<FetchedStockPrice, String>),
    Receiver(oneshot::Receiver<Result<FetchedStockPrice, String>>),
}

pub(super) async fn flush_price_batch<F, Fut>(state: Arc<Mutex<PriceBatchState>>, fetch_batch: F)
where
    F: Fn(Vec<String>) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = Result<BTreeMap<String, FetchedStockPrice>, Error>> + Send + 'static,
{
    let pending = {
        let mut state = state.lock().await;
        state.scheduled = false;
        std::mem::take(&mut state.pending)
    };

    if pending.is_empty() {
        return;
    }

    let symbols = pending.keys().cloned().collect::<Vec<_>>();
    let mut resolved = BTreeMap::new();
    for chunk in symbols.chunks(PRICE_BATCH_LIMIT) {
        let chunk_symbols = chunk.to_vec();
        match fetch_batch(chunk_symbols.clone()).await {
            Ok(prices) => {
                for symbol in chunk_symbols {
                    let result = prices
                        .get(&symbol)
                        .cloned()
                        .ok_or_else(|| INVALID_TICKER.to_string());
                    resolved.insert(symbol, result);
                }
            }
            Err(error) => {
                let error = error.to_string();
                for symbol in chunk_symbols {
                    resolved.insert(symbol, Err(error.clone()));
                }
            }
        }
    }

    for (symbol, senders) in pending {
        let result = resolved
            .remove(&symbol)
            .unwrap_or_else(|| Err(INVALID_TICKER.to_string()));
        for sender in senders {
            let _ = sender.send(result.clone());
        }
    }
}
