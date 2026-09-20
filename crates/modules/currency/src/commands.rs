use dynamo_access::module_access_for_context;
use dynamo_runtime_api::{Context, Error};
use futures_util::future::join_all;
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, Timestamp};

use super::{
    MODULE_ID,
    currency::{
        TOSS_EXCHANGE_SUPPORT_ERROR, is_toss_maintenance_error, resolve_explicit_exchange_pair,
        resolve_rate_base_currency, sanitize_rate_targets,
    },
    render::{currency_display_label, format_decimal, format_rate_board_value},
    settings::{load_exchange_defaults, load_rate_targets},
};

const CURRENCY_THUMBNAIL_URL: &str = "https://cdn.discordapp.com/attachments/1138398345065414657/1138816034049105940/gil.png?ex=65c37c14&is=65b10714&hm=725d32835f239f48cf0a3485491431c7d02a1750b53c9086210d765b89e798f8&";
const BOT_EMBED_COLOR: u32 = 0x068ADD;
pub(super) const TOSS_EXCHANGE_PROVIDER_FOOTER: &str = "Toss Invest";
const TOSS_MAINTENANCE_MESSAGE: &str = "Toss Invest is under maintenance. Please try again later.";

/// Convert one currency amount into another currency.
#[poise::command(slash_command, category = "Currency")]
pub(super) async fn exchange(
    ctx: Context<'_>,
    #[description = "KRW or USD. Default: USD"] from: Option<String>,
    #[description = "KRW or USD. Default: KRW"] to: Option<String>,
    #[description = "The amount of currency. / Default : 1.0"] amount: Option<f64>,
) -> Result<(), Error> {
    ctx.defer().await?;

    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let defaults = load_exchange_defaults(ctx).await?;
    let (from, to) = match resolve_explicit_exchange_pair(from.as_deref(), to.as_deref(), &defaults)
    {
        Ok(pair) => pair,
        Err(message) => {
            ctx.say(message).await?;
            return Ok(());
        }
    };
    let amount = amount.unwrap_or(defaults.default_amount);
    let Some(service) = ctx.data().services.exchange_rates.as_ref() else {
        ctx.say("The exchange-rate service is not available in this deployment.")
            .await?;
        return Ok(());
    };
    let quote = match service.fetch_pair(&from, &to).await {
        Ok(quote) => quote,
        Err(error) => {
            let message = if is_toss_maintenance_error(&error) {
                TOSS_MAINTENANCE_MESSAGE
            } else if error.to_string().contains("KRW") && error.to_string().contains("USD") {
                TOSS_EXCHANGE_SUPPORT_ERROR
            } else {
                "Failed to fetch the latest Toss Invest exchange rate."
            };
            ctx.say(message).await?;
            return Ok(());
        }
    };
    let converted = quote.rate * amount;
    let embed = CreateEmbed::new()
        .title(format!("Exchange rate from {from} to {to}"))
        .thumbnail(CURRENCY_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(TOSS_EXCHANGE_PROVIDER_FOOTER))
        .timestamp(Timestamp::now())
        .field("From", format!("{} {from}", format_decimal(amount)), false)
        .field("To", format!("{} {to}", format_decimal(converted)), false);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Show the configured exchange-rate board for one base currency.
#[poise::command(slash_command, category = "Currency")]
pub(super) async fn rate(
    ctx: Context<'_>,
    #[description = "KRW or USD. Default: USD"] from: Option<String>,
    #[description = "The amount of currency (default: 1.0)"] amount: Option<f64>,
) -> Result<(), Error> {
    ctx.defer().await?;

    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let defaults = load_exchange_defaults(ctx).await?;
    let from = match resolve_rate_base_currency(from.as_deref(), &defaults) {
        Ok(from) => from,
        Err(message) => {
            ctx.say(message).await?;
            return Ok(());
        }
    };
    let amount = amount.unwrap_or(defaults.default_amount);
    let rate_targets = sanitize_rate_targets(&from, load_rate_targets(ctx).await?);
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
            let result = service.fetch_pair(&from, &target).await;
            (target, result)
        }
    });

    let responses = join_all(requests).await;
    if responses.iter().all(|(_, quote)| quote.is_err()) {
        let message = if responses
            .iter()
            .all(|(_, quote)| quote.as_ref().err().is_some_and(is_toss_maintenance_error))
        {
            TOSS_MAINTENANCE_MESSAGE
        } else {
            "Failed to fetch the latest Toss Invest exchange rates."
        };
        ctx.say(message).await?;
        return Ok(());
    }
    let mut embed = CreateEmbed::new()
        .title(format!(
            "Exchange rate from {} {from}",
            format_decimal(amount)
        ))
        .thumbnail(CURRENCY_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(TOSS_EXCHANGE_PROVIDER_FOOTER))
        .timestamp(Timestamp::now());

    for (currency, rate) in responses {
        let name = currency_display_label(&currency);

        embed = embed.field(
            name,
            rate.map(|quote| format_rate_board_value(&quote, amount))
                .unwrap_or_else(|_| "Failed to fetch".to_string()),
            true,
        );
    }

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}
