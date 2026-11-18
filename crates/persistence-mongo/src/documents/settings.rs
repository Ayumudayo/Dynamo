use std::collections::BTreeMap;

use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};
use serde::{Deserialize, Serialize};

use crate::{Error, config::DEPLOYMENT_SETTINGS_ID, documents::parse_snowflake};

#[cfg(test)]
use crate::ids::guild_document_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GuildSettingsDocument {
    #[serde(rename = "_id")]
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) announcement_channel_id: Option<String>,
    #[serde(default)]
    pub(crate) modules: BTreeMap<String, GuildModuleSettings>,
    #[serde(default)]
    pub(crate) commands: BTreeMap<String, GuildCommandSettings>,
}

impl GuildSettingsDocument {
    #[cfg(test)]
    pub(crate) fn default_for_guild(guild_id: u64) -> Self {
        Self {
            id: guild_document_id(guild_id),
            announcement_channel_id: None,
            modules: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }

    pub(crate) fn into_domain(self) -> Result<GuildSettings, Error> {
        Ok(GuildSettings {
            guild_id: self.id.parse::<u64>().map_err(|error| {
                anyhow::anyhow!("Stored guild settings id is not a valid u64: {error}")
            })?,
            announcement_channel_id: self
                .announcement_channel_id
                .map(|value| parse_snowflake(&value, "guild announcement channel id"))
                .transpose()?,
            modules: self.modules,
            commands: self.commands,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeploymentSettingsDocument {
    #[serde(rename = "_id")]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) blocked_guild_ids: Vec<String>,
    #[serde(default)]
    pub(crate) module_notices: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) modules: BTreeMap<String, DeploymentModuleSettings>,
    #[serde(default)]
    pub(crate) commands: BTreeMap<String, DeploymentCommandSettings>,
}

impl DeploymentSettingsDocument {
    pub(crate) fn default_document() -> Self {
        Self {
            id: DEPLOYMENT_SETTINGS_ID.to_string(),
            blocked_guild_ids: Vec::new(),
            module_notices: BTreeMap::new(),
            modules: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }

    pub(crate) fn into_domain(self) -> Result<DeploymentSettings, Error> {
        let mut blocked_guild_ids = Vec::with_capacity(self.blocked_guild_ids.len());
        for value in self.blocked_guild_ids {
            blocked_guild_ids.push(parse_snowflake(&value, "deployment blocked guild id")?);
        }
        Ok(DeploymentSettings {
            blocked_guild_ids,
            module_notices: self.module_notices,
            modules: self.modules,
            commands: self.commands,
        })
    }
}
