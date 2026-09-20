use dynamo_domain_currency::supported_currency_specs;
use dynamo_module_kit::{
    SettingOption, SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{Context, Error};
use dynamo_settings::{DeploymentCommandSettings, GuildCommandSettings};
use serde::{Deserialize, Serialize};

use super::{
    currency::{
        DEFAULT_EXCHANGE_AMOUNT, DEFAULT_EXCHANGE_FROM, DEFAULT_EXCHANGE_TO, DEFAULT_RATE_TARGETS,
        default_rate_targets, normalize_rate_targets,
    },
    render::{currency_option_label, format_decimal, normalize_currency},
};

pub(super) fn command_settings_schema(command_id: &str) -> SettingsSchema {
    match command_id {
        "exchange" => SettingsSchema {
            sections: vec![SettingsSection {
                id: "exchange-defaults",
                title: "Exchange Defaults",
                description: Some("Defaults applied when /exchange arguments are omitted."),
                fields: vec![
                    SettingsField {
                        key: "default_from",
                        label: "Default from currency",
                        help_text: Some("Used when /exchange is called without a source currency."),
                        required: false,
                        kind: SettingsFieldKind::Select {
                            options: currency_select_options(false),
                        },
                    },
                    SettingsField {
                        key: "default_to",
                        label: "Default to currency",
                        help_text: Some("Used when /exchange is called without a target currency."),
                        required: false,
                        kind: SettingsFieldKind::Select {
                            options: currency_select_options(false),
                        },
                    },
                    SettingsField {
                        key: "default_amount",
                        label: "Default amount",
                        help_text: Some("Used when /exchange is called without an amount."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                ],
            }],
        },
        "rate" => SettingsSchema {
            sections: vec![SettingsSection {
                id: "rate-targets",
                title: "Rate Result Currencies",
                description: Some(
                    "Only KRW and USD are supported. Blank or matching values fall back to the opposite currency for the selected base.",
                ),
                fields: vec![
                    rate_target_field("target_1", "Result currency 1"),
                    rate_target_field("target_2", "Result currency 2"),
                    rate_target_field("target_3", "Result currency 3"),
                    rate_target_field("target_4", "Result currency 4"),
                    rate_target_field("target_5", "Result currency 5"),
                    rate_target_field("target_6", "Result currency 6"),
                ],
            }],
        },
        _ => SettingsSchema::empty(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ExchangeCommandSettings {
    default_from: String,
    default_to: String,
    default_amount: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct RateCommandSettings {
    target_1: String,
    target_2: String,
    target_3: String,
    target_4: String,
    target_5: String,
    target_6: String,
}

impl Default for ExchangeCommandSettings {
    fn default() -> Self {
        Self {
            default_from: DEFAULT_EXCHANGE_FROM.to_string(),
            default_to: DEFAULT_EXCHANGE_TO.to_string(),
            default_amount: format_decimal(DEFAULT_EXCHANGE_AMOUNT),
        }
    }
}

impl Default for RateCommandSettings {
    fn default() -> Self {
        Self {
            target_1: DEFAULT_RATE_TARGETS[0].to_string(),
            target_2: DEFAULT_RATE_TARGETS[1].to_string(),
            target_3: DEFAULT_RATE_TARGETS[2].to_string(),
            target_4: DEFAULT_RATE_TARGETS[3].to_string(),
            target_5: DEFAULT_RATE_TARGETS[4].to_string(),
            target_6: DEFAULT_RATE_TARGETS[5].to_string(),
        }
    }
}

pub(super) async fn load_exchange_defaults(
    ctx: Context<'_>,
) -> Result<ResolvedExchangeDefaults, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(ResolvedExchangeDefaults::default());
    };

    let deployment = ctx
        .data()
        .persistence
        .deployment_settings_or_default()
        .await?;
    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;

    let deployment_defaults = deployment
        .commands
        .get("exchange")
        .and_then(parse_deployment_exchange_defaults)
        .unwrap_or_default();
    let guild_defaults = guild_settings
        .commands
        .get("exchange")
        .and_then(parse_guild_exchange_defaults)
        .unwrap_or_default();

    Ok(ResolvedExchangeDefaults {
        default_from: if !guild_defaults.default_from.is_empty() {
            guild_defaults.default_from
        } else {
            first_non_empty(&deployment_defaults.default_from, DEFAULT_EXCHANGE_FROM)
        },
        default_to: if !guild_defaults.default_to.is_empty() {
            guild_defaults.default_to
        } else {
            first_non_empty(&deployment_defaults.default_to, DEFAULT_EXCHANGE_TO)
        },
        default_amount: guild_defaults
            .default_amount
            .or(deployment_defaults.default_amount)
            .unwrap_or(DEFAULT_EXCHANGE_AMOUNT),
    })
}

pub(super) async fn load_rate_targets(ctx: Context<'_>) -> Result<Vec<String>, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(default_rate_targets());
    };

    let deployment = ctx
        .data()
        .persistence
        .deployment_settings_or_default()
        .await?;
    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;

    if let Some(targets) = guild_settings
        .commands
        .get("rate")
        .and_then(parse_guild_rate_targets)
        .filter(|targets| !targets.is_empty())
    {
        return Ok(targets);
    }

    if let Some(targets) = deployment
        .commands
        .get("rate")
        .and_then(parse_deployment_rate_targets)
        .filter(|targets| !targets.is_empty())
    {
        return Ok(targets);
    }

    Ok(default_rate_targets())
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedExchangeDefaults {
    pub(super) default_from: String,
    pub(super) default_to: String,
    pub(super) default_amount: f64,
}

impl Default for ResolvedExchangeDefaults {
    fn default() -> Self {
        Self {
            default_from: DEFAULT_EXCHANGE_FROM.to_string(),
            default_to: DEFAULT_EXCHANGE_TO.to_string(),
            default_amount: DEFAULT_EXCHANGE_AMOUNT,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PartialExchangeDefaults {
    default_from: String,
    default_to: String,
    default_amount: Option<f64>,
}

fn parse_guild_exchange_defaults(
    command: &GuildCommandSettings,
) -> Option<PartialExchangeDefaults> {
    parse_exchange_defaults(&command.configuration).ok()
}

fn parse_deployment_exchange_defaults(
    command: &DeploymentCommandSettings,
) -> Option<PartialExchangeDefaults> {
    parse_exchange_defaults(&command.configuration).ok()
}

fn parse_exchange_defaults(
    configuration: &serde_json::Value,
) -> Result<PartialExchangeDefaults, Error> {
    if configuration.is_null() {
        return Ok(PartialExchangeDefaults::default());
    }

    let settings = serde_json::from_value::<ExchangeCommandSettings>(configuration.clone())?;
    Ok(PartialExchangeDefaults {
        default_from: normalize_currency(&settings.default_from),
        default_to: normalize_currency(&settings.default_to),
        default_amount: parse_amount(&settings.default_amount),
    })
}

fn parse_guild_rate_targets(command: &GuildCommandSettings) -> Option<Vec<String>> {
    parse_rate_targets(&command.configuration).ok()
}

fn parse_deployment_rate_targets(command: &DeploymentCommandSettings) -> Option<Vec<String>> {
    parse_rate_targets(&command.configuration).ok()
}

fn parse_rate_targets(configuration: &serde_json::Value) -> Result<Vec<String>, Error> {
    if configuration.is_null() {
        return Ok(Vec::new());
    }

    let settings = serde_json::from_value::<RateCommandSettings>(configuration.clone())?;
    Ok(normalize_rate_targets(vec![
        settings.target_1,
        settings.target_2,
        settings.target_3,
        settings.target_4,
        settings.target_5,
        settings.target_6,
    ]))
}

fn parse_amount(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    trimmed.parse::<f64>().ok().filter(|amount| *amount > 0.0)
}

fn first_non_empty(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn rate_target_field(key: &'static str, label: &'static str) -> SettingsField {
    SettingsField {
        key,
        label,
        help_text: Some(
            "Only KRW and USD are supported. Leave blank or repeat the base currency to use the opposite currency.",
        ),
        required: false,
        kind: SettingsFieldKind::Select {
            options: currency_select_options(true),
        },
    }
}

pub(super) fn currency_select_options(include_blank: bool) -> Vec<SettingOption> {
    let mut options = Vec::new();
    if include_blank {
        options.push(SettingOption {
            label: "Unused",
            value: "",
        });
    }
    options.extend(
        supported_currency_specs()
            .iter()
            .map(|currency| SettingOption {
                label: currency_option_label(currency.code),
                value: currency.code,
            }),
    );
    options
}
