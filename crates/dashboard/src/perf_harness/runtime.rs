use std::{
    fmt,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::Result;
use tokio::sync::oneshot;

use super::{
    config::{FixtureIdentity, FixtureMode, PerfHarnessConfig, random_control_secret},
    fixture::FixtureData,
};

pub(crate) struct PerfRuntime {
    pub(super) revision: String,
    pub(super) nonce: String,
    pub(super) fixture_mode: FixtureMode,
    pub(super) fixture: FixtureIdentity,
    pub(super) guild_id: u64,
    guild_path: String,
    pub(super) cookie_value: String,
    bot_present: bool,
    guild_lookup_delay_ms: u64,
    pub(super) outbound_calls: AtomicU64,
    pub(super) browser_outbound_attempts: AtomicU64,
    pub(super) denied_requests: AtomicU64,
    pub(super) server_write_attempts: AtomicU64,
    pub(super) repository_reads: AtomicU64,
    pub(super) repository_mutations: AtomicU64,
    pub(super) provider_guild_lookups: AtomicU64,
    shutdown_sender: Mutex<Option<oneshot::Sender<()>>>,
}

impl fmt::Debug for PerfRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerfRuntime")
            .field("revision", &self.revision)
            .field("nonce", &self.nonce)
            .field("fixture_mode", &self.fixture_mode)
            .field("fixture", &self.fixture)
            .field("guild_id", &self.guild_id)
            .field("guild_path", &self.guild_path)
            .field("cookie_value", &"[redacted]")
            .field("bot_present", &self.bot_present)
            .field("guild_lookup_delay_ms", &self.guild_lookup_delay_ms)
            .field("outbound_calls", &self.outbound_calls)
            .field("browser_outbound_attempts", &self.browser_outbound_attempts)
            .field("denied_requests", &self.denied_requests)
            .field("server_write_attempts", &self.server_write_attempts)
            .field("repository_reads", &self.repository_reads)
            .field("repository_mutations", &self.repository_mutations)
            .field("provider_guild_lookups", &self.provider_guild_lookups)
            .field("shutdown_sender", &"[redacted]")
            .finish()
    }
}

impl PerfRuntime {
    pub(super) fn new(
        config: &PerfHarnessConfig,
        fixture: &FixtureData,
        shutdown_sender: oneshot::Sender<()>,
    ) -> Result<Self> {
        Ok(Self {
            revision: config.revision.clone(),
            nonce: config.nonce.clone(),
            fixture_mode: config.fixture_mode,
            fixture: config.fixture.clone(),
            guild_id: fixture.guild_id(),
            guild_path: format!("/guild/{}", fixture.guild_id()),
            cookie_value: random_control_secret()?,
            bot_present: fixture.target.bot_present,
            guild_lookup_delay_ms: fixture.fake_provider_responses.guild_lookup_delay_ms,
            outbound_calls: AtomicU64::new(0),
            browser_outbound_attempts: AtomicU64::new(0),
            denied_requests: AtomicU64::new(0),
            server_write_attempts: AtomicU64::new(0),
            repository_reads: AtomicU64::new(0),
            repository_mutations: AtomicU64::new(0),
            provider_guild_lookups: AtomicU64::new(0),
            shutdown_sender: Mutex::new(Some(shutdown_sender)),
        })
    }

    pub(crate) async fn fixture_bot_present(&self) -> bool {
        self.provider_guild_lookups.fetch_add(1, Ordering::SeqCst);
        if self.guild_lookup_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.guild_lookup_delay_ms)).await;
        }
        self.bot_present
    }
    pub(crate) fn deny_outbound(&self) {
        self.outbound_calls.fetch_add(1, Ordering::SeqCst);
    }
    pub(super) fn increment_browser_outbound_attempts(&self) {
        self.browser_outbound_attempts
            .fetch_add(1, Ordering::SeqCst);
    }
    pub(super) fn increment_server_write_attempts(&self) {
        self.server_write_attempts.fetch_add(1, Ordering::SeqCst);
    }
    pub(super) fn increment_denied_requests(&self) {
        self.denied_requests.fetch_add(1, Ordering::SeqCst);
    }
    pub(super) fn increment_repository_reads(&self) {
        self.repository_reads.fetch_add(1, Ordering::SeqCst);
    }
    pub(super) fn increment_repository_mutations(&self) {
        self.repository_mutations.fetch_add(1, Ordering::SeqCst);
    }
    pub(super) fn trigger_shutdown(&self) -> bool {
        self.shutdown_sender
            .lock()
            .expect("performance shutdown lock poisoned")
            .take()
            .is_some_and(|sender| sender.send(()).is_ok())
    }
    pub(super) fn guild_path(&self) -> &str {
        &self.guild_path
    }
}
