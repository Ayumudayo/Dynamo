mod config;
mod enablement;
mod guard;
mod registry;

use std::sync::Arc;

use chrono::Utc;
use dynamo_contracts::{
    DashboardAuditLogRepository, DeploymentSettings, DeploymentSettingsRepository,
    ExchangeRateService, GiveawaysRepository, GuildSettings, GuildSettingsRepository,
    InviteRepository, MemberStatsRepository, ProviderStateRepository, StockQuoteService,
    SuggestionsRepository, WarningLogRepository,
};
use dynamo_domain_giveaway::GiveawayRecord;
use dynamo_domain_invite::InviteMemberRecord;
use dynamo_domain_stats::MemberStatsRecord;
use dynamo_domain_suggestion::SuggestionRecord;
use poise::Context as PoiseContext;
use serde::{Deserialize, Serialize};

pub use config::{AppConfig, CommandSyncConfig, DiscordConfig, OptionalModulesConfig};
pub use enablement::{
    ResolvedCommandState, ResolvedModuleState, resolve_command_state, resolve_command_states,
    resolve_module_state, resolve_module_states,
};
pub use guard::{
    CommandAccess, ModuleAccess, command_access_for_app, command_access_for_context,
    command_access_for_state, module_access_for_app, module_access_for_context,
    module_access_for_state,
};
pub use registry::{AppState, ModuleRegistry, aggregate_intents};

pub type Error = anyhow::Error;
pub type Context<'a> = PoiseContext<'a, AppState, Error>;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MusicBackendKind {
    #[default]
    Songbird,
    Lavalink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicBackendStatus {
    pub backend: MusicBackendKind,
    pub healthy: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MusicBackendConfig {
    pub backend: MusicBackendKind,
    pub default_source: String,
    pub auto_leave_seconds: u64,
    pub songbird_ytdlp_program: Option<String>,
    pub lavalink_host: Option<String>,
    pub lavalink_port: Option<u16>,
    pub lavalink_password: Option<String>,
    pub lavalink_secure: bool,
    pub lavalink_resume_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MusicTrack {
    pub title: String,
    pub url: Option<String>,
    pub duration_seconds: Option<u64>,
    pub requested_by: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MusicQueueSnapshot {
    pub backend: MusicBackendKind,
    pub connected: bool,
    pub voice_channel_id: Option<u64>,
    pub text_channel_id: Option<u64>,
    pub paused: bool,
    pub current: Option<MusicTrack>,
    pub upcoming: Vec<MusicTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicEnqueueResult {
    pub started_immediately: bool,
    pub track: MusicTrack,
    pub snapshot: MusicQueueSnapshot,
}

#[async_trait::async_trait]
pub trait MusicService: Send + Sync {
    async fn status(&self, config: &MusicBackendConfig) -> Result<MusicBackendStatus, Error>;
    async fn join(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        voice_channel_id: u64,
        text_channel_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn leave(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<(), Error>;
    async fn play(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        voice_channel_id: u64,
        text_channel_id: u64,
        query: &str,
        requested_by: &str,
        config: &MusicBackendConfig,
    ) -> Result<MusicEnqueueResult, Error>;
    async fn pause(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn resume(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn skip(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn stop(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn queue(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
}

#[derive(Clone, Default)]
pub struct ServiceRegistry {
    pub stock_quotes: Option<Arc<dyn StockQuoteService>>,
    pub exchange_rates: Option<Arc<dyn ExchangeRateService>>,
    pub music: Option<Arc<dyn MusicService>>,
}

impl ServiceRegistry {
    pub fn new(
        stock_quotes: Option<Arc<dyn StockQuoteService>>,
        exchange_rates: Option<Arc<dyn ExchangeRateService>>,
        music: Option<Arc<dyn MusicService>>,
    ) -> Self {
        Self {
            stock_quotes,
            exchange_rates,
            music,
        }
    }
}

#[derive(Clone, Default)]
pub struct Persistence {
    pub database_name: Option<String>,
    pub guild_settings: Option<Arc<dyn GuildSettingsRepository>>,
    pub deployment_settings: Option<Arc<dyn DeploymentSettingsRepository>>,
    pub provider_state: Option<Arc<dyn ProviderStateRepository>>,
    pub suggestions: Option<Arc<dyn SuggestionsRepository>>,
    pub giveaways: Option<Arc<dyn GiveawaysRepository>>,
    pub invites: Option<Arc<dyn InviteRepository>>,
    pub member_stats: Option<Arc<dyn MemberStatsRepository>>,
    pub warning_logs: Option<Arc<dyn WarningLogRepository>>,
    pub dashboard_audit_logs: Option<Arc<dyn DashboardAuditLogRepository>>,
}

impl Persistence {
    pub fn new(
        database_name: Option<String>,
        guild_settings: Option<Arc<dyn GuildSettingsRepository>>,
        deployment_settings: Option<Arc<dyn DeploymentSettingsRepository>>,
        provider_state: Option<Arc<dyn ProviderStateRepository>>,
        suggestions: Option<Arc<dyn SuggestionsRepository>>,
        giveaways: Option<Arc<dyn GiveawaysRepository>>,
        invites: Option<Arc<dyn InviteRepository>>,
        member_stats: Option<Arc<dyn MemberStatsRepository>>,
        warning_logs: Option<Arc<dyn WarningLogRepository>>,
        dashboard_audit_logs: Option<Arc<dyn DashboardAuditLogRepository>>,
    ) -> Self {
        Self {
            database_name,
            guild_settings,
            deployment_settings,
            provider_state,
            suggestions,
            giveaways,
            invites,
            member_stats,
            warning_logs,
            dashboard_audit_logs,
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
                commands: Default::default(),
            }),
        }
    }

    pub async fn load_provider_state(
        &self,
        provider_id: &str,
    ) -> Result<Option<serde_json::Value>, Error> {
        match &self.provider_state {
            Some(repo) => repo.load_json(provider_id).await,
            None => Ok(None),
        }
    }

    pub async fn save_provider_state(
        &self,
        provider_id: &str,
        value: serde_json::Value,
    ) -> Result<(), Error> {
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

    pub async fn get_giveaway_by_message(
        &self,
        guild_id: u64,
        message_id: u64,
    ) -> Result<Option<GiveawayRecord>, Error> {
        match &self.giveaways {
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
                created_at: Utc::now(),
                updated_at: Utc::now(),
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
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }),
        }
    }

    pub async fn append_dashboard_audit_log(
        &self,
        entry: dynamo_ops::DashboardAuditLogEntry,
    ) -> Result<Option<dynamo_ops::DashboardAuditLogEntry>, Error> {
        match &self.dashboard_audit_logs {
            Some(repo) => repo.append(entry).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn list_dashboard_audit_logs(
        &self,
        query: dynamo_ops::DashboardAuditLogQuery,
    ) -> Result<dynamo_ops::DashboardAuditLogPage, Error> {
        match &self.dashboard_audit_logs {
            Some(repo) => repo.list(query).await,
            None => Ok(dynamo_ops::DashboardAuditLogPage::empty(
                query.page,
                query.page_size,
            )),
        }
    }
}
