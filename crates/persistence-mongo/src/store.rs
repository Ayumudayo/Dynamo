use mongodb::{Client, Collection, Database};

use crate::{
    Error, MongoPersistenceConfig,
    documents::{
        DashboardAuditLogDocument, DeploymentSettingsDocument, GiveawayDocument,
        GuildSettingsDocument, InviteMemberDocument, MemberStatsDocument, ProviderStateDocument,
        SuggestionDocument, WarningLogDocument,
    },
};

#[derive(Clone)]
pub struct MongoPersistence {
    pub(crate) database: Database,
    pub(crate) guild_settings: Collection<GuildSettingsDocument>,
    pub(crate) deployment_settings: Collection<DeploymentSettingsDocument>,
    pub(crate) provider_state: Collection<ProviderStateDocument>,
    pub(crate) suggestions: Collection<SuggestionDocument>,
    pub(crate) giveaways: Collection<GiveawayDocument>,
    pub(crate) invite_members: Collection<InviteMemberDocument>,
    pub(crate) member_stats: Collection<MemberStatsDocument>,
    pub(crate) warning_logs: Collection<WarningLogDocument>,
    pub(crate) dashboard_audit_logs: Collection<DashboardAuditLogDocument>,
}

impl MongoPersistence {
    pub async fn connect(config: MongoPersistenceConfig) -> Result<Self, Error> {
        let client = Client::with_uri_str(&config.connection_string).await?;
        Ok(Self::from_database(client.database(&config.database_name)))
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    pub(crate) fn from_database(database: Database) -> Self {
        Self {
            guild_settings: database.collection::<GuildSettingsDocument>("guild_settings"),
            deployment_settings: database
                .collection::<DeploymentSettingsDocument>("deployment_settings"),
            provider_state: database.collection::<ProviderStateDocument>("provider_state"),
            suggestions: database.collection::<SuggestionDocument>("suggestions"),
            giveaways: database.collection::<GiveawayDocument>("giveaways"),
            invite_members: database.collection::<InviteMemberDocument>("members"),
            member_stats: database.collection::<MemberStatsDocument>("member-stats"),
            warning_logs: database.collection::<WarningLogDocument>("mod-logs"),
            dashboard_audit_logs: database
                .collection::<DashboardAuditLogDocument>("dashboard-audit-logs"),
            database,
        }
    }
}
