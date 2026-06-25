use crate::constants::{
    DEFAULT_ETF_TICKERS, DEFAULT_REFRESH_DURATION_SECONDS, DEFAULT_REFRESH_INTERVAL_SECONDS,
    DEFAULT_SYMBOL, MAX_REFRESH_DURATION_SECONDS, MIN_REFRESH_DURATION_SECONDS,
    MIN_REFRESH_INTERVAL_SECONDS, MODULE_ID,
};
use dynamo_module_kit::{SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection};
use dynamo_runtime_api::{Context, Error};
use dynamo_settings::{DeploymentCommandSettings, GuildCommandSettings, GuildModuleSettings};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StockSettings {
    pub(crate) default_symbol: String,
    pub(crate) etf_tickers: Vec<String>,
    #[serde(
        default = "default_refresh_interval_seconds",
        deserialize_with = "deserialize_refresh_interval_seconds"
    )]
    pub(crate) refresh_interval_seconds: u32,
    #[serde(
        default = "default_refresh_duration_seconds",
        deserialize_with = "deserialize_refresh_duration_seconds"
    )]
    pub(crate) refresh_duration_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct EtfCommandSettings {
    ticker_1: String,
    ticker_2: String,
    ticker_3: String,
    ticker_4: String,
    ticker_5: String,
}

