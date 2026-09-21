use std::collections::BTreeMap;

use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};
use serde::{Deserialize, Serialize};

use crate::{Error, config::DEPLOYMENT_SETTINGS_ID};

#[cfg(test)]
use crate::ids::guild_document_id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GuildSettingsDocument {
    #[serde(rename = "_id")]
    pub(crate) id: String,
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
            modules: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }

    pub(crate) fn into_domain(self) -> Result<GuildSettings, Error> {
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
pub(crate) struct DeploymentSettingsDocument {
    #[serde(rename = "_id")]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) modules: BTreeMap<String, DeploymentModuleSettings>,
    #[serde(default)]
    pub(crate) commands: BTreeMap<String, DeploymentCommandSettings>,
}

impl DeploymentSettingsDocument {
    pub(crate) fn default_document() -> Self {
        Self {
            id: DEPLOYMENT_SETTINGS_ID.to_string(),
            modules: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }

    pub(crate) fn into_domain(self) -> DeploymentSettings {
        DeploymentSettings {
            modules: self.modules,
            commands: self.commands,
        }
    }
}
