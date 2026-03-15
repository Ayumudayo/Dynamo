use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GuildSettings {
    pub guild_id: u64,
    pub modules: BTreeMap<String, GuildModuleSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GuildModuleSettings {
    pub enabled: bool,
    pub configuration: serde_json::Value,
}

impl Default for GuildModuleSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            configuration: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DeploymentSettings {
    pub modules: BTreeMap<String, DeploymentModuleSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeploymentModuleSettings {
    pub installed: bool,
    pub enabled: bool,
}

impl Default for DeploymentModuleSettings {
    fn default() -> Self {
        Self {
            installed: true,
            enabled: true,
        }
    }
}
