use crate::{constants::MAX_STORED_SESSIONS, session::StockSession};
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
};

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

#[derive(Default)]
struct RegistryState {
    sessions: HashMap<u64, Arc<SessionEntry>>,
    order: VecDeque<u64>,
}

pub(crate) struct SessionRegistry {
    state: RwLock<RegistryState>,
    max_sessions: usize,
}

impl SessionRegistry {
    pub(crate) fn new(max_sessions: usize) -> Self {
        Self {
            state: RwLock::new(RegistryState::default()),
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
            let mut state = self.state.write().await;
            let mut obsolete = Vec::with_capacity(2);

            if let Some(replaced) = state.sessions.remove(&message_id) {
                state.order.retain(|id| *id != message_id);
                obsolete.push(replaced);
            }

            if state.sessions.len() >= self.max_sessions && self.max_sessions > 0 {
                let inactive_id = state.order.iter().copied().find(|id| {
                    state.sessions.get(id).is_some_and(|candidate| {
                        candidate.session.try_lock().is_ok_and(|session| {
                            !session.active && !session.manual_restart_in_progress
                        })
                    })
                });
                let victim_id = inactive_id.or_else(|| state.order.pop_front());
                if let Some(victim_id) = victim_id {
                    state.order.retain(|id| *id != victim_id);
                    if let Some(evicted) = state.sessions.remove(&victim_id) {
                        obsolete.push(evicted);
                    }
                }
            }

            state.sessions.insert(message_id, entry.clone());
            state.order.push_back(message_id);
            obsolete
        };

        for old_entry in obsolete {
            Self::cancel_and_join(old_entry).await;
        }

        entry
    }

    pub(crate) async fn get(&self, message_id: u64) -> Option<Arc<SessionEntry>> {
        self.state.read().await.sessions.get(&message_id).cloned()
    }

    #[cfg(test)]
    pub(crate) async fn remove(&self, message_id: u64) {
        let removed = {
            let mut state = self.state.write().await;
            state.order.retain(|id| *id != message_id);
            state.sessions.remove(&message_id)
        };
        if let Some(entry) = removed {
            Self::cancel_and_join(entry).await;
        }
    }

    pub(crate) async fn is_current(&self, entry: &Arc<SessionEntry>) -> bool {
        self.state
            .read()
            .await
            .sessions
            .get(&entry.message_id)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
    }

    pub(crate) async fn remove_if_current(&self, entry: &Arc<SessionEntry>) {
        let removed = {
            let mut state = self.state.write().await;
            if state
                .sessions
                .get(&entry.message_id)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
            {
                state.order.retain(|id| *id != entry.message_id);
                state.sessions.remove(&entry.message_id)
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

pub(crate) fn stock_sessions() -> &'static SessionRegistry {
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use dynamo_domain_stock::StockQuote;
    use dynamo_service_stock::{Error as StockError, StockQuoteService};
    use tokio::sync::Mutex;

    use super::SessionRegistry;
    use crate::{
        session::{SessionKind, StockSession},
        settings::StockSettings,
    };

    struct DummyStockService;

    #[async_trait]
    impl StockQuoteService for DummyStockService {
        async fn fetch_quote(&self, _symbol: &str) -> Result<Option<StockQuote>, StockError> {
            Ok(None)
        }

        async fn fetch_quotes(
            &self,
            _symbols: &[String],
        ) -> Result<Vec<Result<StockQuote, String>>, StockError> {
            Ok(Vec::new())
        }
    }

    fn make_session(active: bool) -> Arc<Mutex<StockSession>> {
        let mut session = StockSession::new(
            SessionKind::Stock {
                symbol: "NVDA".to_string(),
            },
            Arc::new(DummyStockService),
            StockSettings::default().refresh_schedule(),
        );
        session.active = active;
        Arc::new(Mutex::new(session))
    }

    #[tokio::test]
    async fn registry_evicts_inactive_sessions_before_active_and_falls_back_to_fifo() {
        let registry = SessionRegistry::new(2);
        registry.register(1, make_session(true)).await;
        registry.register(2, make_session(false)).await;

        // Session 2 is inactive so it should be evicted even though session 1 is older.
        registry.register(3, make_session(true)).await;
        assert!(registry.get(1).await.is_some());
        assert!(registry.get(2).await.is_none());
        assert!(registry.get(3).await.is_some());

        // When all sessions are active, FIFO oldest (session 1) is evicted.
        registry.register(4, make_session(true)).await;
        assert!(registry.get(1).await.is_none());
        assert!(registry.get(3).await.is_some());
        assert!(registry.get(4).await.is_some());
    }
}
