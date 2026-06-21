use crate::{
    constants::MODULE_ID,
    maintenance::{
        build_maintenance_embed, create_maintenance_error_embed, fetch_maintenance_info,
    },
    pll::{build_pll_embed, build_pll_stream_buttons, create_pll_error_embed, fetch_pll_info},
    referral::{build_referral_reply, load_settings},
};
use dynamo_access::module_access_for_context;
use dynamo_runtime_api::{Context, Error};

/// Show the War Thunder and World of Tanks referral panel for this guild.
#[poise::command(slash_command, guild_only, category = "Game Info")]
pub(crate) async fn wtinv(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let settings = load_settings(ctx).await?;
    ctx.send(build_referral_reply(&settings)).await?;
    Ok(())
}

/// Show the latest known FFXIV global maintenance window.
#[poise::command(slash_command, category = "Game Info")]
pub(crate) async fn maint(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    ctx.defer().await?;

    let embed = fetch_maintenance_info()
        .await?
        .map(|info| build_maintenance_embed(&info))
        .unwrap_or_else(create_maintenance_error_embed);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Show the latest known Producer Letter Live schedule.
#[poise::command(slash_command, category = "Game Info")]
pub(crate) async fn pll(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    ctx.defer().await?;

    let Some(info) = fetch_pll_info().await? else {
        ctx.send(poise::CreateReply::default().embed(create_pll_error_embed()))
            .await?;
        return Ok(());
    };

    let mut reply = poise::CreateReply::default().embed(build_pll_embed(&info));
    let buttons = build_pll_stream_buttons(&info);
    if !buttons.is_empty() {
        reply = reply.components(buttons);
    }

    ctx.send(reply).await?;
    Ok(())
}
