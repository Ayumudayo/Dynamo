mod config;
mod currency;
mod dashboard_audit;
mod enablement;
mod giveaways;
mod guard;
mod invite;
mod member_stats;
mod module;
mod registry;
mod repositories;
mod services;
mod settings;
mod startup;
mod suggestions;
mod warnings;

pub use config::{AppConfig, CommandSyncConfig, DiscordConfig, OptionalModulesConfig};
pub use currency::{
    CACHED_EXCHANGE_CURRENCIES, CurrencySpec, cached_exchange_currencies, currency_display_label,
    supported_currency_specs,
};
pub use dashboard_audit::{
    DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry, DashboardAuditLogPage,
    DashboardAuditLogQuery, DashboardAuditScope,
};
pub use enablement::{
    ResolvedCommandState, ResolvedModuleState, resolve_command_state, resolve_command_states,
    resolve_module_state, resolve_module_states,
};
pub use giveaways::{GiveawayRecord, GiveawayStatus};
pub use guard::{
    CommandAccess, ModuleAccess, command_access_for_app, command_access_for_context,
    command_access_for_state, module_access_for_app, module_access_for_context,
    module_access_for_state,
};
pub use invite::{InviteCounters, InviteLeaderboardEntry, InviteMemberRecord};
pub use member_stats::{
    CommandUsageStats, MemberStatsRecord, MessageContextUsageStats, VoiceStatsRecord,
};
pub use module::{
    CommandCatalog, CommandCatalogEntry, CommandDescriptor, Module, ModuleCatalog,
    ModuleCatalogEntry, ModuleCategory, ModuleDescriptor, ModuleManifest, SettingOption,
    SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
pub use registry::{AppState, ModuleRegistry, aggregate_intents};
pub use repositories::{
    DashboardAuditLogRepository, DeploymentSettingsRepository, GiveawaysRepository,
    GuildSettingsRepository, InviteRepository, MemberStatsRepository, Persistence,
    ProviderStateRepository, SuggestionsRepository, WarningLogRepository,
};
pub use services::{
    ExchangeRateCacheStatus, ExchangeRateQuote, ExchangeRateRefreshResult, ExchangeRateService,
    ExchangeRateSourceKind, MusicBackendConfig, MusicBackendKind, MusicBackendStatus,
    MusicEnqueueResult, MusicQueueSnapshot, MusicService, MusicTrack, ServiceRegistry, StockQuote,
    StockQuoteService,
};
pub use settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};
pub use startup::{
    CatalogStartupSummary, ScopeStartupSummary, StartupPhase, StartupReport, StartupStatus,
    catalog_startup_summary, format_gateway_intents, format_kv_list, format_preview_kv_list,
    format_preview_list, scope_startup_summary,
};
pub use suggestions::{
    SuggestionRecord, SuggestionStats, SuggestionStatus, SuggestionStatusUpdate,
};
pub use warnings::WarningLogRecord;

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, AppState, Error>;
pub type DiscordCommand = poise::Command<AppState, Error>;
pub type GatewayIntents = poise::serenity_prelude::GatewayIntents;
