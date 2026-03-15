mod config;
mod enablement;
mod guard;
mod invite;
mod member_stats;
mod module;
mod registry;
mod repositories;
mod services;
mod settings;
mod suggestions;
mod warnings;

pub use config::{AppConfig, DiscordConfig};
pub use enablement::{ResolvedModuleState, resolve_module_state, resolve_module_states};
pub use guard::{ModuleAccess, module_access_for_context, module_access_for_state};
pub use invite::{InviteCounters, InviteLeaderboardEntry, InviteMemberRecord};
pub use member_stats::{
    CommandUsageStats, MemberStatsRecord, MessageContextUsageStats, VoiceStatsRecord,
};
pub use module::{
    Module, ModuleCatalog, ModuleCatalogEntry, ModuleCategory, ModuleDescriptor, ModuleManifest,
    SettingOption, SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
pub use registry::{AppState, ModuleRegistry, aggregate_intents};
pub use repositories::{
    DeploymentSettingsRepository, GuildSettingsRepository, Persistence, ProviderStateRepository,
    SuggestionsRepository, InviteRepository, MemberStatsRepository, WarningLogRepository,
};
pub use services::{ServiceRegistry, StockQuote, StockQuoteService};
pub use settings::{
    DeploymentModuleSettings, DeploymentSettings, GuildModuleSettings, GuildSettings,
};
pub use suggestions::{
    SuggestionRecord, SuggestionStats, SuggestionStatus, SuggestionStatusUpdate,
};
pub use warnings::WarningLogRecord;

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, AppState, Error>;
pub type DiscordCommand = poise::Command<AppState, Error>;
pub type GatewayIntents = poise::serenity_prelude::GatewayIntents;
