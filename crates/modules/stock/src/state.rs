use crate::{
    constants::{MAX_MANUAL_REFRESHES, MAX_STORED_SESSIONS, STOCK_REFRESH_BUTTON_ID},
    render::{StockResponse, build_etf_response, build_stock_response, refresh_components},
    settings::RefreshSchedule,
};
use dynamo_runtime_api::Error;
use dynamo_service_stock::StockQuoteService;
use poise::serenity_prelude::{ChannelId, CreateEmbed, EditMessage, Http};
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
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
    pub(crate) refresh_schedule: RefreshSchedule,
    pub(crate) active: bool,
    pub(crate) generation: u64,
    pub(crate) manual_restart_in_progress: bool,
    pub(crate) manual_refresh_count: u32,
    pub(crate) last_stop_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ManualRestartStart {
    Started,
    ActiveLoop,
    AlreadyInProgress,
    LimitReached,
}

impl StockSession {
    pub(crate) fn new(
        kind: SessionKind,
        service: Arc<dyn StockQuoteService>,
        refresh_schedule: RefreshSchedule,
    ) -> Self {
        Self {
            kind,
            service,
            refresh_schedule,
            active: false,
            generation: 0,
            manual_restart_in_progress: false,
            manual_refresh_count: 0,
            last_stop_reason: None,
        }
    }
}

pub(crate) fn try_begin_manual_restart(session: &mut StockSession) -> ManualRestartStart {
    if session.active {
        return ManualRestartStart::ActiveLoop;
    }

    if session.manual_restart_in_progress {
        return ManualRestartStart::AlreadyInProgress;
    }

    if session.manual_refresh_count >= MAX_MANUAL_REFRESHES {
        return ManualRestartStart::LimitReached;
    }

    session.manual_restart_in_progress = true;
    ManualRestartStart::Started
}

pub(crate) struct SessionEntry {
    message_id: u64,
    pub(crate) session: Arc<Mutex<StockSession>>,
    cancelled: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl SessionEntry {
    fn new(message_id: u64, session: Arc<Mutex<StockSession>>) -> Self {
        Self {
            message_id,
            session,
            cancelled: AtomicBool::new(false),
            worker: Mutex::new(None),
        }
    }
}

pub(crate) struct SessionRegistry {
    sessions: RwLock<HashMap<u64, Arc<SessionEntry>>>,
    max_sessions: usize,
}

impl SessionRegistry {
    pub(crate) fn new(max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            max_sessions,
        }
    }

    pub(crate) async fn register(
        &self,
        message_id: u64,
        session: Arc<Mutex<StockSession>>,
    ) -> Arc<SessionEntry> {
        let entry = Arc::new(SessionEntry::new(message_id, session));
        let obsolete = {
            let mut sessions = self.sessions.write().await;
            let mut obsolete = Vec::with_capacity(2);

            if let Some(replaced) = sessions.remove(&message_id) {
                obsolete.push(replaced);
            }

            if sessions.len() >= self.max_sessions
                && let Some(oldest) = sessions.keys().next().copied()
                && let Some(evicted) = sessions.remove(&oldest)
            {
                obsolete.push(evicted);
            }

            sessions.insert(message_id, entry.clone());
            obsolete
        };

        for old_entry in obsolete {
            Self::cancel_and_join(old_entry).await;
        }

        entry
    }

    pub(crate) async fn get(&self, message_id: u64) -> Option<Arc<SessionEntry>> {
        self.sessions.read().await.get(&message_id).cloned()
    }

    #[cfg(test)]
    pub(crate) async fn remove(&self, message_id: u64) {
        let removed = self.sessions.write().await.remove(&message_id);
        if let Some(entry) = removed {
            Self::cancel_and_join(entry).await;
        }
    }

    pub(crate) async fn is_current(&self, entry: &Arc<SessionEntry>) -> bool {
        self.sessions
            .read()
            .await
            .get(&entry.message_id)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
    }

    pub(crate) async fn remove_if_current(&self, entry: &Arc<SessionEntry>) {
        let removed = {
            let mut sessions = self.sessions.write().await;
            if sessions
                .get(&entry.message_id)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
            {
                sessions.remove(&entry.message_id)
            } else {
                None
            }
        };

        if removed.is_some() {
            entry.cancelled.store(true, Ordering::Release);
        }
    }

    pub(crate) async fn start_worker<F>(&self, entry: &Arc<SessionEntry>, worker: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let previous = {
            let mut slot = entry.worker.lock().await;
            if entry.cancelled.load(Ordering::Acquire) {
                return false;
            }
            slot.take()
        };
        if let Some(previous) = previous {
            previous.abort();
            let _ = previous.await;
        }

        if entry.cancelled.load(Ordering::Acquire) {
            return false;
        }

        let mut pending = Some(tokio::spawn(worker));
        let displaced = {
            let mut slot = entry.worker.lock().await;
            if entry.cancelled.load(Ordering::Acquire) {
                None
            } else {
                slot.replace(pending.take().expect("pending Stock worker handle"))
            }
        };
        if let Some(pending) = pending {
            pending.abort();
            let _ = pending.await;
            return false;
        }
        if let Some(displaced) = displaced {
            displaced.abort();
            let _ = displaced.await;
        }
        true
    }

    async fn cancel_and_join(entry: Arc<SessionEntry>) {
        entry.cancelled.store(true, Ordering::Release);
        let worker = entry.worker.lock().await.take();
        if let Some(worker) = worker {
            worker.abort();
            let _ = worker.await;
        }
    }
}

fn stock_sessions() -> &'static SessionRegistry {
    static SESSIONS: OnceLock<SessionRegistry> = OnceLock::new();
    SESSIONS.get_or_init(|| SessionRegistry::new(MAX_STORED_SESSIONS))
}

pub(crate) async fn register_session(
    message_id: u64,
    session: Arc<Mutex<StockSession>>,
) -> Arc<SessionEntry> {
    stock_sessions().register(message_id, session).await
}

pub(crate) async fn session_for_message(message_id: u64) -> Option<Arc<SessionEntry>> {
    stock_sessions().get(message_id).await
}

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

pub(crate) async fn edit_message(
    http: &Http,
    channel_id: ChannelId,
    message_id: u64,
    embed: CreateEmbed,
    refresh_disabled: bool,
) -> Result<(), Error> {
    channel_id
        .edit_message(
            http,
            message_id,
            EditMessage::new()
                .embed(embed)
                .components(refresh_components(
                    STOCK_REFRESH_BUTTON_ID,
                    refresh_disabled,
                )),
        )
        .await?;
    Ok(())
}

pub(crate) async fn edit_refresh_components(
    http: &Http,
    channel_id: ChannelId,
    message_id: u64,
    disabled: bool,
) -> Result<(), Error> {
    channel_id
        .edit_message(
            http,
            message_id,
            EditMessage::new().components(refresh_components(STOCK_REFRESH_BUTTON_ID, disabled)),
        )
        .await?;
    Ok(())
}
