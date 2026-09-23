use dynamo_module_kit::{
    SettingOption, SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{Context, Error};
use serde::{Deserialize, Serialize};

use crate::module::MODULE_ID;

pub(crate) fn settings_schema() -> SettingsSchema {
    SettingsSchema {
        sections: vec![SettingsSection {
            id: "moderation",
            title: "Moderation",
            description: Some("Configure modlog output and max-warning escalation."),
            fields: vec![
                SettingsField {
                    key: "modlog_channel_id",
                    label: "Modlog channel ID",
                    help_text: Some("Optional channel for moderation action embeds."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "max_warn.limit",
                    label: "Max warning limit",
                    help_text: Some("Auto-action threshold for warnings. Set 0 to disable."),
                    required: false,
                    kind: SettingsFieldKind::Integer {
                        min: None,
                        max: None,
                    },
                },
                SettingsField {
                    key: "max_warn.action",
                    label: "Max warning action",
                    help_text: Some("Action to take when warning threshold is reached."),
                    required: false,
                    kind: SettingsFieldKind::Select {
                        options: vec![
                            SettingOption {
                                label: "Timeout",
                                value: "TIMEOUT",
                            },
                            SettingOption {
                                label: "Kick",
                                value: "KICK",
                            },
                            SettingOption {
                                label: "Ban",
                                value: "BAN",
                            },
                        ],
                    },
                },
            ],
        }],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct ModerationSettings {
    #[serde(
        alias = "modlog_channel",
        alias = "modlog",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    pub(crate) modlog_channel_id: Option<u64>,
    pub(crate) max_warn: MaxWarnSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct MaxWarnSettings {
    pub(crate) limit: u64,
    pub(crate) action: MaxWarnAction,
}

impl Default for MaxWarnSettings {
    fn default() -> Self {
        Self {
            limit: 5,
            action: MaxWarnAction::Kick,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum MaxWarnAction {
    Timeout,
    #[default]
    Kick,
    Ban,
}

pub(crate) async fn load_settings(ctx: Context<'_>) -> Result<ModerationSettings, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(ModerationSettings::default());
    };
    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;
    Ok(guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| serde_json::from_value::<ModerationSettings>(module.configuration.clone()))
        .transpose()?
        .unwrap_or_default())
}

pub(crate) fn deserialize_optional_snowflake<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(value) if value.trim().is_empty() => Ok(None),
        serde_json::Value::String(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("snowflake number must be an unsigned integer"))
            .map(Some),
        other => Err(serde::de::Error::custom(format!(
            "snowflake must be a string or number, got {other}"
        ))),
    }
}
