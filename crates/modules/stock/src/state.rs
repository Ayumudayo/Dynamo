use crate::{
    constants::{
        MAX_REFRESH_TIME_MS, MAX_STORED_SESSIONS, REFRESH_INTERVAL_MS, STOCK_REFRESH_BUTTON_ID,
    },
    render::{build_etf_response, build_stock_response, refresh_components, StockResponse},
};
use dynamo_runtime_api::Error;
use dynamo_service_stock::StockQuoteService;
use poise::serenity_prelude::{ChannelId, CreateEmbed, EditMessage, Http};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    sync::{Mutex, RwLock},
    time::sleep,
};

#[derive(Debug, Clone)]
pub(crate) enum SessionKind {
    Stock { symbol: String },
    Etf { tickers: Vec<String> },
}

pub(crate) struct StockSession {
    pub(crate) kind: SessionKind,
    pub(crate) service: Arc<dyn StockQuoteService>,
    pub(crate) active: bool,
    pub(crate) generation: u64,
    pub(crate) manual_restart_in_progress: bool,
    pub(crate) manual_refresh_count: u32,
    pub(crate) last_stop_reason: Option<&'static str>,
}

impl StockSession {
    pub(crate) fn new(kind: SessionKind, service: Arc<dyn StockQuoteService>) -> Self {
        Self {
            kind,
            service,
            active: false,
            generation: 0,
            manual_restart_in_progress: false,
            manual_refresh_count: 0,
            last_stop_reason: None,
        }
    }
}

fn stock_sessions() -> &'static RwLock<HashMap<u64, Arc<Mutex<StockSession>>>> {
    static SESSIONS: OnceLock<RwLock<HashMap<u64, Arc<Mutex<StockSession>>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub(crate) fn total_updates() -> u32 {
    (MAX_REFRESH_TIME_MS / REFRESH_INTERVAL_MS).max(1)
}

pub(crate) async fn register_session(message_id: u64, session: Arc<Mutex<StockSession>>) {
    let mut sessions = stock_sessions().write().await;
    if sessions.len() >= MAX_STORED_SESSIONS {
        if let Some(oldest) = sessions.keys().next().copied() {
            sessions.remove(&oldest);
        }
    }
    sessions.insert(message_id, session);
}

pub(crate) async fn session_for_message(message_id: u64) -> Option<Arc<Mutex<StockSession>>> {
    let sessions = stock_sessions().read().await;
    sessions.get(&message_id).cloned()
}

async fn remove_session(message_id: u64) {
    stock_sessions().write().await.remove(&message_id);
}

pub(crate) async fn initialize_session_loop(
    http: Arc<Http>,
    channel_id: ChannelId,
    message_id: u64,
    session: Arc<Mutex<StockSession>>,
    stop_reason: Option<&'static str>,
) {
    let mut state = session.lock().await;
    state.last_stop_reason = stop_reason;
    state.manual_restart_in_progress = false;

    if stop_reason.is_some() {
        state.active = false;
        return;
    }

    state.active = true;
    state.generation += 1;
    let generation = state.generation;
    drop(state);

    tokio::spawn(async move {
        let max_updates = total_updates();
        let mut update_count = 0u32;
        let mut consecutive_failures = 0u32;

        loop {
            sleep(Duration::from_millis(REFRESH_INTERVAL_MS as u64)).await;

            {
                let state = session.lock().await;
                if !state.active || state.generation != generation {
                    break;
                }
            }

            update_count += 1;

            let (kind, service) = {
                let state = session.lock().await;
                (state.kind.clone(), state.service.clone())
            };

            let response =
                match fetch_response_for_kind(service.as_ref(), &kind, update_count, max_updates)
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            let mut state = session.lock().await;
                            if state.generation == generation {
                                state.active = false;
                                state.last_stop_reason = Some("fetch_error_threshold");
                            }
                            break;
                        }
                        continue;
                    }
                };

            let Some(response) = response else {
                consecutive_failures += 1;
                if consecutive_failures >= 3 {
                    let mut state = session.lock().await;
                    if state.generation == generation {
                        state.active = false;
                        state.last_stop_reason = Some("fetch_error_threshold");
                    }
                    break;
                }
                continue;
            };

            consecutive_failures = 0;

            if edit_message(&http, channel_id, message_id, response.embed.clone())
                .await
                .is_err()
            {
                let mut state = session.lock().await;
                if state.generation == generation {
                    state.active = false;
                    state.last_stop_reason = Some("interaction_edit_failed");
                }
                remove_session(message_id).await;
                break;
            }

            if let Some(reason) = response.stop_reason {
                let mut state = session.lock().await;
                if state.generation == generation {
                    state.active = false;
                    state.last_stop_reason = Some(reason);
                }
                break;
            }

            if update_count >= max_updates {
                let mut state = session.lock().await;
                if state.generation == generation {
                    state.active = false;
                    state.last_stop_reason = Some("max_refresh_reached");
                }
                break;
            }
        }
    });
}

pub(crate) async fn fetch_response_for_kind(
    service: &dyn StockQuoteService,
    kind: &SessionKind,
    update_count: u32,
    total_updates: u32,
) -> Result<Option<StockResponse>, Error> {
    match kind {
        SessionKind::Stock { symbol } => {
            build_stock_response(service, symbol, update_count, total_updates).await
        }
        SessionKind::Etf { tickers } => {
            build_etf_response(service, tickers, update_count, total_updates).await
        }
    }
}

pub(crate) async fn edit_message(
    http: &Http,
    channel_id: ChannelId,
    message_id: u64,
    embed: CreateEmbed,
) -> Result<(), Error> {
    channel_id
        .edit_message(
            http,
            message_id,
            EditMessage::new()
                .embed(embed)
                .components(refresh_components(STOCK_REFRESH_BUTTON_ID)),
        )
        .await?;
    Ok(())
}
