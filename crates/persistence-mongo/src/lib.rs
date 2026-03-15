use std::{collections::BTreeMap, env};

use async_trait::async_trait;
use dynamo_core::{
    DeploymentModuleSettings, DeploymentSettings, DeploymentSettingsRepository, Error,
    GuildModuleSettings, GuildSettings, GuildSettingsRepository,
};
use mongodb::{
    Client, Collection, Database,
    bson::{doc, to_bson},
};
use serde::{Deserialize, Serialize};

const DEPLOYMENT_SETTINGS_ID: &str = "global";

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
            env::var("MONGODB_DATABASE").unwrap_or_else(|_| "dynamo_rust".to_string());

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
            env::var("MONGODB_DATABASE").unwrap_or_else(|_| "dynamo_rust".to_string());

        Ok(Some(Self::new(connection_string, database_name)))
    }
}

#[derive(Clone)]
pub struct MongoPersistence {
    database: Database,
    guild_settings: Collection<GuildSettingsDocument>,
    deployment_settings: Collection<DeploymentSettingsDocument>,
}

impl MongoPersistence {
    pub async fn connect(config: MongoPersistenceConfig) -> Result<Self, Error> {
        let client = Client::with_uri_str(&config.connection_string).await?;
        let database = client.database(&config.database_name);
        let guild_settings = database.collection::<GuildSettingsDocument>("guild_settings");
        let deployment_settings =
            database.collection::<DeploymentSettingsDocument>("deployment_settings");

        Ok(Self {
            database,
            guild_settings,
            deployment_settings,
        })
    }

    pub fn database(&self) -> &Database {
        &self.database
    }

    fn guild_document_id(guild_id: u64) -> String {
        guild_id.to_string()
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
