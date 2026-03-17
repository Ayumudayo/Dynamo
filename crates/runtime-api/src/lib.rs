use std::sync::Arc;

use chrono::Utc;
use dynamo_domain_giveaway::GiveawayRecord;
use dynamo_domain_invite::InviteMemberRecord;
use dynamo_domain_stats::MemberStatsRecord;
use dynamo_domain_suggestion::SuggestionRecord;
use dynamo_module_kit::{CommandCatalog, ModuleCatalog};
use dynamo_ops::{
    DashboardAuditLogEntry, DashboardAuditLogPage, DashboardAuditLogQuery,
    DashboardAuditLogRepository,
};
use dynamo_repositories::{
    DeploymentSettingsRepository, GiveawaysRepository, GuildSettingsRepository, InviteRepository,
    MemberStatsRepository, ProviderStateRepository, SuggestionsRepository, WarningLogRepository,
};
use dynamo_service_exchange::ExchangeRateService;
use dynamo_service_stock::StockQuoteService;
use dynamo_settings::{DeploymentSettings, GuildSettings};
use poise::Context as PoiseContext;

pub type Error = anyhow::Error;
pub type Context<'a> = PoiseContext<'a, AppState, Error>;

#[derive(Clone, Default)]
pub struct ServiceRegistry {
    pub stock_quotes: Option<Arc<dyn StockQuoteService>>,
    pub exchange_rates: Option<Arc<dyn ExchangeRateService>>,
}

impl ServiceRegistry {
    pub fn new(
        stock_quotes: Option<Arc<dyn StockQuoteService>>,
        exchange_rates: Option<Arc<dyn ExchangeRateService>>,
    ) -> Self {
        Self {
            stock_quotes,
            exchange_rates,
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
    #[allow(clippy::too_many_arguments)]
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
        entry: DashboardAuditLogEntry,
    ) -> Result<Option<DashboardAuditLogEntry>, Error> {
        match &self.dashboard_audit_logs {
            Some(repo) => repo.append(entry).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn list_dashboard_audit_logs(
        &self,
        query: DashboardAuditLogQuery,
    ) -> Result<DashboardAuditLogPage, Error> {
        match &self.dashboard_audit_logs {
            Some(repo) => repo.list(query).await,
            None => Ok(DashboardAuditLogPage::empty(query.page, query.page_size)),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub started_at: std::time::Instant,
    pub module_catalog: ModuleCatalog,
    pub command_catalog: CommandCatalog,
    pub persistence: Persistence,
    pub services: ServiceRegistry,
}

impl AppState {
    pub fn new(
        module_catalog: ModuleCatalog,
        command_catalog: CommandCatalog,
        persistence: Persistence,
        services: ServiceRegistry,
    ) -> Self {
        Self {
            started_at: std::time::Instant::now(),
            module_catalog,
            command_catalog,
            persistence,
            services,
        }
    }
}
