use std::{collections::BTreeMap, env};

use async_trait::async_trait;
use dynamo_core::{
    DeploymentModuleSettings, DeploymentSettings, DeploymentSettingsRepository, Error,
    GuildModuleSettings, GuildSettings, GuildSettingsRepository, ProviderStateRepository,
    SuggestionRecord, SuggestionStats, SuggestionStatus, SuggestionStatusUpdate,
    SuggestionsRepository,
};
use mongodb::{
    Client, Collection, Database,
    bson::{DateTime as BsonDateTime, doc, from_bson, to_bson},
};
use serde::{Deserialize, Serialize};

const DEPLOYMENT_SETTINGS_ID: &str = "global";
pub const DEFAULT_DATABASE_NAME: &str = "dynamo-rs";

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

        Ok(Self {
            database,
            guild_settings,
            deployment_settings,
            provider_state,
            suggestions,
        })
    }

    pub async fn ensure_initialized(&self) -> Result<(), Error> {
        let existing_collections = self.database.list_collection_names().await?;

        if !existing_collections
            .iter()
            .any(|name| name == "guild_settings")
        {
            self.database.create_collection("guild_settings").await?;
        }

        if !existing_collections
            .iter()
            .any(|name| name == "deployment_settings")
        {
            self.database
                .create_collection("deployment_settings")
                .await?;
        }

        if !existing_collections
            .iter()
            .any(|name| name == "provider_state")
        {
            self.database.create_collection("provider_state").await?;
        }

        if !existing_collections
            .iter()
            .any(|name| name == "suggestions")
        {
            self.database.create_collection("suggestions").await?;
        }

        self.deployment_settings
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

        Ok(())
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
}

impl GuildSettingsDocument {
    fn from_domain(settings: GuildSettings) -> Self {
        Self {
            id: MongoPersistence::guild_document_id(settings.guild_id),
            modules: settings.modules,
        }
    }

    fn into_domain(self) -> Result<GuildSettings, Error> {
        Ok(GuildSettings {
            guild_id: self.id.parse::<u64>().map_err(|error| {
                anyhow::anyhow!("Stored guild settings id is not a valid u64: {error}")
            })?,
            modules: self.modules,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeploymentSettingsDocument {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    modules: BTreeMap<String, DeploymentModuleSettings>,
}

impl DeploymentSettingsDocument {
    fn default_document() -> Self {
        Self {
            id: DEPLOYMENT_SETTINGS_ID.to_string(),
            modules: BTreeMap::new(),
        }
    }

    fn into_domain(self) -> DeploymentSettings {
        DeploymentSettings {
            modules: self.modules,
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
struct SuggestionStatusUpdateDocument {
    user_id: String,
    status: SuggestionStatus,
    #[serde(default)]
    reason: Option<String>,
    timestamp: BsonDateTime,
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

fn parse_snowflake(value: &str, field_name: &str) -> Result<u64, Error> {
    value
        .parse()
        .map_err(|error| anyhow::anyhow!("Stored {field_name} is not a valid u64: {error}"))
}

#[async_trait]
impl GuildSettingsRepository for MongoPersistence {
    async fn get_or_create(&self, guild_id: u64) -> Result<GuildSettings, Error> {
        let id = Self::guild_document_id(guild_id);

        if let Some(document) = self.guild_settings.find_one(doc! { "_id": &id }).await? {
            return document.into_domain();
        }

        let settings = GuildSettings {
            guild_id,
            modules: BTreeMap::new(),
        };
        self.guild_settings
            .insert_one(GuildSettingsDocument::from_domain(settings.clone()))
            .await?;

        Ok(settings)
    }

    async fn upsert_module_settings(
        &self,
        guild_id: u64,
        module_id: &str,
        settings: GuildModuleSettings,
    ) -> Result<GuildSettings, Error> {
        let id = Self::guild_document_id(guild_id);
        let module_path = format!("modules.{module_id}");
        let module_settings = to_bson(&settings)?;

        self.guild_settings
            .update_one(
                doc! { "_id": &id },
                doc! {
                    "$setOnInsert": { "_id": &id },
                    "$set": { module_path: module_settings },
                },
            )
            .upsert(true)
            .await?;

        self.get_or_create(guild_id).await
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
        let module_path = format!("modules.{module_id}");
        let module_settings = to_bson(&settings)?;

        self.deployment_settings
            .update_one(
                doc! { "_id": DEPLOYMENT_SETTINGS_ID },
                doc! {
                    "$setOnInsert": { "_id": DEPLOYMENT_SETTINGS_ID },
                    "$set": { module_path: module_settings },
                },
            )
            .upsert(true)
            .await?;

        self.get().await
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
