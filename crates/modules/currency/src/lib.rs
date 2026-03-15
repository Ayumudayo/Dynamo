use std::env;

use dynamo_core::{
    Context, DiscordCommand, Error, GatewayIntents, Module, ModuleCategory, ModuleManifest,
    SettingsSchema, module_access_for_context,
};
use futures_util::future::join_all;
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, Timestamp};
use reqwest::Client;
use serde::Deserialize;

const MODULE_ID: &str = "currency";
const CURRENCY_THUMBNAIL_URL: &str = "https://cdn.discordapp.com/attachments/1138398345065414657/1138816034049105940/gil.png?ex=65c37c14&is=65b10714&hm=725d32835f239f48cf0a3485491431c7d02a1750b53c9086210d765b89e798f8&";
const BOT_EMBED_COLOR: u32 = 0x068ADD;
const DEFAULT_RATE_TARGETS: [&str; 6] = ["USD", "KRW", "JPY", "EUR", "TRY", "UAH"];

pub struct CurrencyModule;

impl Module for CurrencyModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Currency",
            "Exchange rate commands backed by ExchangeRate-API.",
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
}

#[derive(Debug, Deserialize)]
struct ExchangeRateApiResponse {
    result: String,
    #[serde(rename = "conversion_result")]
    conversion_result: Option<f64>,
    #[serde(rename = "conversion_rate")]
    conversion_rate: Option<f64>,
    #[serde(rename = "error-type")]
    error_type: Option<String>,
}

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

    let api_key = exchange_api_key()?;
    let client = Client::new();
    let from = normalize_currency(from.as_deref().unwrap_or("USD"));
    let to = normalize_currency(to.as_deref().unwrap_or("KRW"));
    let amount = amount.unwrap_or(1.0);

    let converted = convert(&client, &api_key, &from, &to, amount).await?;
    let embed = CreateEmbed::new()
        .title(format!("Exchange rate from {from} to {to}"))
        .thumbnail(CURRENCY_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new("Data from ExchangeRate-API."))
        .timestamp(Timestamp::now())
        .field("From", format!("{} {from}", format_decimal(amount)), false)
        .field("To", format!("{} {to}", format_decimal(converted)), false);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

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

    let api_key = exchange_api_key()?;
    let client = Client::new();
    let from = normalize_currency(from.as_deref().unwrap_or("USD"));
    let amount = amount.unwrap_or(1.0);

    let requests = DEFAULT_RATE_TARGETS.iter().map(|target| {
        let client = client.clone();
        let api_key = api_key.clone();
        let from = from.clone();
        let target = (*target).to_string();
        async move {
            let value = convert(&client, &api_key, &from, &target, amount)
                .await
                .ok();
            (target, value)
        }
    });

    let responses = join_all(requests).await;
    let mut embed = CreateEmbed::new()
        .title(format!(
            "Exchange rate from {} {from}",
            format_decimal(amount)
        ))
        .thumbnail(CURRENCY_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new("Data from ExchangeRate-API."))
        .timestamp(Timestamp::now());

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
            rate.map(format_decimal)
                .unwrap_or_else(|| "Failed to fetch".to_string()),
            true,
        );
    }

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

fn exchange_api_key() -> Result<String, Error> {
    env::var("EXCHANGE_API_KEY")
        .map_err(|_| anyhow::anyhow!("EXCHANGE_API_KEY is not configured in this deployment."))
}

async fn convert(
    client: &Client,
    api_key: &str,
    from: &str,
    to: &str,
    amount: f64,
) -> Result<f64, Error> {
    let url = format!("https://v6.exchangerate-api.com/v6/{api_key}/pair/{from}/{to}/{amount}");
    let response = client.get(url).send().await?;
    let payload = response.json::<ExchangeRateApiResponse>().await?;

    if payload.result == "success" {
        return payload
            .conversion_result
            .or(payload.conversion_rate)
            .ok_or_else(|| anyhow::anyhow!("ExchangeRate-API returned success without a value."));
    }

    Err(anyhow::anyhow!(
        "ExchangeRate-API Error: {}",
        payload
            .error_type
            .unwrap_or_else(|| "Unknown API Error".to_string())
    ))
}

fn normalize_currency(input: &str) -> String {
    input.trim().to_ascii_uppercase()
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
