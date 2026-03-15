use async_trait::async_trait;

use crate::{
    Error,
    settings::{DeploymentModuleSettings, DeploymentSettings, GuildModuleSettings, GuildSettings},
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
