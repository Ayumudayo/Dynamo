use crate::constants::{
    DEFAULT_LIMIT, DEFAULT_SETUP_DESCRIPTION, DEFAULT_SETUP_FOOTER, DEFAULT_SETUP_TITLE, MODULE_ID,
};
use dynamo_module_kit::{SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection};
use dynamo_runtime_api::{
    AppState, Context, Error, decode_module_configuration, deserialize_optional_snowflake,
    deserialize_snowflake_vec,
};
use dynamo_settings::{GuildModuleSettings, GuildSettings};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TicketSettings {
    pub(crate) setup_title: String,
    pub(crate) setup_description: String,
    pub(crate) setup_footer: String,
    #[serde(
        alias = "log_channel",
        serialize_with = "dynamo_runtime_api::serialize_optional_snowflake",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    pub(crate) log_channel_id: Option<u64>,
    #[serde(
        default,
        serialize_with = "dynamo_runtime_api::serialize_optional_snowflake",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    pub(crate) category_id: Option<u64>,
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
            category_id: None,
            limit: DEFAULT_LIMIT,
            categories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub(crate) struct TicketCategory {
    pub(crate) name: String,
    #[serde(
        rename = "staff_roles",
        alias = "staff_role_ids",
        serialize_with = "dynamo_runtime_api::serialize_snowflake_vec",
        deserialize_with = "deserialize_snowflake_vec"
    )]
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
                    kind: SettingsFieldKind::Textarea,
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
                    label: "Log channel",
                    help_text: Some(
                        "Optional channel that receives ticket closed notifications and transcripts.",
                    ),
                    required: false,
                    kind: SettingsFieldKind::Channel,
                },
                SettingsField {
                    key: "category_id",
                    label: "Ticket category ID",
                    help_text: Some(
                        "Optional Discord category ID where new ticket channels are created.",
                    ),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "limit",
                    label: "Open ticket limit",
                    help_text: Some("Maximum number of concurrently open ticket channels."),
                    required: false,
                    kind: SettingsFieldKind::Integer {
                        min: None,
                        max: None,
                    },
                },
                SettingsField {
                    key: "categories",
                    label: "Categories",
                    help_text: Some(
                        "Add ticket topics and select the staff roles notified for each topic.",
                    ),
                    required: false,
                    kind: SettingsFieldKind::TicketCategories,
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

    let guild_settings = data.guild_settings_cached(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| decode_module_configuration::<TicketSettings>(&module.configuration))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
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

    let current = repo
        .get(guild_id.get())
        .await?
        .unwrap_or_else(|| GuildSettings::for_guild(guild_id.get()));
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
    ctx.data()
        .invalidate_guild_settings_cache(guild_id.get())
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::TicketCategory;

    #[test]
    fn ticket_category_serializes_staff_roles_as_string_snowflakes() {
        let category = TicketCategory {
            name: "Billing".to_string(),
            staff_role_ids: vec![123_456_789_012_345_678],
        };
        let value = serde_json::to_value(&category).expect("serialize ticket category");
        assert_eq!(
            value,
            serde_json::json!({
                "name": "Billing",
                "staff_roles": ["123456789012345678"]
            })
        );
        let round_trip: TicketCategory =
            serde_json::from_value(value).expect("deserialize ticket category");
        assert_eq!(round_trip.name, "Billing");
        assert_eq!(round_trip.staff_role_ids, vec![123_456_789_012_345_678]);
    }
}
