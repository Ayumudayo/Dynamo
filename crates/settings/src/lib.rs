use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};

pub fn decode_module_configuration<T: DeserializeOwned + Default>(
    value: &serde_json::Value,
) -> Result<T, serde_json::Error> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value.clone())
}

pub fn serialize_optional_snowflake<S>(
    value: &Option<u64>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value.filter(|id| *id > 0) {
        Some(id) => serializer.serialize_some(&id.to_string()),
        None => serializer.serialize_none(),
    }
}

pub fn serialize_snowflake_vec<S>(values: &[u64], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let strings: Vec<String> = values
        .iter()
        .copied()
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
        .collect();
    strings.serialize(serializer)
}

pub fn deserialize_optional_snowflake<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    parse_optional_snowflake_value(value).map_err(serde::de::Error::custom)
}

pub fn deserialize_snowflake_vec<'de, D>(deserializer: D) -> Result<Vec<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    parse_snowflake_vec_value(value).map_err(serde::de::Error::custom)
}

fn parse_optional_snowflake_value(value: Option<serde_json::Value>) -> Result<Option<u64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };

    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(value) if value.trim().is_empty() => Ok(None),
        serde_json::Value::String(value) => value
            .trim()
            .parse::<u64>()
            .map(|id| (id > 0).then_some(id))
            .map_err(|error| format!("invalid snowflake `{value}`: {error}")),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| "snowflake number must be an unsigned integer".to_string())
            .map(|id| (id > 0).then_some(id)),
        other => Err(format!("snowflake must be a string or number, got {other}")),
    }
}

fn parse_snowflake_vec_value(value: Option<serde_json::Value>) -> Result<Vec<u64>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| parse_optional_snowflake_value(Some(value)))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.into_iter().flatten().collect()),
        serde_json::Value::String(values) if values.trim().is_empty() => Ok(Vec::new()),
        serde_json::Value::String(values) => values
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid snowflake `{value}`: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|ids| ids.into_iter().filter(|id| *id > 0).collect()),
        other => Err(format!(
            "snowflake array must be a string or array, got {other}"
        )),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct GuildSettings {
    pub guild_id: u64,
    #[serde(default)]
    pub announcement_channel_id: Option<u64>,
    pub modules: BTreeMap<String, GuildModuleSettings>,
    pub commands: BTreeMap<String, GuildCommandSettings>,
}

impl GuildSettings {
    pub fn for_guild(guild_id: u64) -> Self {
        Self {
            guild_id,
            ..Self::default()
        }
    }
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
    #[serde(default)]
    pub blocked_guild_ids: Vec<u64>,
    #[serde(default)]
    pub module_notices: BTreeMap<String, String>,
    pub modules: BTreeMap<String, DeploymentModuleSettings>,
    pub commands: BTreeMap<String, DeploymentCommandSettings>,
}

#[cfg(test)]
mod tests {
    use super::{DeploymentSettings, GuildSettings};

    #[test]
    fn settings_deserialize_with_default_operational_fields() {
        let guild: GuildSettings =
            serde_json::from_str(r#"{"guild_id":42}"#).expect("guild settings deserialize");
        assert_eq!(guild.guild_id, 42);
        assert_eq!(guild.announcement_channel_id, None);

        let deployment: DeploymentSettings =
            serde_json::from_str("{}").expect("deployment settings deserialize");
        assert!(deployment.blocked_guild_ids.is_empty());
        assert!(deployment.module_notices.is_empty());
    }

    #[test]
    fn settings_round_trip_new_operational_fields() {
        let mut guild = GuildSettings::for_guild(99);
        guild.announcement_channel_id = Some(123_456_789);
        let guild_json = serde_json::to_string(&guild).expect("serialize guild");
        let parsed_guild: GuildSettings =
            serde_json::from_str(&guild_json).expect("deserialize guild");
        assert_eq!(parsed_guild, guild);

        let mut deployment = DeploymentSettings::default();
        deployment.blocked_guild_ids.push(777);
        deployment
            .module_notices
            .insert("global".to_string(), "Maintenance at 02:00 UTC".to_string());
        let dep_json = serde_json::to_string(&deployment).expect("serialize deployment");
        let parsed_dep: DeploymentSettings =
            serde_json::from_str(&dep_json).expect("deserialize deployment");
        assert_eq!(parsed_dep, deployment);
    }
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
