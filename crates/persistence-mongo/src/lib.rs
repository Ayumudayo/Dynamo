use std::{collections::BTreeMap, env};

use async_trait::async_trait;
use dynamo_domain_giveaway::{GiveawayRecord, GiveawayStatus};
use dynamo_domain_invite::{InviteCounters, InviteLeaderboardEntry, InviteMemberRecord};
use dynamo_domain_moderation::WarningLogRecord;
use dynamo_domain_stats::{
    CommandUsageStats, MemberStatsRecord, MessageContextUsageStats, VoiceStatsRecord,
};
use dynamo_domain_suggestion::{
    SuggestionRecord, SuggestionStats, SuggestionStatus, SuggestionStatusUpdate,
};
use dynamo_ops::{
    DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry, DashboardAuditLogPage,
    DashboardAuditLogQuery, DashboardAuditLogRepository, DashboardAuditScope,
};
use dynamo_repositories::{
    DeploymentSettingsRepository, GiveawaysRepository, GuildSettingsRepository, InviteRepository,
    MemberStatsRepository, ProviderStateRepository, SuggestionsRepository, WarningLogRepository,
};
use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};
use futures_util::TryStreamExt;
use mongodb::{
    Client, Collection, Database,
    bson::{Bson, DateTime as BsonDateTime, Document, doc, from_bson, oid::ObjectId, to_bson},
    options::ReturnDocument,
};
use serde::{Deserialize, Serialize};

type Error = anyhow::Error;

const DEPLOYMENT_SETTINGS_ID: &str = "global";
pub const DEFAULT_DATABASE_NAME: &str = "dynamo-rs";

#[derive(Debug, Clone)]
pub struct MongoInitializationReport {
    pub database_name: String,
    pub existing_collections: Vec<String>,
    pub created_collections: Vec<String>,
    pub final_collections: Vec<String>,
    pub deployment_settings_seeded: bool,
}

#[derive(Debug, Clone)]
pub struct MongoPersistenceConfig {
    pub connection_string: String,
    pub database_name: String,
}

impl MongoPersistenceConfig {
    pub fn new(connection_string: impl Into<String>, database_name: impl Into<String>) -> Self {
        Self {
            connection_string: connection_string.into(),
            database_name: database_name.into(),
        }
    }

    pub fn from_env() -> Result<Self, Error> {
        let connection_string = env::var("MONGODB_URI")
            .or_else(|_| env::var("MONGO_CONNECTION"))
            .map_err(|_| anyhow::anyhow!("MONGODB_URI or MONGO_CONNECTION must be set"))?;
        let database_name =
            env::var("MONGODB_DATABASE").unwrap_or_else(|_| DEFAULT_DATABASE_NAME.to_string());

        Ok(Self::new(connection_string, database_name))
    }

    pub fn try_from_env() -> Result<Option<Self>, Error> {
        let connection_string =
            match env::var("MONGODB_URI").or_else(|_| env::var("MONGO_CONNECTION")) {
                Ok(value) => value,
                Err(env::VarError::NotPresent) => return Ok(None),
                Err(error) => {
                    return Err(anyhow::anyhow!(
                        "MongoDB connection environment could not be read: {error}"
                    ));
                }
            };
        let database_name =
            env::var("MONGODB_DATABASE").unwrap_or_else(|_| DEFAULT_DATABASE_NAME.to_string());

        Ok(Some(Self::new(connection_string, database_name)))
    }
}

#[derive(Clone)]
pub struct MongoPersistence {
    database: Database,
    guild_settings: Collection<GuildSettingsDocument>,
    deployment_settings: Collection<DeploymentSettingsDocument>,
    provider_state: Collection<ProviderStateDocument>,
    suggestions: Collection<SuggestionDocument>,
    giveaways: Collection<GiveawayDocument>,
    invite_members: Collection<InviteMemberDocument>,
    member_stats: Collection<MemberStatsDocument>,
    warning_logs: Collection<WarningLogDocument>,
    dashboard_audit_logs: Collection<DashboardAuditLogDocument>,
}

impl MongoPersistence {
    pub async fn connect(config: MongoPersistenceConfig) -> Result<Self, Error> {
        let client = Client::with_uri_str(&config.connection_string).await?;
        let database = client.database(&config.database_name);
        let guild_settings = database.collection::<GuildSettingsDocument>("guild_settings");
        let deployment_settings =
            database.collection::<DeploymentSettingsDocument>("deployment_settings");
        let provider_state = database.collection::<ProviderStateDocument>("provider_state");
        let suggestions = database.collection::<SuggestionDocument>("suggestions");
        let giveaways = database.collection::<GiveawayDocument>("giveaways");
        let invite_members = database.collection::<InviteMemberDocument>("members");
        let member_stats = database.collection::<MemberStatsDocument>("member-stats");
        let warning_logs = database.collection::<WarningLogDocument>("mod-logs");
        let dashboard_audit_logs =
            database.collection::<DashboardAuditLogDocument>("dashboard-audit-logs");

        Ok(Self {
            database,
            guild_settings,
            deployment_settings,
            provider_state,
            suggestions,
            giveaways,
            invite_members,
            member_stats,
            warning_logs,
            dashboard_audit_logs,
        })
    }

