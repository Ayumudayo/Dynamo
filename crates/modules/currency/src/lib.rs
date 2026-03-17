use dynamo_domain_currency::{ExchangeRateSourceKind, supported_currency_specs};
use dynamo_enablement::module_access_for_context;
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingOption,
    SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{AppState, Context, Error};
use dynamo_settings::{DeploymentCommandSettings, GuildCommandSettings};
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

pub struct CurrencyModule;

impl Module<AppState, Error> for CurrencyModule {
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

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
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
    let mut embed = CreateEmbed::new()
        .title(format!("Exchange rate from {from} to {to}"))
        .thumbnail(CURRENCY_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new("Data from Google Finance."))
        .timestamp(Timestamp::now())
        .field("From", format!("{} {from}", format_decimal(amount)), false)
        .field("To", format!("{} {to}", format_decimal(converted)), false);

    if quote.source_kind == ExchangeRateSourceKind::Cache {
        embed = embed.field("Data Source", source_label(quote.source_kind), false);
    }

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
    if responses.iter().all(|(_, quote)| quote.is_none()) {
        ctx.say("Failed to fetch latest exchange data and no cached data is available.")
            .await?;
        return Ok(());
    }
    let mut embed = CreateEmbed::new()
        .title(format!(
            "Exchange rate from {} {from}",
            format_decimal(amount)
        ))
        .thumbnail(CURRENCY_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new("Data from Google Finance."))
        .timestamp(Timestamp::now());

