use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    Error,
    invite::{InviteLeaderboardEntry, InviteMemberRecord},
    member_stats::MemberStatsRecord,
    settings::{DeploymentModuleSettings, DeploymentSettings, GuildModuleSettings, GuildSettings},
    suggestions::SuggestionRecord,
    warnings::WarningLogRecord,
};

#[async_trait]
pub trait GuildSettingsRepository: Send + Sync {
    async fn get_or_create(&self, guild_id: u64) -> Result<GuildSettings, Error>;
    async fn upsert_module_settings(
        &self,
        guild_id: u64,
        module_id: &str,
        settings: GuildModuleSettings,
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

#[derive(Clone, Default)]
pub struct Persistence {
    pub database_name: Option<String>,
    pub guild_settings: Option<Arc<dyn GuildSettingsRepository>>,
    pub deployment_settings: Option<Arc<dyn DeploymentSettingsRepository>>,
    pub provider_state: Option<Arc<dyn ProviderStateRepository>>,
    pub suggestions: Option<Arc<dyn SuggestionsRepository>>,
    pub invites: Option<Arc<dyn InviteRepository>>,
    pub member_stats: Option<Arc<dyn MemberStatsRepository>>,
    pub warning_logs: Option<Arc<dyn WarningLogRepository>>,
}

impl Persistence {
    pub fn new(
        database_name: Option<String>,
        guild_settings: Option<Arc<dyn GuildSettingsRepository>>,
        deployment_settings: Option<Arc<dyn DeploymentSettingsRepository>>,
        provider_state: Option<Arc<dyn ProviderStateRepository>>,
        suggestions: Option<Arc<dyn SuggestionsRepository>>,
        invites: Option<Arc<dyn InviteRepository>>,
        member_stats: Option<Arc<dyn MemberStatsRepository>>,
        warning_logs: Option<Arc<dyn WarningLogRepository>>,
    ) -> Self {
        Self {
            database_name,
            guild_settings,
            deployment_settings,
            provider_state,
            suggestions,
            invites,
            member_stats,
            warning_logs,
        }
    }

    pub async fn deployment_settings_or_default(&self) -> Result<DeploymentSettings, Error> {
        match &self.deployment_settings {
            Some(repo) => repo.get().await,
            None => Ok(DeploymentSettings::default()),
        }
    }

    pub async fn guild_settings_or_default(&self, guild_id: u64) -> Result<GuildSettings, Error> {
        match &self.guild_settings {
            Some(repo) => repo.get_or_create(guild_id).await,
            None => Ok(GuildSettings {
                guild_id,
                modules: Default::default(),
            }),
        }
    }

    pub async fn load_provider_state(&self, provider_id: &str) -> Result<Option<Value>, Error> {
        match &self.provider_state {
            Some(repo) => repo.load_json(provider_id).await,
            None => Ok(None),
        }
    }

    pub async fn save_provider_state(&self, provider_id: &str, value: Value) -> Result<(), Error> {
        match &self.provider_state {
            Some(repo) => repo.save_json(provider_id, value).await,
            None => Ok(()),
        }
    }

    pub async fn get_suggestion_by_message(
        &self,
        guild_id: u64,
        message_id: u64,
    ) -> Result<Option<SuggestionRecord>, Error> {
        match &self.suggestions {
            Some(repo) => repo.get_by_message(guild_id, message_id).await,
            None => Ok(None),
        }
    }

    pub async fn invite_record_or_default(
        &self,
        guild_id: u64,
        member_id: &str,
    ) -> Result<InviteMemberRecord, Error> {
        match &self.invites {
            Some(repo) => repo.get_or_create(guild_id, member_id).await,
            None => Ok(InviteMemberRecord {
                guild_id,
                member_id: member_id.to_string(),
                invite_data: Default::default(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }),
        }
    }

    pub async fn member_stats_or_default(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<MemberStatsRecord, Error> {
        match &self.member_stats {
            Some(repo) => repo.get_or_create(guild_id, member_id).await,
            None => Ok(MemberStatsRecord {
                guild_id,
                member_id,
                messages: 0,
                voice: Default::default(),
                commands: Default::default(),
                contexts: Default::default(),
                xp: 0,
                level: 1,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }),
        }
    }
}
