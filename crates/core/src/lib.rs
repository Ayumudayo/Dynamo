mod config;
mod enablement;
mod module;
mod registry;
mod repositories;
mod services;
mod settings;

pub use config::{AppConfig, DiscordConfig};
pub use enablement::{ResolvedModuleState, resolve_module_states};
pub use module::{
    Module, ModuleCatalog, ModuleCatalogEntry, ModuleCategory, ModuleDescriptor, ModuleManifest,
    SettingOption, SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
pub use registry::{AppState, ModuleRegistry, aggregate_intents};
pub use repositories::{
    DeploymentSettingsRepository, GuildSettingsRepository, Persistence, ProviderStateRepository,
};
pub use services::{ServiceRegistry, StockQuote, StockQuoteService};
pub use settings::{
    DeploymentModuleSettings, DeploymentSettings, GuildModuleSettings, GuildSettings,
};

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, AppState, Error>;
pub type DiscordCommand = poise::Command<AppState, Error>;
pub type GatewayIntents = poise::serenity_prelude::GatewayIntents;
