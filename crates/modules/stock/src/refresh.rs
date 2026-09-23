use crate::{
    discord::{edit_message, edit_refresh_components},
    registry::{SessionEntry, stock_sessions},
    render::{StockResponse, build_etf_response, build_stock_response},
    session::{SessionKind, StockSession},
};
use dynamo_runtime_api::Error;
use dynamo_service_stock::StockQuoteService;
use poise::serenity_prelude::{ChannelId, Http};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::sleep};

pub(crate) async fn initialize_session_loop(
    http: Arc<Http>,
    channel_id: ChannelId,
    message_id: u64,
    entry: Arc<SessionEntry>,
    stop_reason: Option<&'static str>,
) {
    if !stock_sessions().is_current(&entry).await {
        return;
    }

    let session = entry.session.clone();
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
    let refresh_schedule = state.refresh_schedule;
    drop(state);

    let worker_entry = entry.clone();
    let worker = async move {
        let max_updates = refresh_schedule.total_updates();
        let interval = Duration::from_secs(refresh_schedule.interval_seconds as u64);
        let mut update_count = 0u32;
        let mut consecutive_failures = 0u32;

        loop {
            sleep(interval).await;

            if !stock_sessions().is_current(&worker_entry).await {
                break;
            }

            {
                let state = session.lock().await;
                if !state.active || state.generation != generation {
                    break;
                }
            }

            update_count += 1;

            let response = match fetch_response_for_session(&session, update_count).await {
                Ok(value) => value,
                Err(_) => {
                    consecutive_failures += 1;
                    if consecutive_failures >= 3 {
                        let mut state = session.lock().await;
                        if state.generation == generation {
                            state.active = false;
                            state.last_stop_reason = Some("fetch_error_threshold");
                        }
                        drop(state);
                        if stock_sessions().is_current(&worker_entry).await {
                            let _ =
                                edit_refresh_components(&http, channel_id, message_id, false).await;
                        }
                        break;
                    }
                    continue;
                }
            };

            if !stock_sessions().is_current(&worker_entry).await {
                break;
            }

            let Some(response) = response else {
                consecutive_failures += 1;
                if consecutive_failures >= 3 {
                    let mut state = session.lock().await;
                    if state.generation == generation {
                        state.active = false;
                        state.last_stop_reason = Some("fetch_error_threshold");
                    }
                    drop(state);
                    if stock_sessions().is_current(&worker_entry).await {
                        let _ = edit_refresh_components(&http, channel_id, message_id, false).await;
                    }
                    break;
                }
                continue;
            };

            consecutive_failures = 0;
            let terminal_after_edit = response.stop_reason.is_some() || update_count >= max_updates;

            if edit_message(
                &http,
                channel_id,
                message_id,
                response.embed.clone(),
                !terminal_after_edit,
            )
            .await
            .is_err()
            {
                let mut state = session.lock().await;
                if state.generation == generation {
                    state.active = false;
                    state.last_stop_reason = Some("interaction_edit_failed");
                }
                drop(state);
                stock_sessions().remove_if_current(&worker_entry).await;
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
    };

    if !stock_sessions().start_worker(&entry, worker).await {
        let mut state = entry.session.lock().await;
        if state.generation == generation {
            state.active = false;
        }
    }
}

pub(crate) async fn fetch_response_for_session(
    session: &Arc<Mutex<StockSession>>,
    update_count: u32,
) -> Result<Option<StockResponse>, Error> {
    let (kind, service, refresh_schedule) = {
        let state = session.lock().await;
        (
            state.kind.clone(),
            state.service.clone(),
            state.refresh_schedule,
        )
    };

    fetch_response_for_kind(
        service.as_ref(),
        &kind,
        update_count,
        refresh_schedule.total_updates(),
    )
    .await
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
