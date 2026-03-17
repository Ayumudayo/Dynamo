use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct GuildSettings {
    pub guild_id: u64,
    pub modules: BTreeMap<String, GuildModuleSettings>,
    pub commands: BTreeMap<String, GuildCommandSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GuildCommandSettings {
    pub enabled: bool,
    pub configuration: serde_json::Value,
}

impl Default for GuildCommandSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            configuration: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct DeploymentSettings {
    pub modules: BTreeMap<String, DeploymentModuleSettings>,
    pub commands: BTreeMap<String, DeploymentCommandSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DeploymentCommandSettings {
    pub installed: bool,
    pub enabled: bool,
    pub configuration: serde_json::Value,
}

impl Default for DeploymentCommandSettings {
    fn default() -> Self {
        Self {
            installed: true,
            enabled: true,
            configuration: serde_json::Value::Null,
        }
    }
}