impl Default for StockSettings {
    fn default() -> Self {
        Self {
            default_symbol: DEFAULT_SYMBOL.to_string(),
            etf_tickers: DEFAULT_ETF_TICKERS
                .iter()
                .map(|value| value.to_string())
                .collect(),
            refresh_interval_seconds: DEFAULT_REFRESH_INTERVAL_SECONDS,
            refresh_duration_seconds: DEFAULT_REFRESH_DURATION_SECONDS,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct RefreshSchedule {
    pub(crate) interval_seconds: u32,
    pub(crate) duration_seconds: u32,
}

impl RefreshSchedule {
    pub(crate) fn total_updates(self) -> u32 {
        (self.duration_seconds / self.interval_seconds).max(1)
    }
}

impl StockSettings {
    pub(crate) fn refresh_schedule(&self) -> RefreshSchedule {
        let duration_seconds = self
            .refresh_duration_seconds
            .clamp(MIN_REFRESH_DURATION_SECONDS, MAX_REFRESH_DURATION_SECONDS);
        let interval_seconds = self
            .refresh_interval_seconds
            .max(MIN_REFRESH_INTERVAL_SECONDS)
            .min(duration_seconds);

        RefreshSchedule {
            interval_seconds,
            duration_seconds,
        }
    }
}

pub(crate) fn settings_schema() -> SettingsSchema {
    SettingsSchema {
        sections: vec![SettingsSection {
            id: "quotes",
            title: "Quotes",
            description: Some("Customize stock defaults and ETF ticker groups."),
            fields: vec![
                SettingsField {
                    key: "default_symbol",
                    label: "Default stock symbol",
                    help_text: Some("Used by /stock when no symbol is supplied."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "etf_tickers",
                    label: "ETF tickers",
                    help_text: Some("Array of ETF tickers used by /etf."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "refresh_interval_seconds",
                    label: "Refresh interval seconds",
                    help_text: Some("Controls live embed edit cadence. Minimum 3 seconds."),
                    required: false,
                    kind: SettingsFieldKind::Integer {
                        min: Some(MIN_REFRESH_INTERVAL_SECONDS as i64),
                        max: None,
                    },
                },
                SettingsField {
                    key: "refresh_duration_seconds",
                    label: "Refresh duration seconds",
                    help_text: Some(
                        "Controls how long live embeds update. Allowed range: 60-180 seconds.",
                    ),
                    required: false,
                    kind: SettingsFieldKind::Integer {
                        min: Some(MIN_REFRESH_DURATION_SECONDS as i64),
                        max: Some(MAX_REFRESH_DURATION_SECONDS as i64),
                    },
                },
            ],
        }],
    }
}

pub(crate) async fn load_settings(ctx: Context<'_>) -> Result<StockSettings, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(StockSettings::default());
    };

    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;

    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(parse_stock_settings)
        .transpose()?
        .unwrap_or_default();

    Ok(settings)
}

pub(crate) async fn load_effective_etf_tickers(ctx: Context<'_>) -> Result<Vec<String>, Error> {
    let settings = load_settings(ctx).await?;
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(normalize_symbols(settings.etf_tickers));
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

    let guild_override = guild_settings
        .commands
        .get("etf")
        .and_then(parse_etf_command_settings)
        .filter(|tickers| !tickers.is_empty());
    if let Some(tickers) = guild_override {
        return Ok(tickers);
    }

    let deployment_override = deployment
        .commands
        .get("etf")
        .and_then(parse_deployment_etf_command_settings)
        .filter(|tickers| !tickers.is_empty());
    if let Some(tickers) = deployment_override {
        return Ok(tickers);
    }

    Ok(normalize_symbols(settings.etf_tickers))
}

pub(crate) fn parse_stock_settings(module: &GuildModuleSettings) -> Result<StockSettings, Error> {
    if module.configuration.is_null() {
        return Ok(StockSettings::default());
    }

    Ok(serde_json::from_value::<StockSettings>(
        module.configuration.clone(),
    )?)
}

fn default_refresh_interval_seconds() -> u32 {
    DEFAULT_REFRESH_INTERVAL_SECONDS
}

fn default_refresh_duration_seconds() -> u32 {
    DEFAULT_REFRESH_DURATION_SECONDS
}

fn deserialize_refresh_interval_seconds<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(parse_u32_setting(
        value.as_ref(),
        DEFAULT_REFRESH_INTERVAL_SECONDS,
    ))
}

fn deserialize_refresh_duration_seconds<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(parse_u32_setting(
        value.as_ref(),
        DEFAULT_REFRESH_DURATION_SECONDS,
    ))
}

fn parse_u32_setting(value: Option<&serde_json::Value>, default: u32) -> u32 {
    match value {
        Some(serde_json::Value::Number(number)) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(default),
        Some(serde_json::Value::String(value)) => value.trim().parse::<u32>().unwrap_or(default),
        Some(serde_json::Value::Null) | None => default,
        _ => default,
    }
}

fn parse_etf_command_settings(command: &GuildCommandSettings) -> Option<Vec<String>> {
    parse_etf_command_configuration(&command.configuration).ok()
}

fn parse_deployment_etf_command_settings(
    command: &DeploymentCommandSettings,
) -> Option<Vec<String>> {
    parse_etf_command_configuration(&command.configuration).ok()
}

fn parse_etf_command_configuration(
    configuration: &serde_json::Value,
) -> Result<Vec<String>, Error> {
    if configuration.is_null() {
        return Ok(Vec::new());
    }

    let settings = serde_json::from_value::<EtfCommandSettings>(configuration.clone())?;
    Ok(normalize_symbols(vec![
        settings.ticker_1,
        settings.ticker_2,
        settings.ticker_3,
        settings.ticker_4,
        settings.ticker_5,
    ]))
}

pub(crate) fn etf_ticker_field(key: &'static str, label: &'static str) -> SettingsField {
    SettingsField {
        key,
        label,
        help_text: Some("Optional ticker symbol. Leave blank to skip this slot."),
        required: false,
        kind: SettingsFieldKind::Text,
    }
}

pub(crate) fn normalize_symbol(symbol: String) -> String {
    let trimmed = symbol.trim().to_ascii_uppercase();
    if trimmed.is_empty() {
        DEFAULT_SYMBOL.to_string()
    } else {
        trimmed
    }
}

pub(crate) fn normalize_symbols(symbols: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    symbols
        .into_iter()
        .filter_map(normalize_optional_symbol)
        .filter(|symbol| seen.insert(symbol.clone()))
        .collect()
}

fn normalize_optional_symbol(symbol: String) -> Option<String> {
    let trimmed = symbol.trim().to_ascii_uppercase();
    (!trimmed.is_empty()).then_some(trimmed)
}
