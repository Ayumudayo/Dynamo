use crate::{
    constants::{MODULE_ID, STOCK_REFRESH_BUTTON_ID},
    render::{build_etf_response, build_stock_response, refresh_components},
    settings::{load_effective_etf_tickers, load_settings, normalize_symbol},
    state::{SessionKind, StockSession, initialize_session_loop, register_session, total_updates},
};
use dynamo_access::module_access_for_context;
use dynamo_runtime_api::{Context, Error};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Show a quote for one stock symbol and keep it refreshable for a short time.
#[poise::command(slash_command, guild_only, category = "Stock")]
pub(crate) async fn stock(
    ctx: Context<'_>,
    #[description = "Symbol of the stock"] symbol: Option<String>,
) -> Result<(), Error> {
    ctx.defer().await?;

    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let Some(service) = ctx.data().services.stock_quotes.clone() else {
        ctx.say("The stock data service is not available in this deployment.")
            .await?;
        return Ok(());
    };

    let settings = load_settings(ctx).await?;
    let symbol = normalize_symbol(symbol.unwrap_or(settings.default_symbol));
    let total_updates = total_updates();
    let response = build_stock_response(service.as_ref(), &symbol, 0, total_updates).await?;
    let Some(response) = response else {
        ctx.say("Failed to fetch stock data. Please try again later.")
            .await?;
        return Ok(());
    };

    let session = Arc::new(Mutex::new(StockSession::new(
        SessionKind::Stock {
            symbol: symbol.clone(),
        },
        service,
    )));

    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(response.embed.clone())
                .components(refresh_components(STOCK_REFRESH_BUTTON_ID)),
        )
        .await?;
    let message = reply.message().await?.into_owned();

    register_session(message.id.get(), session.clone()).await;
    initialize_session_loop(
        ctx.serenity_context().http.clone(),
        message.channel_id,
        message.id.get(),
        session,
        response.stop_reason,
    )
    .await;
    Ok(())
}

/// Show the configured ETF watchlist for this guild.
#[poise::command(slash_command, guild_only, category = "Stock")]
pub(crate) async fn etf(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let Some(service) = ctx.data().services.stock_quotes.clone() else {
        ctx.say("The stock data service is not available in this deployment.")
            .await?;
        return Ok(());
    };

    let tickers = load_effective_etf_tickers(ctx).await?;
    if tickers.is_empty() {
        ctx.say(
            "No stock tickers configured for this server. Please configure them in the dashboard.",
        )
        .await?;
        return Ok(());
    }

    let total_updates = total_updates();
    let response = build_etf_response(service.as_ref(), &tickers, 0, total_updates).await?;
    let Some(response) = response else {
        ctx.say("Failed to fetch ETF data. Please try again later.")
            .await?;
        return Ok(());
    };

    let session = Arc::new(Mutex::new(StockSession::new(
        SessionKind::Etf {
            tickers: tickers.clone(),
        },
        service,
    )));

    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(response.embed.clone())
                .components(refresh_components(STOCK_REFRESH_BUTTON_ID)),
        )
        .await?;
    let message = reply.message().await?.into_owned();

    register_session(message.id.get(), session.clone()).await;
    initialize_session_loop(
        ctx.serenity_context().http.clone(),
        message.channel_id,
        message.id.get(),
        session,
        response.stop_reason,
    )
    .await;
    Ok(())
}