    pub async fn ensure_initialized_report(&self) -> Result<MongoInitializationReport, Error> {
        let existing_collections = self.database.list_collection_names().await?;
        let mut created_collections = Vec::new();

        if !existing_collections
            .iter()
            .any(|name| name == "guild_settings")
        {
            self.database.create_collection("guild_settings").await?;
            created_collections.push("guild_settings".to_string());
        }

        if !existing_collections
            .iter()
            .any(|name| name == "deployment_settings")
        {
            self.database
                .create_collection("deployment_settings")
                .await?;
            created_collections.push("deployment_settings".to_string());
        }

        if !existing_collections
            .iter()
            .any(|name| name == "provider_state")
        {
            self.database.create_collection("provider_state").await?;
            created_collections.push("provider_state".to_string());
        }

        if !existing_collections
            .iter()
            .any(|name| name == "suggestions")
        {
            self.database.create_collection("suggestions").await?;
            created_collections.push("suggestions".to_string());
        }

        if !existing_collections.iter().any(|name| name == "giveaways") {
            self.database.create_collection("giveaways").await?;
            created_collections.push("giveaways".to_string());
        }

        if !existing_collections.iter().any(|name| name == "members") {
            self.database.create_collection("members").await?;
            created_collections.push("members".to_string());
        }

        if !existing_collections
            .iter()
            .any(|name| name == "member-stats")
        {
            self.database.create_collection("member-stats").await?;
            created_collections.push("member-stats".to_string());
        }

        if !existing_collections.iter().any(|name| name == "mod-logs") {
            self.database.create_collection("mod-logs").await?;
            created_collections.push("mod-logs".to_string());
        }

        if !existing_collections
            .iter()
            .any(|name| name == "dashboard-audit-logs")
        {
            self.database
                .create_collection("dashboard-audit-logs")
                .await?;
            created_collections.push("dashboard-audit-logs".to_string());
        }

        let deployment_settings_result = self
            .deployment_settings
            .update_one(
                doc! { "_id": DEPLOYMENT_SETTINGS_ID },
                doc! {
                    "$setOnInsert": {
                        "_id": DEPLOYMENT_SETTINGS_ID,
                        "modules": {}
                    }
                },
            )
            .upsert(true)
            .await?;

        let final_collections = self.database.list_collection_names().await?;

        Ok(MongoInitializationReport {
            database_name: self.database.name().to_string(),
            existing_collections,
            created_collections,
            final_collections,
            deployment_settings_seeded: deployment_settings_result.upserted_id.is_some(),
        })
    }

