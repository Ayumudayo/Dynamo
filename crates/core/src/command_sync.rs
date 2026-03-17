use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const COMMAND_SYNC_PROVIDER_ID: &str = "discord_command_sync_state";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommandSyncStateStore {
    #[serde(default)]
    pub global: CommandSyncScopeState,
    #[serde(default)]
    pub guilds: BTreeMap<u64, CommandSyncScopeState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommandSyncScopeState {
    pub requested_at: Option<DateTime<Utc>>,
    pub requested_by_user_id: Option<u64>,
    pub requested_by_username: Option<String>,
    pub last_handled_request_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_synced_fingerprint: Option<String>,
    pub last_result: Option<CommandSyncResult>,
    pub last_error: Option<String>,
    pub last_submitted_top_level_commands: Option<usize>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandSyncResult {
    Success,
    Failed,
}

impl CommandSyncStateStore {
    pub fn guild(&self, guild_id: u64) -> Option<&CommandSyncScopeState> {
        self.guilds.get(&guild_id)
    }

    pub fn guild_mut(&mut self, guild_id: u64) -> &mut CommandSyncScopeState {
        self.guilds.entry(guild_id).or_default()
    }

    pub fn pending_guild_ids(&self) -> Vec<u64> {
        self.guilds
            .iter()
            .filter_map(|(guild_id, state)| state.has_pending_request().then_some(*guild_id))
            .collect()
    }
}

impl CommandSyncScopeState {
    pub fn has_pending_request(&self) -> bool {
        match (self.requested_at, self.last_handled_request_at) {
            (Some(requested_at), Some(handled_at)) => handled_at < requested_at,
            (Some(_), None) => true,
            _ => false,
        }
    }

    pub fn request_sync(
        &mut self,
        requested_at: DateTime<Utc>,
        user_id: Option<u64>,
        username: Option<String>,
    ) {
        self.requested_at = Some(requested_at);
        self.requested_by_user_id = user_id;
        self.requested_by_username = username;
    }

    pub fn mark_success(
        &mut self,
        synced_at: DateTime<Utc>,
        fingerprint: String,
        submitted_top_level_commands: usize,
    ) {
        self.last_handled_request_at = self.requested_at.or(Some(synced_at));
        self.last_synced_at = Some(synced_at);
        self.last_synced_fingerprint = Some(fingerprint);
        self.last_result = Some(CommandSyncResult::Success);
        self.last_error = None;
        self.last_submitted_top_level_commands = Some(submitted_top_level_commands);
    }

    pub fn mark_failure(&mut self, failed_at: DateTime<Utc>, error: String) {
        self.last_handled_request_at = self.requested_at.or(Some(failed_at));
        self.last_result = Some(CommandSyncResult::Failed);
        self.last_error = Some(error);
    }

    pub fn is_in_sync_with(&self, fingerprint: &str) -> bool {
        self.last_result == Some(CommandSyncResult::Success)
            && self.last_synced_fingerprint.as_deref() == Some(fingerprint)
    }
}
