use std::sync::Arc;

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

#[derive(Clone, Default)]
pub struct Persistence {
    pub database_name: Option<String>,
    pub guild_settings: Option<Arc<dyn GuildSettingsRepository>>,
    pub deployment_settings: Option<Arc<dyn DeploymentSettingsRepository>>,
}

impl Persistence {
    pub fn new(
        database_name: Option<String>,
        guild_settings: Option<Arc<dyn GuildSettingsRepository>>,
        deployment_settings: Option<Arc<dyn DeploymentSettingsRepository>>,
    ) -> Self {
        Self {
            database_name,
            guild_settings,
            deployment_settings,
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
}
