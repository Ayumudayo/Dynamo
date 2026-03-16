use dynamo_core::{
    Context, DeploymentCommandSettings, DiscordCommand, Error, GatewayIntents,
    GuildCommandSettings, Module, ModuleCategory, ModuleManifest, SettingOption, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection, module_access_for_context,
};
use futures_util::future::join_all;
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, Timestamp};
use serde::{Deserialize, Serialize};

const MODULE_ID: &str = "currency";
const CURRENCY_THUMBNAIL_URL: &str = "https://cdn.discordapp.com/attachments/1138398345065414657/1138816034049105940/gil.png?ex=65c37c14&is=65b10714&hm=725d32835f239f48cf0a3485491431c7d02a1750b53c9086210d765b89e798f8&";
const BOT_EMBED_COLOR: u32 = 0x068ADD;
const DEFAULT_RATE_TARGETS: [&str; 6] = ["USD", "KRW", "JPY", "EUR", "TRY", "UAH"];
const DEFAULT_EXCHANGE_FROM: &str = "USD";
const DEFAULT_EXCHANGE_TO: &str = "KRW";
const DEFAULT_EXCHANGE_AMOUNT: f64 = 1.0;
const EXCHANGE_CHOICES: [&str; 18] = [
    "USD", "KRW", "EUR", "GBP", "JPY", "CAD", "CHF", "HKD", "TWD", "AUD", "NZD", "INR", "BRL",
    "PLN", "RUB", "TRY", "CNY", "UAH",
];

pub struct CurrencyModule;