    pub async fn ensure_initialized(&self) -> Result<(), Error> {
        self.ensure_initialized_report().await.map(|_| ())
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    fn guild_document_id(guild_id: u64) -> String {
        guild_id.to_string()
    }

    pub async fn load_provider_state(
        &self,
        provider_id: &str,
    ) -> Result<Option<serde_json::Value>, Error> {
        let document = self
            .provider_state
            .find_one(doc! { "_id": provider_id })
            .await?;
        let Some(document) = document else {
            return Ok(None);
        };

        Ok(Some(from_bson(document.state)?))
    }

    pub async fn save_provider_state(
        &self,
        provider_id: &str,
        state: serde_json::Value,
    ) -> Result<(), Error> {
        self.provider_state
            .update_one(
                doc! { "_id": provider_id },
                doc! {
                    "$setOnInsert": { "_id": provider_id },
                    "$set": {
                        "state": to_bson(&state)?,
                        "updated_at": BsonDateTime::now(),
                    },
                },
            )
            .upsert(true)
            .await?;

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GuildSettingsDocument {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    modules: BTreeMap<String, GuildModuleSettings>,
    #[serde(default)]
    commands: BTreeMap<String, GuildCommandSettings>,
}

impl GuildSettingsDocument {
    #[cfg(test)]
    fn default_for_guild(guild_id: u64) -> Self {
        Self {
            id: MongoPersistence::guild_document_id(guild_id),
            modules: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }

    fn into_domain(self) -> Result<GuildSettings, Error> {
        Ok(GuildSettings {
            guild_id: self.id.parse::<u64>().map_err(|error| {
                anyhow::anyhow!("Stored guild settings id is not a valid u64: {error}")
            })?,
            modules: self.modules,
            commands: self.commands,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeploymentSettingsDocument {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    modules: BTreeMap<String, DeploymentModuleSettings>,
    #[serde(default)]
    commands: BTreeMap<String, DeploymentCommandSettings>,
}

impl DeploymentSettingsDocument {
    fn default_document() -> Self {
        Self {
            id: DEPLOYMENT_SETTINGS_ID.to_string(),
            modules: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }

    fn into_domain(self) -> DeploymentSettings {
        DeploymentSettings {
            modules: self.modules,
            commands: self.commands,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderStateDocument {
    #[serde(rename = "_id")]
    id: String,
    state: mongodb::bson::Bson,
    #[serde(default)]
    updated_at: Option<BsonDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuggestionDocument {
    guild_id: String,
    channel_id: String,
    message_id: String,
    user_id: String,
    suggestion: String,
    status: SuggestionStatus,
    stats: SuggestionStats,
    #[serde(default)]
    status_updates: Vec<SuggestionStatusUpdateDocument>,
    created_at: BsonDateTime,
    updated_at: BsonDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GiveawayDocument {
    guild_id: String,
    channel_id: String,
    message_id: String,
    prize: String,
    winner_count: u64,
    host_user_id: String,
    #[serde(default)]
    allowed_role_ids: Vec<String>,
    #[serde(default)]
    entries: Vec<String>,
    #[serde(default)]
    winner_ids: Vec<String>,
    status: GiveawayStatus,
    started_at: BsonDateTime,
    ends_at: BsonDateTime,
    #[serde(default)]
    paused_at: Option<BsonDateTime>,
    button_label: String,
    created_at: BsonDateTime,
    updated_at: BsonDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuggestionStatusUpdateDocument {
    user_id: String,
    status: SuggestionStatus,
    #[serde(default)]
    reason: Option<String>,
    timestamp: BsonDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InviteMemberDocument {
    guild_id: String,
    member_id: String,
    #[serde(default)]
    invite_data: InviteCounters,
    created_at: BsonDateTime,
    updated_at: BsonDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemberStatsDocument {
    guild_id: String,
    member_id: String,
    messages: u64,
    voice: VoiceStatsRecord,
    commands: CommandUsageStats,
    contexts: MessageContextUsageStats,
    xp: u64,
    level: u32,
    created_at: BsonDateTime,
    updated_at: BsonDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WarningLogDocument {
    guild_id: String,
    member_id: String,
    reason: Option<String>,
    admin_id: String,
    admin_tag: String,
    created_at: BsonDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DashboardAuditLogDocument {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<ObjectId>,
    timestamp: BsonDateTime,
    actor_user_id: String,
    actor_username: String,
    scope: DashboardAuditScope,
    #[serde(default)]
    guild_id: Option<String>,
    entity_type: DashboardAuditEntityType,
    entity_id: String,
    action: DashboardAuditAction,
    summary: String,
}

impl SuggestionDocument {
    fn from_domain(value: SuggestionRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            channel_id: value.channel_id.to_string(),
            message_id: value.message_id.to_string(),
            user_id: value.user_id.to_string(),
            suggestion: value.suggestion,
            status: value.status,
            stats: value.stats,
            status_updates: value
                .status_updates
                .into_iter()
                .map(SuggestionStatusUpdateDocument::from_domain)
                .collect(),
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
            updated_at: BsonDateTime::from_millis(value.updated_at.timestamp_millis()),
        }
    }

    fn into_domain(self) -> Result<SuggestionRecord, Error> {
        Ok(SuggestionRecord {
            guild_id: parse_snowflake(&self.guild_id, "suggestion guild id")?,
            channel_id: parse_snowflake(&self.channel_id, "suggestion channel id")?,
            message_id: parse_snowflake(&self.message_id, "suggestion message id")?,
            user_id: parse_snowflake(&self.user_id, "suggestion user id")?,
            suggestion: self.suggestion,
            status: self.status,
            stats: self.stats,
            status_updates: self
                .status_updates
                .into_iter()
                .map(SuggestionStatusUpdateDocument::into_domain)
                .collect::<Result<Vec<_>, _>>()?,
            created_at: self.created_at.to_system_time().into(),
            updated_at: self.updated_at.to_system_time().into(),
        })
    }
}

impl GiveawayDocument {
    fn from_domain(value: GiveawayRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            channel_id: value.channel_id.to_string(),
            message_id: value.message_id.to_string(),
            prize: value.prize,
            winner_count: value.winner_count,
            host_user_id: value.host_user_id.to_string(),
            allowed_role_ids: value
                .allowed_role_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            entries: value.entries.into_iter().map(|id| id.to_string()).collect(),
            winner_ids: value
                .winner_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            status: value.status,
            started_at: BsonDateTime::from_millis(value.started_at.timestamp_millis()),
            ends_at: BsonDateTime::from_millis(value.ends_at.timestamp_millis()),
            paused_at: value
                .paused_at
                .map(|timestamp| BsonDateTime::from_millis(timestamp.timestamp_millis())),
            button_label: value.button_label,
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
            updated_at: BsonDateTime::from_millis(value.updated_at.timestamp_millis()),
        }
    }

    fn into_domain(self) -> Result<GiveawayRecord, Error> {
        Ok(GiveawayRecord {
            guild_id: parse_snowflake(&self.guild_id, "giveaway guild id")?,
            channel_id: parse_snowflake(&self.channel_id, "giveaway channel id")?,
            message_id: parse_snowflake(&self.message_id, "giveaway message id")?,
            prize: self.prize,
            winner_count: self.winner_count,
            host_user_id: parse_snowflake(&self.host_user_id, "giveaway host user id")?,
            allowed_role_ids: self
                .allowed_role_ids
                .into_iter()
                .map(|value| parse_snowflake(&value, "giveaway allowed role id"))
                .collect::<Result<Vec<_>, _>>()?,
            entries: self
                .entries
                .into_iter()
                .map(|value| parse_snowflake(&value, "giveaway entry user id"))
                .collect::<Result<Vec<_>, _>>()?,
            winner_ids: self
                .winner_ids
                .into_iter()
                .map(|value| parse_snowflake(&value, "giveaway winner user id"))
                .collect::<Result<Vec<_>, _>>()?,
            status: self.status,
            started_at: self.started_at.to_system_time().into(),
            ends_at: self.ends_at.to_system_time().into(),
            paused_at: self
                .paused_at
                .map(|timestamp| timestamp.to_system_time().into()),
            button_label: self.button_label,
            created_at: self.created_at.to_system_time().into(),
            updated_at: self.updated_at.to_system_time().into(),
        })
    }
}

impl SuggestionStatusUpdateDocument {
    fn from_domain(value: SuggestionStatusUpdate) -> Self {
        Self {
            user_id: value.user_id.to_string(),
            status: value.status,
            reason: value.reason,
            timestamp: BsonDateTime::from_millis(value.timestamp.timestamp_millis()),
        }
    }

    fn into_domain(self) -> Result<SuggestionStatusUpdate, Error> {
        Ok(SuggestionStatusUpdate {
            user_id: parse_snowflake(&self.user_id, "suggestion status update user id")?,
            status: self.status,
            reason: self.reason,
            timestamp: self.timestamp.to_system_time().into(),
        })
    }
}

impl InviteMemberDocument {
    fn from_domain(value: InviteMemberRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            member_id: value.member_id,
            invite_data: value.invite_data,
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
            updated_at: BsonDateTime::from_millis(value.updated_at.timestamp_millis()),
        }
    }

    fn into_domain(self) -> Result<InviteMemberRecord, Error> {
        Ok(InviteMemberRecord {
            guild_id: parse_snowflake(&self.guild_id, "invite member guild id")?,
            member_id: self.member_id,
            invite_data: self.invite_data,
            created_at: self.created_at.to_system_time().into(),
            updated_at: self.updated_at.to_system_time().into(),
        })
    }
}

impl MemberStatsDocument {
    fn from_domain(value: MemberStatsRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            member_id: value.member_id.to_string(),
            messages: value.messages,
            voice: value.voice,
            commands: value.commands,
            contexts: value.contexts,
            xp: value.xp,
            level: value.level,
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
            updated_at: BsonDateTime::from_millis(value.updated_at.timestamp_millis()),
        }
    }

    fn into_domain(self) -> Result<MemberStatsRecord, Error> {
        Ok(MemberStatsRecord {
            guild_id: parse_snowflake(&self.guild_id, "member stats guild id")?,
            member_id: parse_snowflake(&self.member_id, "member stats member id")?,
            messages: self.messages,
            voice: self.voice,
            commands: self.commands,
            contexts: self.contexts,
            xp: self.xp,
            level: self.level,
            created_at: self.created_at.to_system_time().into(),
            updated_at: self.updated_at.to_system_time().into(),
        })
    }
}

impl WarningLogDocument {
    fn from_domain(value: WarningLogRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            member_id: value.member_id.to_string(),
            reason: value.reason,
            admin_id: value.admin_id.to_string(),
            admin_tag: value.admin_tag,
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
        }
    }

    fn into_domain(self) -> Result<WarningLogRecord, Error> {
        Ok(WarningLogRecord {
            guild_id: parse_snowflake(&self.guild_id, "warning log guild id")?,
            member_id: parse_snowflake(&self.member_id, "warning log member id")?,
            reason: self.reason,
            admin_id: parse_snowflake(&self.admin_id, "warning log admin id")?,
            admin_tag: self.admin_tag,
            created_at: self.created_at.to_system_time().into(),
        })
    }
}

impl DashboardAuditLogDocument {
    fn from_domain(value: DashboardAuditLogEntry) -> Self {
        Self {
            id: value.id.and_then(|value| ObjectId::parse_str(&value).ok()),
            timestamp: BsonDateTime::from_millis(value.timestamp.timestamp_millis()),
            actor_user_id: value.actor_user_id.to_string(),
            actor_username: value.actor_username,
            scope: value.scope,
            guild_id: value.guild_id.map(|value| value.to_string()),
            entity_type: value.entity_type,
            entity_id: value.entity_id,
            action: value.action,
            summary: value.summary,
        }
    }

    fn into_domain(self) -> Result<DashboardAuditLogEntry, Error> {
        Ok(DashboardAuditLogEntry {
            id: self.id.map(|value| value.to_hex()),
            timestamp: self.timestamp.to_system_time().into(),
            actor_user_id: parse_snowflake(&self.actor_user_id, "dashboard audit actor user id")?,
            actor_username: self.actor_username,
            scope: self.scope,
            guild_id: self
                .guild_id
                .map(|value| parse_snowflake(&value, "dashboard audit guild id"))
                .transpose()?,
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            action: self.action,
            summary: self.summary,
        })
    }
}

fn parse_snowflake(value: &str, field_name: &str) -> Result<u64, Error> {
    value
        .parse()
        .map_err(|error| anyhow::anyhow!("Stored {field_name} is not a valid u64: {error}"))
}

fn settings_set_on_insert(document_id: &str) -> Document {
    doc! {
        "_id": document_id,
        "modules": {},
        "commands": {},
    }
}

fn settings_field_path(section: &str, id_kind: &str, id: &str) -> Result<String, Error> {
    if id.is_empty() {
        return Err(anyhow::anyhow!(
            "{id_kind} id cannot be empty for Mongo settings paths"
        ));
    }

    if id.contains('.') {
        return Err(anyhow::anyhow!(
            "{id_kind} id `{id}` cannot contain `.` for Mongo settings paths"
        ));
    }

    if id.starts_with('$') {
        return Err(anyhow::anyhow!(
            "{id_kind} id `{id}` cannot start with `$` for Mongo settings paths"
        ));
    }

    Ok(format!("{section}.{id}"))
}

fn settings_upsert_update(document_id: &str, settings_path: &str, settings: Bson) -> Document {
    doc! {
        "$setOnInsert": settings_set_on_insert(document_id),
        "$set": {
            settings_path: settings,
        },
    }
}

#[async_trait]
impl GuildSettingsRepository for MongoPersistence {
    async fn get_or_create(&self, guild_id: u64) -> Result<GuildSettings, Error> {
        let id = Self::guild_document_id(guild_id);
        let document = self
            .guild_settings
            .find_one_and_update(
                doc! { "_id": &id },
                doc! {
                    "$setOnInsert": settings_set_on_insert(&id),
                },
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| anyhow::anyhow!("guild settings upsert returned no document"))?;

        document.into_domain()
    }

    async fn upsert_module_settings(
        &self,
        guild_id: u64,
        module_id: &str,
        settings: GuildModuleSettings,
    ) -> Result<GuildSettings, Error> {
        let id = Self::guild_document_id(guild_id);
        let module_path = settings_field_path("modules", "module", module_id)?;
        let module_settings = to_bson(&settings)?;
        let document = self
            .guild_settings
            .find_one_and_update(
                doc! { "_id": &id },
                settings_upsert_update(&id, &module_path, module_settings),
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| anyhow::anyhow!("guild module settings upsert returned no document"))?;

        document.into_domain()
    }

    async fn upsert_command_settings(
        &self,
        guild_id: u64,
        command_id: &str,
        settings: GuildCommandSettings,
    ) -> Result<GuildSettings, Error> {
        let id = Self::guild_document_id(guild_id);
        let command_path = settings_field_path("commands", "command", command_id)?;
        let command_settings = to_bson(&settings)?;
        let document = self
            .guild_settings
            .find_one_and_update(
                doc! { "_id": &id },
                settings_upsert_update(&id, &command_path, command_settings),
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| anyhow::anyhow!("guild command settings upsert returned no document"))?;

        document.into_domain()
    }
}

#[async_trait]
impl DeploymentSettingsRepository for MongoPersistence {
    async fn get(&self) -> Result<DeploymentSettings, Error> {
        let document = self
            .deployment_settings
            .find_one(doc! { "_id": DEPLOYMENT_SETTINGS_ID })
            .await?;

        Ok(document
            .unwrap_or_else(DeploymentSettingsDocument::default_document)
            .into_domain())
    }

    async fn upsert_module_settings(
        &self,
        module_id: &str,
        settings: DeploymentModuleSettings,
    ) -> Result<DeploymentSettings, Error> {
        let module_path = settings_field_path("modules", "module", module_id)?;
        let module_settings = to_bson(&settings)?;
        let document = self
            .deployment_settings
            .find_one_and_update(
                doc! { "_id": DEPLOYMENT_SETTINGS_ID },
                settings_upsert_update(DEPLOYMENT_SETTINGS_ID, &module_path, module_settings),
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("deployment module settings upsert returned no document")
            })?;

        Ok(document.into_domain())
    }

    async fn upsert_command_settings(
        &self,
        command_id: &str,
        settings: DeploymentCommandSettings,
    ) -> Result<DeploymentSettings, Error> {
        let command_path = settings_field_path("commands", "command", command_id)?;
        let command_settings = to_bson(&settings)?;
        let document = self
            .deployment_settings
            .find_one_and_update(
                doc! { "_id": DEPLOYMENT_SETTINGS_ID },
                settings_upsert_update(DEPLOYMENT_SETTINGS_ID, &command_path, command_settings),
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("deployment command settings upsert returned no document")
            })?;

        Ok(document.into_domain())
    }
}

#[async_trait]
impl ProviderStateRepository for MongoPersistence {
    async fn load_json(&self, provider_id: &str) -> Result<Option<serde_json::Value>, Error> {
        self.load_provider_state(provider_id).await
    }

    async fn save_json(&self, provider_id: &str, value: serde_json::Value) -> Result<(), Error> {
        self.save_provider_state(provider_id, value).await
    }
}

#[async_trait]
impl SuggestionsRepository for MongoPersistence {
    async fn create(&self, record: SuggestionRecord) -> Result<SuggestionRecord, Error> {
        let document = SuggestionDocument::from_domain(record);
        self.suggestions.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn get_by_message(
        &self,
        guild_id: u64,
        message_id: u64,
    ) -> Result<Option<SuggestionRecord>, Error> {
        let document = self
            .suggestions
            .find_one(doc! {
                "guild_id": guild_id.to_string(),
                "message_id": message_id.to_string(),
            })
            .await?;

        document.map(SuggestionDocument::into_domain).transpose()
    }

    async fn save(&self, record: SuggestionRecord) -> Result<SuggestionRecord, Error> {
        let document = SuggestionDocument::from_domain(record);
        self.suggestions
            .replace_one(
                doc! {
                    "guild_id": &document.guild_id,
                    "message_id": &document.message_id,
                },
                document.clone(),
            )
            .upsert(true)
            .await?;

        document.into_domain()
    }
}

#[async_trait]
impl GiveawaysRepository for MongoPersistence {
    async fn create(&self, record: GiveawayRecord) -> Result<GiveawayRecord, Error> {
        let document = GiveawayDocument::from_domain(record);
        self.giveaways.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn get_by_message(
        &self,
        guild_id: u64,
        message_id: u64,
    ) -> Result<Option<GiveawayRecord>, Error> {
        let document = self
            .giveaways
            .find_one(doc! {
                "guild_id": guild_id.to_string(),
                "message_id": message_id.to_string(),
            })
            .await?;

        document.map(GiveawayDocument::into_domain).transpose()
    }

    async fn save(&self, record: GiveawayRecord) -> Result<GiveawayRecord, Error> {
        let document = GiveawayDocument::from_domain(record);
        self.giveaways
            .replace_one(
                doc! {
                    "guild_id": &document.guild_id,
                    "message_id": &document.message_id,
                },
                document.clone(),
            )
            .upsert(true)
            .await?;

        document.into_domain()
    }

    async fn list_by_guild(&self, guild_id: u64) -> Result<Vec<GiveawayRecord>, Error> {
        let mut cursor = self
            .giveaways
            .find(doc! { "guild_id": guild_id.to_string() })
            .await?;

        let mut records = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            records.push(document.into_domain()?);
        }
        Ok(records)
    }

    async fn list_due_before(
        &self,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<GiveawayRecord>, Error> {
        let mut cursor = self
            .giveaways
            .find(doc! {
                "status": "ACTIVE",
                "ends_at": { "$lte": BsonDateTime::from_millis(timestamp.timestamp_millis()) },
            })
            .await?;

        let mut records = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            records.push(document.into_domain()?);
        }
        Ok(records)
    }
}

#[async_trait]
impl InviteRepository for MongoPersistence {
    async fn get_or_create(
        &self,
        guild_id: u64,
        member_id: &str,
    ) -> Result<InviteMemberRecord, Error> {
        let document = self
            .invite_members
            .find_one(doc! {
                "guild_id": guild_id.to_string(),
                "member_id": member_id,
            })
            .await?;

        if let Some(document) = document {
            return document.into_domain();
        }

        let now = chrono::Utc::now();
        let record = InviteMemberRecord {
            guild_id,
            member_id: member_id.to_string(),
            invite_data: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let document = InviteMemberDocument::from_domain(record);
        self.invite_members.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn save(&self, record: InviteMemberRecord) -> Result<InviteMemberRecord, Error> {
        let document = InviteMemberDocument::from_domain(record);
        self.invite_members
            .replace_one(
                doc! {
                    "guild_id": &document.guild_id,
                    "member_id": &document.member_id,
                },
                document.clone(),
            )
            .upsert(true)
            .await?;
        document.into_domain()
    }

    async fn leaderboard(
        &self,
        guild_id: u64,
        limit: u32,
    ) -> Result<Vec<InviteLeaderboardEntry>, Error> {
        let pipeline = vec![
            doc! { "$match": { "guild_id": guild_id.to_string() } },
            doc! {
                "$project": {
                    "member_id": "$member_id",
                    "invites": {
                        "$subtract": [
                            { "$add": ["$invite_data.tracked", "$invite_data.added"] },
                            { "$add": ["$invite_data.left", "$invite_data.fake"] }
                        ]
                    }
                }
            },
            doc! { "$match": { "invites": { "$gt": 0 } } },
            doc! { "$sort": { "invites": -1 } },
            doc! { "$limit": limit as i64 },
        ];

        let mut cursor = self.invite_members.aggregate(pipeline).await?;
        let mut entries = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            let member_id = document
                .get_str("member_id")
                .map_err(|error| anyhow::anyhow!("invite leaderboard member_id missing: {error}"))?
                .to_string();
            let invites = document
                .get_i64("invites")
                .map_err(|error| anyhow::anyhow!("invite leaderboard invites missing: {error}"))?;
            entries.push(InviteLeaderboardEntry { member_id, invites });
        }
        Ok(entries)
    }
}

#[async_trait]
impl MemberStatsRepository for MongoPersistence {
    async fn get_or_create(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<MemberStatsRecord, Error> {
        let document = self
            .member_stats
            .find_one(doc! {
                "guild_id": guild_id.to_string(),
                "member_id": member_id.to_string(),
            })
            .await?;

        if let Some(document) = document {
            return document.into_domain();
        }

        let now = chrono::Utc::now();
        let record = MemberStatsRecord {
            guild_id,
            member_id,
            messages: 0,
            voice: Default::default(),
            commands: Default::default(),
            contexts: Default::default(),
            xp: 0,
            level: 1,
            created_at: now,
            updated_at: now,
        };
        let document = MemberStatsDocument::from_domain(record);
        self.member_stats.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn save(&self, record: MemberStatsRecord) -> Result<MemberStatsRecord, Error> {
        let document = MemberStatsDocument::from_domain(record);
        self.member_stats
            .replace_one(
                doc! {
                    "guild_id": &document.guild_id,
                    "member_id": &document.member_id,
                },
                document.clone(),
            )
            .upsert(true)
            .await?;
        document.into_domain()
    }
}

#[async_trait]
impl WarningLogRepository for MongoPersistence {
    async fn add(&self, record: WarningLogRecord) -> Result<WarningLogRecord, Error> {
        let document = WarningLogDocument::from_domain(record);
        self.warning_logs.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn list_for_member(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<Vec<WarningLogRecord>, Error> {
        let mut cursor = self
            .warning_logs
            .find(doc! {
                "guild_id": guild_id.to_string(),
                "member_id": member_id.to_string(),
            })
            .await?;

        let mut records = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            records.push(document.into_domain()?);
        }
        Ok(records)
    }

    async fn clear_for_member(&self, guild_id: u64, member_id: u64) -> Result<u64, Error> {
        let deleted = self
            .warning_logs
            .delete_many(doc! {
                "guild_id": guild_id.to_string(),
                "member_id": member_id.to_string(),
            })
            .await?;
        Ok(deleted.deleted_count)
    }
}

#[async_trait]
impl DashboardAuditLogRepository for MongoPersistence {
    async fn append(
        &self,
        record: DashboardAuditLogEntry,
    ) -> Result<DashboardAuditLogEntry, Error> {
        let mut document = DashboardAuditLogDocument::from_domain(record);
        let result = self
            .dashboard_audit_logs
            .insert_one(document.clone())
            .await?;
        document.id = result.inserted_id.as_object_id();
        document.into_domain()
    }

    async fn list(&self, query: DashboardAuditLogQuery) -> Result<DashboardAuditLogPage, Error> {
        let page = query.page.max(1);
        let page_size = query.page_size.clamp(1, 100);
        let skip = page.saturating_sub(1).saturating_mul(page_size);

        let mut filter = doc! {
            "scope": to_bson(&query.scope)?,
        };
        if let Some(guild_id) = query.guild_id {
            filter.insert("guild_id", guild_id.to_string());
        }
        if let Some(entity_type) = query.entity_type {
            filter.insert("entity_type", to_bson(&entity_type)?);
        }
        if let Some(action) = query.action {
            filter.insert("action", to_bson(&action)?);
        }

        let total = self
            .dashboard_audit_logs
            .count_documents(filter.clone())
            .await?;
        let mut cursor = self
            .dashboard_audit_logs
            .find(filter)
            .sort(doc! { "timestamp": -1, "_id": -1 })
            .skip(skip)
            .limit(page_size as i64)
            .await?;

        let mut entries = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            entries.push(document.into_domain()?);
        }

        Ok(DashboardAuditLogPage {
            entries,
            page,
            page_size,
            total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_DATABASE_NAME, DeploymentSettingsDocument, GuildSettingsDocument, MongoPersistence,
        MongoPersistenceConfig,
    };
    use crate::MongoInitializationReport;
    use dynamo_ops::DashboardAuditLogRepository;
    use dynamo_ops::{
        DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry,
        DashboardAuditLogQuery, DashboardAuditScope,
    };
    use dynamo_repositories::{DeploymentSettingsRepository, GuildSettingsRepository};
    use dynamo_settings::{
        DeploymentCommandSettings, DeploymentModuleSettings, GuildCommandSettings,
        GuildModuleSettings,
    };
    use mongodb::bson::{Bson, doc, to_bson};
    use serde_json::json;

    fn require_mongo_test_config(test_name: &str) -> anyhow::Result<MongoPersistenceConfig> {
        let _ = dotenvy::dotenv();
        MongoPersistenceConfig::try_from_env()?.ok_or_else(|| {
            anyhow::anyhow!(
                "{test_name} requires MongoDB test configuration; set MONGODB_URI or MONGO_CONNECTION"
            )
        })
    }

    fn isolated_mongo_test_config(
        base: &MongoPersistenceConfig,
        test_name: &str,
    ) -> MongoPersistenceConfig {
        let label: String = test_name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let database_name = format!(
            "dynamo_persistence_mongo_{label}_{}",
            chrono::Utc::now().timestamp_millis().unsigned_abs()
        );

        MongoPersistenceConfig::new(base.connection_string.clone(), database_name)
    }

    #[test]
    fn initialization_report_can_include_dashboard_audit_collection() {
        let report = MongoInitializationReport {
            database_name: DEFAULT_DATABASE_NAME.to_string(),
            existing_collections: vec!["guild_settings".to_string()],
            created_collections: vec!["dashboard-audit-logs".to_string()],
            final_collections: vec![
                "guild_settings".to_string(),
                "dashboard-audit-logs".to_string(),
            ],
            deployment_settings_seeded: false,
        };

        assert!(
            report
                .final_collections
                .iter()
                .any(|value| value == "dashboard-audit-logs")
        );
    }

    #[test]
    fn guild_default_document_initializes_required_fields() {
        let document = GuildSettingsDocument::default_for_guild(42);

        assert_eq!(document.id, "42");
        assert!(document.modules.is_empty());
        assert!(document.commands.is_empty());
    }

    #[test]
    fn guild_upsert_update_seeds_required_fields_on_insert() {
        let settings = GuildModuleSettings {
            enabled: false,
            configuration: json!({ "threshold": 7 }),
        };

        let update = super::settings_upsert_update(
            "42",
            "modules.stock",
            to_bson(&settings).expect("guild module settings serialize"),
        );

        assert_eq!(
            update,
            doc! {
                "$setOnInsert": {
                    "_id": "42",
                    "modules": {},
                    "commands": {},
                },
                "$set": {
                    "modules.stock": {
                        "enabled": false,
                        "configuration": { "threshold": 7i64 },
                    },
                },
            }
        );
    }

    #[test]
    fn deployment_upsert_update_seeds_required_fields_on_insert() {
        let settings = DeploymentModuleSettings {
            installed: false,
            enabled: true,
        };

        let update = super::settings_upsert_update(
            "global",
            "modules.stock",
            to_bson(&settings).expect("deployment module settings serialize"),
        );

        assert_eq!(
            update,
            doc! {
                "$setOnInsert": {
                    "_id": "global",
                    "modules": {},
                    "commands": {},
                },
                "$set": {
                    "modules.stock": {
                        "installed": false,
                        "enabled": true,
                    },
                },
            }
        );
    }

    #[test]
    fn deployment_default_document_initializes_required_fields() {
        let document = DeploymentSettingsDocument::default_document();

        assert_eq!(document.id, "global");
        assert_eq!(
            to_bson(&document.modules).ok(),
            Some(Bson::Document(doc! {}))
        );
        assert_eq!(
            to_bson(&document.commands).ok(),
            Some(Bson::Document(doc! {}))
        );
    }

    #[test]
    fn settings_field_path_accepts_real_style_ids() {
        assert_eq!(
            super::settings_field_path("modules", "module", "stock").unwrap(),
            "modules.stock"
        );
        assert_eq!(
            super::settings_field_path("commands", "command", "exchange::rate").unwrap(),
            "commands.exchange::rate"
        );
    }

    #[test]
    fn settings_field_path_rejects_invalid_ids() {
        let empty = super::settings_field_path("modules", "module", "").unwrap_err();
        assert!(
            empty
                .to_string()
                .contains("module id cannot be empty for Mongo settings paths")
        );

        let dotted = super::settings_field_path("modules", "module", "a.b").unwrap_err();
        assert!(
            dotted
                .to_string()
                .contains("module id `a.b` cannot contain `.` for Mongo settings paths")
        );

        let dollar = super::settings_field_path("commands", "command", "$bad").unwrap_err();
        assert!(
            dollar
                .to_string()
                .contains("command id `$bad` cannot start with `$` for Mongo settings paths")
        );
    }

    #[tokio::test]
    #[ignore = "requires MongoDB environment and persists test data"]
    async fn settings_round_trip_against_mongo() -> anyhow::Result<()> {
        let config = require_mongo_test_config("settings_round_trip_against_mongo")?;
        let guild_store = MongoPersistence::connect(isolated_mongo_test_config(
            &config,
            "settings_round_trip_guild",
        ))
        .await?;

        let marker = chrono::Utc::now().timestamp_millis().unsigned_abs();
        let created_guild_id = marker;
        let guild_module_guild_id = marker + 1;
        let guild_command_guild_id = marker + 2;
        let guild_module_id = format!("integration_guild_module_{marker}");
        let guild_command_id = format!("integration::guild::command::{marker}");

        let created =
            GuildSettingsRepository::get_or_create(&guild_store, created_guild_id).await?;
        assert_eq!(created.guild_id, created_guild_id);
        assert!(created.modules.is_empty());
        assert!(created.commands.is_empty());

        let guild_module_settings = GuildModuleSettings {
            enabled: false,
            configuration: json!({ "threshold": 7 }),
        };
        let guild_after_module = GuildSettingsRepository::upsert_module_settings(
            &guild_store,
            guild_module_guild_id,
            &guild_module_id,
            guild_module_settings.clone(),
        )
        .await?;
        assert_eq!(guild_after_module.guild_id, guild_module_guild_id);
        assert_eq!(
            guild_after_module.modules.get(&guild_module_id),
            Some(&guild_module_settings)
        );
        assert!(guild_after_module.commands.is_empty());

        let guild_module_settings_updated = GuildModuleSettings {
            enabled: true,
            configuration: json!({ "threshold": 11, "mode": "updated" }),
        };
        let guild_after_module_update = GuildSettingsRepository::upsert_module_settings(
            &guild_store,
            guild_module_guild_id,
            &guild_module_id,
            guild_module_settings_updated.clone(),
        )
        .await?;
        assert_eq!(
            guild_after_module_update.modules.get(&guild_module_id),
            Some(&guild_module_settings_updated)
        );
        assert_ne!(
            guild_after_module_update.modules.get(&guild_module_id),
            Some(&guild_module_settings)
        );

        let guild_command_settings = GuildCommandSettings {
            enabled: false,
            configuration: json!({ "mode": "strict" }),
        };
        let guild_after_command = GuildSettingsRepository::upsert_command_settings(
            &guild_store,
            guild_command_guild_id,
            &guild_command_id,
            guild_command_settings.clone(),
        )
        .await?;
        assert_eq!(guild_after_command.guild_id, guild_command_guild_id);
        assert_eq!(
            guild_after_command.commands.get(&guild_command_id),
            Some(&guild_command_settings)
        );
        assert!(guild_after_command.modules.is_empty());

        let guild_command_settings_updated = GuildCommandSettings {
            enabled: true,
            configuration: json!({ "mode": "relaxed", "version": 2 }),
        };
        let guild_after_command_update = GuildSettingsRepository::upsert_command_settings(
            &guild_store,
            guild_command_guild_id,
            &guild_command_id,
            guild_command_settings_updated.clone(),
        )
        .await?;
        assert_eq!(
            guild_after_command_update.commands.get(&guild_command_id),
            Some(&guild_command_settings_updated)
        );
        assert_ne!(
            guild_after_command_update.commands.get(&guild_command_id),
            Some(&guild_command_settings)
        );
        assert!(guild_after_command_update.modules.is_empty());

        let deployment_module_store = MongoPersistence::connect(isolated_mongo_test_config(
            &config,
            "settings_round_trip_deployment_module",
        ))
        .await?;
        let deployment_module_id = format!("integration_deployment_module_{marker}");

        let deployment_module_settings = DeploymentModuleSettings {
            installed: false,
            enabled: true,
        };
        let deployment_after_module = DeploymentSettingsRepository::upsert_module_settings(
            &deployment_module_store,
            &deployment_module_id,
            deployment_module_settings.clone(),
        )
        .await?;
        assert_eq!(
            deployment_after_module.modules.get(&deployment_module_id),
            Some(&deployment_module_settings)
        );

        let deployment_module_settings_updated = DeploymentModuleSettings {
            installed: true,
            enabled: false,
        };
        let deployment_after_module_update = DeploymentSettingsRepository::upsert_module_settings(
            &deployment_module_store,
            &deployment_module_id,
            deployment_module_settings_updated.clone(),
        )
        .await?;
        assert_eq!(
            deployment_after_module_update
                .modules
                .get(&deployment_module_id),
            Some(&deployment_module_settings_updated)
        );
        assert_ne!(
            deployment_after_module_update
                .modules
                .get(&deployment_module_id),
            Some(&deployment_module_settings)
        );

        let deployment_command_store = MongoPersistence::connect(isolated_mongo_test_config(
            &config,
            "settings_round_trip_deployment_command",
        ))
        .await?;
        let deployment_command_id = format!("integration::deployment::command::{marker}");
        let deployment_command_settings = DeploymentCommandSettings {
            installed: false,
            enabled: false,
            configuration: json!({ "mode": "dry-run" }),
        };
        let deployment_after_command = DeploymentSettingsRepository::upsert_command_settings(
            &deployment_command_store,
            &deployment_command_id,
            deployment_command_settings.clone(),
        )
        .await?;
        assert_eq!(
            deployment_after_command
                .commands
                .get(&deployment_command_id),
            Some(&deployment_command_settings)
        );
        assert!(deployment_after_command.modules.is_empty());

        let deployment_command_settings_updated = DeploymentCommandSettings {
            installed: true,
            enabled: true,
            configuration: json!({ "mode": "live", "version": 2 }),
        };
        let deployment_after_command_update =
            DeploymentSettingsRepository::upsert_command_settings(
                &deployment_command_store,
                &deployment_command_id,
                deployment_command_settings_updated.clone(),
            )
            .await?;
        assert_eq!(
            deployment_after_command_update
                .commands
                .get(&deployment_command_id),
            Some(&deployment_command_settings_updated)
        );
        assert_ne!(
            deployment_after_command_update
                .commands
                .get(&deployment_command_id),
            Some(&deployment_command_settings)
        );
        assert!(deployment_after_command_update.modules.is_empty());

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires MongoDB environment and persists test data"]
    async fn dashboard_audit_logs_round_trip_against_mongo() -> anyhow::Result<()> {
        let config = require_mongo_test_config("dashboard_audit_logs_round_trip_against_mongo")?;
        let store = MongoPersistence::connect(isolated_mongo_test_config(
            &config,
            "dashboard_audit_logs_round_trip",
        ))
        .await?;

        store.ensure_initialized().await?;
        let marker = format!("integration::{}", chrono::Utc::now().timestamp_millis());
        let entry = DashboardAuditLogEntry {
            id: None,
            timestamp: chrono::Utc::now(),
            actor_user_id: 1,
            actor_username: "integration-test".to_string(),
            scope: DashboardAuditScope::Guild,
            guild_id: Some(42),
            entity_type: DashboardAuditEntityType::Command,
            entity_id: marker.clone(),
            action: DashboardAuditAction::SaveSettings,
            summary: "Saved guild settings for command integration::test.".to_string(),
        };

        let saved = store.append(entry).await?;
        assert!(saved.id.is_some());

        let page = store
            .list(DashboardAuditLogQuery {
                scope: DashboardAuditScope::Guild,
                guild_id: Some(42),
                entity_type: Some(DashboardAuditEntityType::Command),
                action: Some(DashboardAuditAction::SaveSettings),
                page: 1,
                page_size: 10,
            })
            .await?;

        assert!(page.entries.iter().any(|row| row.entity_id == marker));
        Ok(())
    }
}
