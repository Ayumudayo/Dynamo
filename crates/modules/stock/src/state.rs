//! Compatibility facade for stock refresh-session state.
//!
//! Callers continue to import the session API from this module while the
//! implementation is separated by state-machine, registry, refresh, and
//! Discord-edit responsibilities.

#[allow(unused_imports)]
pub(crate) use crate::{
    discord::{edit_message, edit_refresh_components},
    refresh::{fetch_response_for_kind, fetch_response_for_session, initialize_session_loop},
    registry::{SessionEntry, SessionRegistry, register_session, session_for_message},
    session::{ManualRestartStart, SessionKind, StockSession, try_begin_manual_restart},
};
