use crate::{constants::MAX_STORED_SESSIONS, session::StockSession};
use std::{
    collections::HashMap,
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
