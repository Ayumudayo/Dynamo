use crate::{constants::MAX_MANUAL_REFRESHES, settings::RefreshSchedule};
use dynamo_service_stock::StockQuoteService;
use std::sync::Arc;

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
