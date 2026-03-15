use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

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

#[async_trait]
pub trait ProviderStateRepository: Send + Sync {
    async fn load_json(&self, provider_id: &str) -> Result<Option<Value>, Error>;
    async fn save_json(&self, provider_id: &str, value: Value) -> Result<(), Error>;
}

#[derive(Clone, Default)]
pub struct Persistence {
    pub database_name: Option<String>,
    pub guild_settings: Option<Arc<dyn GuildSettingsRepository>>,
    pub deployment_settings: Option<Arc<dyn DeploymentSettingsRepository>>,
    pub provider_state: Option<Arc<dyn ProviderStateRepository>>,
}

impl Persistence {
    pub fn new(
        database_name: Option<String>,
        guild_settings: Option<Arc<dyn GuildSettingsRepository>>,
        deployment_settings: Option<Arc<dyn DeploymentSettingsRepository>>,
        provider_state: Option<Arc<dyn ProviderStateRepository>>,
    ) -> Self {
        Self {
            database_name,
            guild_settings,
            deployment_settings,
            provider_state,
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
}
