use dynamo_runtime_api::{AppState, Error};
use dynamo_settings::GuildModuleSettings;
use serde::{Deserialize, Deserializer, Serialize};

use crate::constants::MODULE_ID;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct SuggestionSettings {
    #[serde(deserialize_with = "deserialize_optional_snowflake")]
    pub(crate) channel_id: Option<u64>,
    #[serde(
        alias = "approved_channel",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    pub(crate) approved_channel_id: Option<u64>,
    #[serde(
        alias = "rejected_channel",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    pub(crate) rejected_channel_id: Option<u64>,
    #[serde(alias = "staff_roles", deserialize_with = "deserialize_snowflake_vec")]
    pub(crate) staff_role_ids: Vec<u64>,
}

pub(crate) async fn load_settings(
    data: &AppState,
    guild_id: Option<u64>,
) -> Result<SuggestionSettings, Error> {
    let Some(guild_id) = guild_id else {
        return Ok(SuggestionSettings::default());
    };

    let guild_settings = data.persistence.guild_settings_or_default(guild_id).await?;
    guild_settings
        .modules
        .get(MODULE_ID)
        .map(parse_suggestion_settings)
        .transpose()
        .map(|settings| settings.unwrap_or_default())
}

fn parse_suggestion_settings(module: &GuildModuleSettings) -> Result<SuggestionSettings, Error> {
    Ok(serde_json::from_value::<SuggestionSettings>(
        module.configuration.clone(),
    )?)
}

fn deserialize_optional_snowflake<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    parse_optional_snowflake_value(value).map_err(serde::de::Error::custom)
}

fn deserialize_snowflake_vec<'de, D>(deserializer: D) -> Result<Vec<u64>, D::Error>
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
            .parse::<u64>()
            .map(Some)
            .map_err(|error| format!("invalid snowflake `{value}`: {error}")),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| "snowflake number must be an unsigned integer".to_string())
            .map(Some),
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
            .collect(),
        other => Err(format!(
            "snowflake array must be a string or array, got {other}"
        )),
    }
}
