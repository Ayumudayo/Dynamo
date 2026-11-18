use dynamo_runtime_api::{
    AppState, Error, decode_module_configuration, deserialize_optional_snowflake,
    deserialize_snowflake_vec,
};
use dynamo_settings::GuildModuleSettings;
use serde::{Deserialize, Serialize};

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

    let guild_settings = data.guild_settings_cached(guild_id).await?;
    guild_settings
        .modules
        .get(MODULE_ID)
        .map(parse_suggestion_settings)
        .transpose()
        .map(|settings| settings.unwrap_or_default())
}

fn parse_suggestion_settings(module: &GuildModuleSettings) -> Result<SuggestionSettings, Error> {
    Ok(decode_module_configuration(&module.configuration)?)
}