impl Module for CurrencyModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Currency",
            "Exchange rate commands backed by Google Finance with cached fallback.",
            ModuleCategory::Currency,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![exchange(), rate()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::empty()
    }

    fn command_settings_schema(&self, command_id: &str) -> SettingsSchema {
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
                            help_text: Some(
                                "Used when /exchange is called without a source currency.",
                            ),
                            required: false,
                            kind: SettingsFieldKind::Select {
                                options: currency_select_options(false),
                            },
                        },
                        SettingsField {
                            key: "default_to",
                            label: "Default to currency",
                            help_text: Some(
                                "Used when /exchange is called without a target currency.",
                            ),
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
                    description: Some("Choose up to six currencies to display for /rate in order."),
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

/// Convert one currency amount into another currency.
#[poise::command(slash_command, category = "Currency")]
async fn exchange(
    ctx: Context<'_>,
    #[description = "The currency you want to convert (From) / Default : USD"] from: Option<String>,
    #[description = "The currency you want to convert (To) / Default : KRW"] to: Option<String>,
    #[description = "The amount of currency. / Default : 1.0"] amount: Option<f64>,
) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let defaults = load_exchange_defaults(ctx).await?;
    let from = normalize_currency(from.as_deref().unwrap_or(defaults.default_from.as_str()));
    let to = normalize_currency(to.as_deref().unwrap_or(defaults.default_to.as_str()));
    let amount = amount.unwrap_or(defaults.default_amount);
    let Some(service) = ctx.data().services.exchange_rates.as_ref() else {
        ctx.say("The exchange-rate service is not available in this deployment.")
            .await?;
        return Ok(());
    };
    let quote = match service.fetch_pair(&from, &to).await {
        Ok(quote) => quote,
        Err(_) => {
            ctx.say("Failed to fetch latest exchange data and no cached data is available.")
                .await?;
            return Ok(());
        }
    };
    let converted = quote.rate * amount;
    let embed = CreateEmbed::new()
        .title(format!("Exchange rate from {from} to {to}"))
        .thumbnail(CURRENCY_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(format!(
            "Data from Google Finance. {}",
            source_footer_suffix(quote.source_kind)
        )))
        .timestamp(Timestamp::from_unix_timestamp(
            quote.source_timestamp.timestamp(),
        )?)
        .field("From", format!("{} {from}", format_decimal(amount)), false)
        .field("To", format!("{} {to}", format_decimal(converted)), false)
        .field("Data Source", source_label(quote.source_kind), true)
        .field("As of", quote.source_timestamp_text, true);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Show the configured exchange-rate board for one base currency.
#[poise::command(slash_command, category = "Currency")]
async fn rate(
    ctx: Context<'_>,
    #[description = "The currency you want to convert from (default: USD)"] from: Option<String>,
    #[description = "The amount of currency (default: 1.0)"] amount: Option<f64>,
) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let defaults = load_exchange_defaults(ctx).await?;
    let from = normalize_currency(from.as_deref().unwrap_or(defaults.default_from.as_str()));
    let amount = amount.unwrap_or(defaults.default_amount);
    let rate_targets = load_rate_targets(ctx).await?;
    let Some(service) = ctx.data().services.exchange_rates.as_ref() else {
        ctx.say("The exchange-rate service is not available in this deployment.")
            .await?;
        return Ok(());
    };

    let requests = rate_targets.iter().map(|target| {
        let from = from.clone();
        let target = target.clone();
        let service = service.clone();
        async move {
            let value = service.fetch_pair(&from, &target).await.ok();
            (target, value)
        }
    });

    let responses = join_all(requests).await;
    let timestamps = responses
        .iter()
        .filter_map(|(_, quote)| {
            quote
                .as_ref()
                .map(|quote| quote.source_timestamp_text.clone())
        })
        .collect::<Vec<_>>();
    if timestamps.is_empty() {
        ctx.say("Failed to fetch latest exchange data and no cached data is available.")
            .await?;
        return Ok(());
    }
    let uniform_timestamp = timestamps
        .first()
        .filter(|first| timestamps.iter().all(|timestamp| timestamp == *first))
        .cloned();
    let mut embed = CreateEmbed::new()
        .title(format!(
            "Exchange rate from {} {from}",
            format_decimal(amount)
        ))
        .thumbnail(CURRENCY_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(match uniform_timestamp {
            Some(timestamp) => format!("Data from Google Finance. As of {timestamp}."),
            None => "Data from Google Finance. Timestamps vary by row.".to_string(),
        }));

    for (currency, rate) in responses {
        let name = match currency.as_str() {
            "USD" => "🇺🇸 USD",
            "KRW" => "🇰🇷 KRW",
            "JPY" => "🇯🇵 JPY",
            "EUR" => "🇪🇺 EUR",
            "TRY" => "🇹🇷 TRY",
            "UAH" => "🇺🇦 UAH",
            _ => currency.as_str(),
        };

        embed = embed.field(
            name,
            rate.map(|quote| {
                format!(
                    "{}\n{} · {}",
                    format_decimal(quote.rate * amount),
                    source_label(quote.source_kind),
                    quote.source_timestamp_text
                )
            })
            .unwrap_or_else(|| "Failed to fetch".to_string()),
            true,
        );
    }

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

async fn load_exchange_defaults(ctx: Context<'_>) -> Result<ResolvedExchangeDefaults, Error> {
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

async fn load_rate_targets(ctx: Context<'_>) -> Result<Vec<String>, Error> {
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
struct ResolvedExchangeDefaults {
    default_from: String,
    default_to: String,
    default_amount: f64,
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

fn default_rate_targets() -> Vec<String> {
    DEFAULT_RATE_TARGETS
        .iter()
        .map(|value| (*value).to_string())
        .collect()
}

fn normalize_rate_targets(targets: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    targets
        .into_iter()
        .map(|value| normalize_currency(&value))
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn rate_target_field(key: &'static str, label: &'static str) -> SettingsField {
    SettingsField {
        key,
        label,
        help_text: Some("Leave blank to skip this slot in the /rate result list."),
        required: false,
        kind: SettingsFieldKind::Select {
            options: currency_select_options(true),
        },
    }
}

fn currency_select_options(include_blank: bool) -> Vec<SettingOption> {
    let mut options = Vec::new();
    if include_blank {
        options.push(SettingOption {
            label: "Unused",
            value: "",
        });
    }
    options.extend(EXCHANGE_CHOICES.iter().map(|currency| SettingOption {
        label: currency,
        value: currency,
    }));
    options
}

fn normalize_currency(input: &str) -> String {
    input.trim().to_ascii_uppercase()
}

fn source_label(kind: dynamo_core::ExchangeRateSourceKind) -> &'static str {
    match kind {
        dynamo_core::ExchangeRateSourceKind::Live => "Live",
        dynamo_core::ExchangeRateSourceKind::Cache => "Cached fallback",
    }
}

fn source_footer_suffix(kind: dynamo_core::ExchangeRateSourceKind) -> &'static str {
    match kind {
        dynamo_core::ExchangeRateSourceKind::Live => "Live quote.",
        dynamo_core::ExchangeRateSourceKind::Cache => "Cached fallback.",
    }
}

fn format_decimal(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    let sign = if rounded < 0.0 { "-" } else { "" };
    let absolute = rounded.abs();
    let integer = absolute.trunc() as i64;
    let fraction = ((absolute - integer as f64) * 100.0).round() as i64;
    let integer = format_grouped_integer(integer);

    if fraction == 0 {
        return format!("{sign}{integer}");
    }

    if fraction % 10 == 0 {
        return format!("{sign}{integer}.{}", fraction / 10);
    }

    format!("{sign}{integer}.{fraction:02}")
}

fn format_grouped_integer(value: i64) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    let mut count = 0usize;

    for ch in digits.chars().rev() {
        if count == 3 {
            output.push(',');
            count = 0;
        }
        output.push(ch);
        count += 1;
    }

    output.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::{format_decimal, normalize_currency};

    #[test]
    fn formats_grouped_decimals_like_js_locale_output() {
        assert_eq!(format_decimal(12345.678), "12,345.68");
        assert_eq!(format_decimal(12345.6), "12,345.6");
        assert_eq!(format_decimal(12345.0), "12,345");
    }

    #[test]
    fn normalizes_currency_to_uppercase() {
        assert_eq!(normalize_currency(" krw "), "KRW");
    }
}
