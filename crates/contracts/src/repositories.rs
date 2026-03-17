use async_trait::async_trait;
use dynamo_domain_giveaway::GiveawayRecord;
use dynamo_domain_invite::{InviteLeaderboardEntry, InviteMemberRecord};
use dynamo_domain_moderation::WarningLogRecord;
use dynamo_domain_stats::MemberStatsRecord;
use dynamo_domain_suggestion::SuggestionRecord;
use dynamo_ops::{DashboardAuditLogEntry, DashboardAuditLogPage, DashboardAuditLogQuery};
use serde_json::Value;

use crate::settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};

pub type Error = anyhow::Error;

#[async_trait]
pub trait GuildSettingsRepository: Send + Sync {
    async fn get_or_create(&self, guild_id: u64) -> Result<GuildSettings, Error>;
    async fn upsert_module_settings(
        &self,
        guild_id: u64,
        module_id: &str,
        settings: GuildModuleSettings,
    ) -> Result<GuildSettings, Error>;
    async fn upsert_command_settings(
        &self,
        guild_id: u64,
        command_id: &str,
        settings: GuildCommandSettings,
    ) -> Result<GuildSettings, Error>;
}

#[async_trait]
pub trait DeploymentSettingsRepository: Send + Sync {
    async fn get(&self) -> Result<DeploymentSettings, Error>;
    async fn upsert_module_settings(
        &self,
        module_id: &str,
        settings: DeploymentModuleSettings,
    ) -> Result<DeploymentSettings, Error>;
    async fn upsert_command_settings(
        &self,
        command_id: &str,
        settings: DeploymentCommandSettings,
    ) -> Result<DeploymentSettings, Error>;
}

#[async_trait]
pub trait ProviderStateRepository: Send + Sync {
    async fn load_json(&self, provider_id: &str) -> Result<Option<Value>, Error>;
    async fn save_json(&self, provider_id: &str, value: Value) -> Result<(), Error>;
}

#[async_trait]
pub trait SuggestionsRepository: Send + Sync {
    async fn create(&self, record: SuggestionRecord) -> Result<SuggestionRecord, Error>;
    async fn get_by_message(
        &self,
        guild_id: u64,
        message_id: u64,
    ) -> Result<Option<SuggestionRecord>, Error>;
    async fn save(&self, record: SuggestionRecord) -> Result<SuggestionRecord, Error>;
}

#[async_trait]
pub trait GiveawaysRepository: Send + Sync {
    async fn create(&self, record: GiveawayRecord) -> Result<GiveawayRecord, Error>;
    async fn get_by_message(
        &self,
        guild_id: u64,
        message_id: u64,
    ) -> Result<Option<GiveawayRecord>, Error>;
    async fn save(&self, record: GiveawayRecord) -> Result<GiveawayRecord, Error>;
    async fn list_by_guild(&self, guild_id: u64) -> Result<Vec<GiveawayRecord>, Error>;
    async fn list_due_before(
        &self,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<GiveawayRecord>, Error>;
}

#[async_trait]
pub trait InviteRepository: Send + Sync {
    async fn get_or_create(
        &self,
        guild_id: u64,
        member_id: &str,
    ) -> Result<InviteMemberRecord, Error>;
    async fn save(&self, record: InviteMemberRecord) -> Result<InviteMemberRecord, Error>;
    async fn leaderboard(
        &self,
        guild_id: u64,
        limit: u32,
    ) -> Result<Vec<InviteLeaderboardEntry>, Error>;
}

#[async_trait]
pub trait MemberStatsRepository: Send + Sync {
    async fn get_or_create(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<MemberStatsRecord, Error>;
    async fn save(&self, record: MemberStatsRecord) -> Result<MemberStatsRecord, Error>;
}

#[async_trait]
pub trait WarningLogRepository: Send + Sync {
    async fn add(&self, record: WarningLogRecord) -> Result<WarningLogRecord, Error>;
    async fn list_for_member(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<Vec<WarningLogRecord>, Error>;
    async fn clear_for_member(&self, guild_id: u64, member_id: u64) -> Result<u64, Error>;
}

#[async_trait]
pub trait DashboardAuditLogRepository: Send + Sync {
    async fn append(&self, entry: DashboardAuditLogEntry) -> Result<DashboardAuditLogEntry, Error>;
    async fn list(&self, query: DashboardAuditLogQuery) -> Result<DashboardAuditLogPage, Error>;
}