    for (currency, rate) in responses {
        let name = currency_display_label(&currency);

        embed = embed.field(
            name,
            rate.map(|quote| {
                let value = format_decimal(quote.rate * amount);
                if quote.source_kind == ExchangeRateSourceKind::Cache {
                    format!("{value}\nCached fallback")
                } else {
                    value
                }
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

fn currency_option_label(code: &str) -> &'static str {
    match code {
        "AED" => "United Arab Emirates Dirham (AED)",
        "AFN" => "Afghan Afghani (AFN)",
        "ALL" => "Albanian Lek (ALL)",
        "AMD" => "Armenian Dram (AMD)",
        "ANG" => "Netherlands Antillean Guilder (ANG)",
        "AOA" => "Angolan Kwanza (AOA)",
        "ARS" => "Argentine Peso (ARS)",
        "AUD" => "Australian Dollar (AUD)",
        "AWG" => "Aruban Florin (AWG)",
        "AZN" => "Azerbaijani Manat (AZN)",
        "BAM" => "Bosnia and Herzegovina Convertible Mark (BAM)",
        "BBD" => "Barbadian Dollar (BBD)",
        "BDT" => "Bangladeshi Taka (BDT)",
        "BGN" => "Bulgarian Lev (BGN)",
        "BHD" => "Bahraini Dinar (BHD)",
        "BIF" => "Burundian Franc (BIF)",
        "BMD" => "Bermudian Dollar (BMD)",
        "BND" => "Brunei Dollar (BND)",
        "BOB" => "Bolivian Boliviano (BOB)",
        "BRL" => "Brazilian Real (BRL)",
        "BSD" => "Bahamian Dollar (BSD)",
        "BTN" => "Bhutanese Ngultrum (BTN)",
        "BWP" => "Botswana Pula (BWP)",
        "BYN" => "Belarusian Ruble (BYN)",
        "BZD" => "Belize Dollar (BZD)",
        "CAD" => "Canadian Dollar (CAD)",
        "CDF" => "Congolese Franc (CDF)",
        "CHF" => "Swiss Franc (CHF)",
        "CLP" => "Chilean Peso (CLP)",
        "CNY" => "Chinese Yuan (CNY)",
        "COP" => "Colombian Peso (COP)",
        "CRC" => "Costa Rican Colon (CRC)",
        "CUP" => "Cuban Peso (CUP)",
        "CVE" => "Cape Verdean Escudo (CVE)",
        "CZK" => "Czech Koruna (CZK)",
        "DJF" => "Djiboutian Franc (DJF)",
        "DKK" => "Danish Krone (DKK)",
        "DOP" => "Dominican Peso (DOP)",
        "DZD" => "Algerian Dinar (DZD)",
        "EGP" => "Egyptian Pound (EGP)",
        "ERN" => "Eritrean Nakfa (ERN)",
        "ETB" => "Ethiopian Birr (ETB)",
        "EUR" => "Euro (EUR)",
        "FJD" => "Fijian Dollar (FJD)",
        "FKP" => "Falkland Islands Pound (FKP)",
        "GBP" => "British Pound Sterling (GBP)",
        "GEL" => "Georgian Lari (GEL)",
        "GHS" => "Ghanaian Cedi (GHS)",
        "GIP" => "Gibraltar Pound (GIP)",
        "GMD" => "Gambian Dalasi (GMD)",
        "GNF" => "Guinean Franc (GNF)",
        "GTQ" => "Guatemalan Quetzal (GTQ)",
        "GYD" => "Guyanese Dollar (GYD)",
        "HKD" => "Hong Kong Dollar (HKD)",
        "HNL" => "Honduran Lempira (HNL)",
        "HTG" => "Haitian Gourde (HTG)",
        "HUF" => "Hungarian Forint (HUF)",
        "IDR" => "Indonesian Rupiah (IDR)",
        "ILS" => "Israeli New Shekel (ILS)",
        "INR" => "Indian Rupee (INR)",
        "IQD" => "Iraqi Dinar (IQD)",
        "IRR" => "Iranian Rial (IRR)",
        "ISK" => "Icelandic Krona (ISK)",
        "JMD" => "Jamaican Dollar (JMD)",
        "JOD" => "Jordanian Dinar (JOD)",
        "JPY" => "Japanese Yen (JPY)",
        "KES" => "Kenyan Shilling (KES)",
        "KGS" => "Kyrgyzstani Som (KGS)",
        "KHR" => "Cambodian Riel (KHR)",
        "KMF" => "Comorian Franc (KMF)",
        "KRW" => "South Korean Won (KRW)",
        "KWD" => "Kuwaiti Dinar (KWD)",
        "KYD" => "Cayman Islands Dollar (KYD)",
        "KZT" => "Kazakhstani Tenge (KZT)",
        "LAK" => "Lao Kip (LAK)",
        "LBP" => "Lebanese Pound (LBP)",
        "LKR" => "Sri Lankan Rupee (LKR)",
        "LRD" => "Liberian Dollar (LRD)",
        "LSL" => "Lesotho Loti (LSL)",
        "LYD" => "Libyan Dinar (LYD)",
        "MAD" => "Moroccan Dirham (MAD)",
        "MDL" => "Moldovan Leu (MDL)",
        "MGA" => "Malagasy Ariary (MGA)",
        "MKD" => "Macedonian Denar (MKD)",
        "MMK" => "Myanmar Kyat (MMK)",
        "MNT" => "Mongolian Tugrik (MNT)",
        "MOP" => "Macanese Pataca (MOP)",
        "MRU" => "Mauritanian Ouguiya (MRU)",
        "MUR" => "Mauritian Rupee (MUR)",
        "MVR" => "Maldivian Rufiyaa (MVR)",
        "MWK" => "Malawian Kwacha (MWK)",
        "MXN" => "Mexican Peso (MXN)",
        "MYR" => "Malaysian Ringgit (MYR)",
        "MZN" => "Mozambican Metical (MZN)",
        "NAD" => "Namibian Dollar (NAD)",
        "NGN" => "Nigerian Naira (NGN)",
        "NIO" => "Nicaraguan Cordoba (NIO)",
        "NOK" => "Norwegian Krone (NOK)",
        "NPR" => "Nepalese Rupee (NPR)",
        "NZD" => "New Zealand Dollar (NZD)",
        "OMR" => "Omani Rial (OMR)",
        "PAB" => "Panamanian Balboa (PAB)",
        "PEN" => "Peruvian Sol (PEN)",
        "PGK" => "Papua New Guinean Kina (PGK)",
        "PHP" => "Philippine Peso (PHP)",
        "PKR" => "Pakistani Rupee (PKR)",
        "PLN" => "Polish Zloty (PLN)",
        "PYG" => "Paraguayan Guarani (PYG)",
        "QAR" => "Qatari Riyal (QAR)",
        "RON" => "Romanian Leu (RON)",
        "RSD" => "Serbian Dinar (RSD)",
        "RUB" => "Russian Ruble (RUB)",
        "RWF" => "Rwandan Franc (RWF)",
        "SAR" => "Saudi Riyal (SAR)",
        "SBD" => "Solomon Islands Dollar (SBD)",
        "SCR" => "Seychellois Rupee (SCR)",
        "SDG" => "Sudanese Pound (SDG)",
        "SEK" => "Swedish Krona (SEK)",
        "SGD" => "Singapore Dollar (SGD)",
        "SHP" => "Saint Helena Pound (SHP)",
        "SLE" => "Sierra Leonean Leone (SLE)",
        "SOS" => "Somali Shilling (SOS)",
        "SRD" => "Surinamese Dollar (SRD)",
        "SSP" => "South Sudanese Pound (SSP)",
        "STN" => "Sao Tome and Principe Dobra (STN)",
        "SVC" => "Salvadoran Colon (SVC)",
        "SYP" => "Syrian Pound (SYP)",
        "SZL" => "Swazi Lilangeni (SZL)",
        "THB" => "Thai Baht (THB)",
        "TJS" => "Tajikistani Somoni (TJS)",
        "TMT" => "Turkmenistani Manat (TMT)",
        "TND" => "Tunisian Dinar (TND)",
        "TOP" => "Tongan Paʻanga (TOP)",
        "TRY" => "Turkish Lira (TRY)",
        "TTD" => "Trinidad and Tobago Dollar (TTD)",
        "TWD" => "New Taiwan Dollar (TWD)",
        "TZS" => "Tanzanian Shilling (TZS)",
        "UAH" => "Ukrainian Hryvnia (UAH)",
        "UGX" => "Ugandan Shilling (UGX)",
        "USD" => "United States Dollar (USD)",
        "UYU" => "Uruguayan Peso (UYU)",
        "UZS" => "Uzbekistani Som (UZS)",
        "VES" => "Venezuelan Bolivar (VES)",
        "VND" => "Vietnamese Dong (VND)",
        "VUV" => "Vanuatu Vatu (VUV)",
        "WST" => "Samoan Tala (WST)",
        "XAF" => "Central African CFA Franc (XAF)",
        "XCD" => "East Caribbean Dollar (XCD)",
        "XOF" => "West African CFA Franc (XOF)",
        "XPF" => "CFP Franc (XPF)",
        "YER" => "Yemeni Rial (YER)",
        "ZAR" => "South African Rand (ZAR)",
        "ZMW" => "Zambian Kwacha (ZMW)",
        "ZWL" => "Zimbabwean Dollar (ZWL)",
        _ => "Custom Currency",
    }
}

fn normalize_currency(input: &str) -> String {
    input.trim().to_ascii_uppercase()
}

fn source_label(kind: ExchangeRateSourceKind) -> &'static str {
    match kind {
        ExchangeRateSourceKind::Live => "Live",
        ExchangeRateSourceKind::Cache => "Cached fallback",
    }
}

fn currency_display_label(currency: &str) -> String {
    let normalized = normalize_currency(currency);
    dynamo_domain_currency::currency_display_label(&normalized)
        .map(str::to_string)
        .unwrap_or_else(|| format!("🌐 {normalized}"))
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
    use super::{
        currency_display_label, currency_option_label, format_decimal, normalize_currency,
    };
    use dynamo_domain_currency::supported_currency_specs;

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

    #[test]
    fn all_supported_rate_currencies_have_display_labels() {
        for currency in supported_currency_specs() {
            let code = currency.code;
            let label = currency_display_label(code);
            assert!(
                label.ends_with(code),
                "display label should end with currency code for {code}: {label}"
            );
            assert_ne!(
                label, code,
                "supported currency should not fall back to bare code: {code}"
            );
        }
    }

    #[test]
    fn dropdown_labels_include_human_readable_currency_names() {
        assert_eq!(currency_option_label("KRW"), "South Korean Won (KRW)");
        assert_eq!(currency_option_label("USD"), "United States Dollar (USD)");
        assert_eq!(currency_option_label("EUR"), "Euro (EUR)");
    }
}
