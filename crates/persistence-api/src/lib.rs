use std::sync::Arc;

use chrono::Utc;
use dynamo_domain_giveaway::GiveawayRecord;
use dynamo_domain_invite::InviteMemberRecord;
use dynamo_domain_stats::MemberStatsRecord;
use dynamo_domain_suggestion::SuggestionRecord;
use dynamo_ops::{
    DashboardAuditLogEntry, DashboardAuditLogPage, DashboardAuditLogQuery,
    DashboardAuditLogRepository,
};
use dynamo_repositories::{
    DeploymentSettingsRepository, GiveawaysRepository, GuildSettingsRepository, InviteRepository,
    MemberStatsRepository, ProviderStateRepository, SuggestionsRepository, WarningLogRepository,
};
use dynamo_settings::{DeploymentSettings, GuildSettings};

pub type Error = anyhow::Error;

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
        Ok(self
            .guild_settings(guild_id)
            .await?
            .unwrap_or_else(|| GuildSettings::for_guild(guild_id)))
    }

    pub async fn guild_settings(&self, guild_id: u64) -> Result<Option<GuildSettings>, Error> {
        match &self.guild_settings {
            Some(repo) => repo.get(guild_id).await,
            None => Ok(None),
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use dynamo_repositories::GuildSettingsRepository;
    use dynamo_settings::{GuildCommandSettings, GuildModuleSettings, GuildSettings};

    use super::Persistence;

    enum ReadResult {
        Absent,
        Existing(GuildSettings),
        Unavailable,
    }

    struct FakeGuildSettingsRepository {
        result: ReadResult,
        reads: AtomicUsize,
        writes: AtomicUsize,
    }

    impl FakeGuildSettingsRepository {
        fn new(result: ReadResult) -> Self {
            Self {
                result,
                reads: AtomicUsize::new(0),
                writes: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl GuildSettingsRepository for FakeGuildSettingsRepository {
        async fn get(&self, _guild_id: u64) -> anyhow::Result<Option<GuildSettings>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match &self.result {
                ReadResult::Absent => Ok(None),
                ReadResult::Existing(settings) => Ok(Some(settings.clone())),
                ReadResult::Unavailable => anyhow::bail!("fake repository unavailable"),
            }
        }

        async fn upsert_module_settings(
            &self,
            _guild_id: u64,
            _module_id: &str,
            _settings: GuildModuleSettings,
        ) -> anyhow::Result<GuildSettings> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("unexpected fake repository write")
        }

        async fn upsert_command_settings(
            &self,
            _guild_id: u64,
            _command_id: &str,
            _settings: GuildCommandSettings,
        ) -> anyhow::Result<GuildSettings> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("unexpected fake repository write")
        }
    }

    fn persistence_with(repository: Arc<FakeGuildSettingsRepository>) -> Persistence {
        Persistence {
            guild_settings: Some(repository),
            ..Persistence::default()
        }
    }

    #[tokio::test]
    async fn absent_guild_read_returns_none_and_compatibility_default_without_writing() {
        let repository = Arc::new(FakeGuildSettingsRepository::new(ReadResult::Absent));
        let persistence = persistence_with(repository.clone());

        assert_eq!(persistence.guild_settings(42).await.unwrap(), None);
        assert_eq!(
            persistence.guild_settings_or_default(42).await.unwrap(),
            GuildSettings::for_guild(42)
        );
        assert_eq!(repository.reads.load(Ordering::SeqCst), 2);
        assert_eq!(repository.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn existing_guild_read_returns_stored_settings_without_writing() {
        let expected = GuildSettings::for_guild(42);
        let repository = Arc::new(FakeGuildSettingsRepository::new(ReadResult::Existing(
            expected.clone(),
        )));
        let persistence = persistence_with(repository.clone());

        assert_eq!(
            persistence.guild_settings(42).await.unwrap(),
            Some(expected)
        );
        assert_eq!(repository.reads.load(Ordering::SeqCst), 1);
        assert_eq!(repository.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unavailable_guild_read_is_not_converted_to_absent_or_default() {
        let repository = Arc::new(FakeGuildSettingsRepository::new(ReadResult::Unavailable));
        let persistence = persistence_with(repository.clone());

        assert!(persistence.guild_settings(42).await.is_err());
        assert!(persistence.guild_settings_or_default(42).await.is_err());
        assert_eq!(repository.reads.load(Ordering::SeqCst), 2);
        assert_eq!(repository.writes.load(Ordering::SeqCst), 0);
    }
}
