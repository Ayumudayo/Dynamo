use dynamo_domain_currency::ExchangeRateSourceKind;
use dynamo_runtime_api::{Context, Error};
use futures_util::future::join_all;
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, Timestamp};

use super::{
    currency::{
        TOSS_EXCHANGE_SUPPORT_ERROR, is_toss_maintenance_error, resolve_explicit_exchange_pair,
        resolve_rate_base_currency, sanitize_rate_targets,
    },
    render::{currency_display_label, format_decimal, format_rate_board_value},
    settings::{load_exchange_defaults, load_rate_targets},
};

const MAX_EXCHANGE_AMOUNT: f64 = 1e15;
const BOT_EMBED_COLOR: u32 = 0x068ADD;
pub(super) const TOSS_EXCHANGE_PROVIDER_FOOTER: &str = "Toss Invest";
const TOSS_MAINTENANCE_MESSAGE: &str = "Toss Invest is under maintenance. Please try again later.";

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub(super) enum CurrencyChoice {
    #[name = "USD"]
    Usd,
    #[name = "KRW"]
    Krw,
}

impl CurrencyChoice {
    fn as_str(self) -> &'static str {
        match self {
            Self::Usd => "USD",
            Self::Krw => "KRW",
        }
    }
}

fn validate_amount(amount: f64) -> Result<(), &'static str> {
    if !amount.is_finite() || amount <= 0.0 || amount > MAX_EXCHANGE_AMOUNT {
        return Err("Amount must be a finite positive number up to 1,000,000,000,000,000.");
    }
    Ok(())
}

/// Convert one currency amount into another currency.
#[poise::command(slash_command, category = "Currency")]
pub(super) async fn exchange(
    ctx: Context<'_>,
    #[description = "KRW or USD. Default: USD"] from: Option<CurrencyChoice>,
    #[description = "KRW or USD. Default: KRW"] to: Option<CurrencyChoice>,
    #[description = "The amount of currency. / Default : 1.0"] amount: Option<f64>,
) -> Result<(), Error> {
    ctx.defer().await?;

    let defaults = load_exchange_defaults(ctx).await?;
    let (from, to) = match resolve_explicit_exchange_pair(
        from.map(CurrencyChoice::as_str),
        to.map(CurrencyChoice::as_str),
        &defaults,
    ) {
        Ok(pair) => pair,
        Err(message) => {
            ctx.say(message).await?;
            return Ok(());
        }
    };
    let amount = amount.unwrap_or(defaults.default_amount);
    if let Err(message) = validate_amount(amount) {
        ctx.say(message).await?;
        return Ok(());
    }
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
    let footer_text = if quote.source_kind == ExchangeRateSourceKind::Cache {
        format!(
            "{TOSS_EXCHANGE_PROVIDER_FOOTER} • {} (cached)",
            quote.source_timestamp_text
        )
    } else {
        format!(
            "{TOSS_EXCHANGE_PROVIDER_FOOTER} • {}",
            quote.source_timestamp_text
        )
    };
    let embed = CreateEmbed::new()
        .title(format!("Exchange rate from {from} to {to}"))
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(footer_text))
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
    #[description = "KRW or USD. Default: USD"] from: Option<CurrencyChoice>,
    #[description = "The amount of currency (default: 1.0)"] amount: Option<f64>,
) -> Result<(), Error> {
    ctx.defer().await?;

    let defaults = load_exchange_defaults(ctx).await?;
    let from = match resolve_rate_base_currency(from.map(CurrencyChoice::as_str), &defaults) {
        Ok(from) => from,
        Err(message) => {
            ctx.say(message).await?;
            return Ok(());
        }
    };
    let amount = amount.unwrap_or(defaults.default_amount);
    if let Err(message) = validate_amount(amount) {
        ctx.say(message).await?;
        return Ok(());
    }
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
    let mut footer_text = TOSS_EXCHANGE_PROVIDER_FOOTER.to_string();
    if let Some(first_ok) = responses.iter().find_map(|(_, res)| res.as_ref().ok()) {
        footer_text = if first_ok.source_kind == ExchangeRateSourceKind::Cache {
            format!(
                "{TOSS_EXCHANGE_PROVIDER_FOOTER} • {} (cached)",
                first_ok.source_timestamp_text
            )
        } else {
            format!(
                "{TOSS_EXCHANGE_PROVIDER_FOOTER} • {}",
                first_ok.source_timestamp_text
            )
        };
    }
    let mut embed = CreateEmbed::new()
        .title(format!(
            "Exchange rate from {} {from}",
            format_decimal(amount)
        ))
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(footer_text))
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
