use std::sync::Arc;

use anyhow::ensure;
use dynamo_ops::{
    DashboardAuditLogEntry, DashboardAuditLogPage, DashboardAuditLogQuery,
    DashboardAuditLogRepository,
};
use dynamo_repositories::{
    DeploymentSettingsRepository, GuildSettingsRepository, ProviderStateRepository,
};
use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};

use super::{fixture::FixtureData, runtime::PerfRuntime};

#[derive(Clone)]
pub(super) struct PerfRepositories {
    runtime: Arc<PerfRuntime>,
    deployment: DeploymentSettings,
    guild: GuildSettings,
}

impl PerfRepositories {
    pub(super) fn new(runtime: Arc<PerfRuntime>, fixture: &FixtureData) -> Self {
        Self {
            runtime,
            deployment: DeploymentSettings {
                modules: fixture.settings.deployment.modules.clone(),
                commands: fixture.settings.deployment.commands.clone(),
            },
            guild: GuildSettings {
                guild_id: fixture.guild_id(),
                modules: fixture.settings.guild.modules.clone(),
                commands: fixture.settings.guild.commands.clone(),
            },
        }
    }

    fn deny_mutation<T>(&self) -> anyhow::Result<T> {
        self.runtime.increment_repository_mutations();
        anyhow::bail!("performance harness repository mutation denied")
    }
}

#[async_trait::async_trait]
impl DeploymentSettingsRepository for PerfRepositories {
    async fn get(&self) -> anyhow::Result<DeploymentSettings> {
        self.runtime.increment_repository_reads();
        Ok(self.deployment.clone())
    }

    async fn upsert_module_settings(
        &self,
        _module_id: &str,
        _settings: DeploymentModuleSettings,
    ) -> anyhow::Result<DeploymentSettings> {
        self.deny_mutation()
    }

    async fn upsert_command_settings(
        &self,
        _command_id: &str,
        _settings: DeploymentCommandSettings,
    ) -> anyhow::Result<DeploymentSettings> {
        self.deny_mutation()
    }
}

#[async_trait::async_trait]
impl GuildSettingsRepository for PerfRepositories {
    async fn get(&self, guild_id: u64) -> anyhow::Result<Option<GuildSettings>> {
        self.runtime.increment_repository_reads();
        ensure!(
            guild_id == self.guild.guild_id,
            "performance fixture repository only contains the target guild"
        );
        Ok(Some(self.guild.clone()))
    }

    async fn upsert_module_settings(
        &self,
        _guild_id: u64,
        _module_id: &str,
        _settings: GuildModuleSettings,
    ) -> anyhow::Result<GuildSettings> {
        self.deny_mutation()
    }

    async fn upsert_command_settings(
        &self,
        _guild_id: u64,
        _command_id: &str,
        _settings: GuildCommandSettings,
    ) -> anyhow::Result<GuildSettings> {
        self.deny_mutation()
    }
}

#[async_trait::async_trait]
impl ProviderStateRepository for PerfRepositories {
    async fn load_json(&self, _provider_id: &str) -> anyhow::Result<Option<serde_json::Value>> {
        self.runtime.increment_repository_reads();
        Ok(None)
    }

    async fn save_json(&self, _provider_id: &str, _value: serde_json::Value) -> anyhow::Result<()> {
        self.deny_mutation()
    }
}

#[async_trait::async_trait]
impl DashboardAuditLogRepository for PerfRepositories {
    async fn append(
        &self,
        _entry: DashboardAuditLogEntry,
    ) -> anyhow::Result<DashboardAuditLogEntry> {
        self.deny_mutation()
    }

    async fn list(&self, query: DashboardAuditLogQuery) -> anyhow::Result<DashboardAuditLogPage> {
        self.runtime.increment_repository_reads();
        Ok(DashboardAuditLogPage::empty(query.page, query.page_size))
    }
}
