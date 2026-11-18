use dynamo_runtime_api::{
    AppState, Error, decode_module_configuration, deserialize_optional_snowflake,
};
use serde::{Deserialize, Serialize};

use crate::constants::{DEFAULT_BUTTON_LABEL, MODULE_ID};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct GiveawaySettings {
    #[serde(
        alias = "default_channel_id",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    pub(crate) default_channel: Option<u64>,
    pub(crate) button_label: String,
}

impl Default for GiveawaySettings {
    fn default() -> Self {
        Self {
            default_channel: None,
            button_label: DEFAULT_BUTTON_LABEL.to_string(),
        }
    }
}

pub(crate) async fn load_settings(
    data: &AppState,
    guild_id: u64,
) -> Result<GiveawaySettings, Error> {
    let guild_settings = data.guild_settings_cached(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| decode_module_configuration::<GiveawaySettings>(&module.configuration))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::GiveawaySettings;
    #[test]
    fn giveaway_settings_accept_string_channel() {
        let settings: GiveawaySettings = serde_json::from_value(
            serde_json::json!({"default_channel":"123","button_label":"Join"}),
        )
        .expect("settings");
        assert_eq!(settings.default_channel, Some(123));
        assert_eq!(settings.button_label, "Join");
    }
}
