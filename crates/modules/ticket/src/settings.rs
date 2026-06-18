use crate::constants::{
    DEFAULT_LIMIT, DEFAULT_SETUP_DESCRIPTION, DEFAULT_SETUP_FOOTER, DEFAULT_SETUP_TITLE, MODULE_ID,
};
use dynamo_module_kit::{SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection};
use dynamo_runtime_api::{AppState, Context, Error};
use dynamo_settings::GuildModuleSettings;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TicketSettings {
    pub(crate) setup_title: String,
    pub(crate) setup_description: String,
    pub(crate) setup_footer: String,
    #[serde(
        alias = "log_channel",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    pub(crate) log_channel_id: Option<u64>,
    pub(crate) limit: usize,
    pub(crate) categories: Vec<TicketCategory>,
}

impl Default for TicketSettings {
    fn default() -> Self {
        Self {
            setup_title: DEFAULT_SETUP_TITLE.to_string(),
            setup_description: DEFAULT_SETUP_DESCRIPTION.to_string(),
            setup_footer: DEFAULT_SETUP_FOOTER.to_string(),
            log_channel_id: None,
            limit: DEFAULT_LIMIT,
            categories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct TicketCategory {
    pub(crate) name: String,
    #[serde(alias = "staff_roles", deserialize_with = "deserialize_snowflake_vec")]
    pub(crate) staff_role_ids: Vec<u64>,
}

pub(crate) fn settings_schema() -> SettingsSchema {
    SettingsSchema {
        sections: vec![SettingsSection {
            id: "ticketing",
            title: "Ticketing",
            description: Some(
                "Configure ticket panel text, log channel, open-ticket limit, and category roles.",
            ),
            fields: vec![
                SettingsField {
                    key: "setup_title",
                    label: "Panel title",
                    help_text: Some("Embed title used in the ticket creation panel."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "setup_description",
                    label: "Panel description",
                    help_text: Some("Embed description used in the ticket creation panel."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "setup_footer",
                    label: "Panel footer",
                    help_text: Some("Embed footer used in the ticket creation panel."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "log_channel_id",
                    label: "Log channel ID",
                    help_text: Some(
                        "Optional channel that receives ticket closed notifications and transcripts.",
                    ),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "limit",
                    label: "Open ticket limit",
                    help_text: Some("Maximum number of concurrently open ticket channels."),
                    required: false,
                    kind: SettingsFieldKind::Integer,
                },
                SettingsField {
                    key: "categories",
                    label: "Categories",
                    help_text: Some(
                        "Array of category objects with `name` and `staff_roles`/`staff_role_ids`.",
                    ),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
            ],
        }],
    }
}

pub(crate) async fn load_settings(
    data: &AppState,
    guild_id: Option<u64>,
) -> Result<TicketSettings, Error> {
    let Some(guild_id) = guild_id else {
        return Ok(TicketSettings::default());
    };

    let guild_settings = data.persistence.guild_settings_or_default(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(parse_ticket_settings)
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

fn parse_ticket_settings(module: &GuildModuleSettings) -> Result<TicketSettings, Error> {
    Ok(serde_json::from_value::<TicketSettings>(
        module.configuration.clone(),
    )?)
}

pub(crate) async fn save_settings(
    ctx: Context<'_>,
    settings: &TicketSettings,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let Some(repo) = ctx.data().persistence.guild_settings.clone() else {
        return Err(anyhow::anyhow!(
            "guild settings repository is not configured for this deployment"
        ));
    };

    let current = repo.get_or_create(guild_id.get()).await?;
    let enabled = current
        .modules
        .get(MODULE_ID)
        .map(|module| module.enabled)
        .unwrap_or(true);

    repo.upsert_module_settings(
        guild_id.get(),
        MODULE_ID,
        GuildModuleSettings {
            enabled,
            configuration: serde_json::to_value(settings)?,
        },
    )
    .await?;

    Ok(())
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
