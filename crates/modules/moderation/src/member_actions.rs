use chrono::{Duration as ChronoDuration, Utc};
use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{EditMember, Permissions, Timestamp, User};

use crate::{
    access::ensure_module_access,
    modlog::send_modlog,
    target::{ensure_moderatable, require_author_member},
};

/// Timeout a guild member for a specific duration.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "MODERATE_MEMBERS"
)]
pub(crate) async fn timeout(
    ctx: Context<'_>,
    #[description = "Member to timeout"] user: User,
    #[description = "Duration like 1h, 30m, 2d"] duration: String,
    #[description = "Optional timeout reason"] reason: Option<String>,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let mut target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::MODERATE_MEMBERS).await?;
    let duration = humantime::parse_duration(&duration)
        .map_err(|error| anyhow::anyhow!("Invalid duration: {error}"))?;
    let expires_at = Utc::now() + ChronoDuration::from_std(duration)?;
    let timestamp = Timestamp::from_unix_timestamp(expires_at.timestamp())?;
    target
        .disable_communication_until_datetime(ctx, timestamp)
        .await?;
    send_modlog(ctx, "TIMEOUT", &target.user, reason.as_deref()).await?;
    ctx.say(format!("{} is timed out!", target.user.name))
        .await?;
    Ok(())
}

/// Remove an active timeout from a guild member.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "MODERATE_MEMBERS"
)]
pub(crate) async fn untimeout(
    ctx: Context<'_>,
    #[description = "Member to remove timeout from"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let mut target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::MODERATE_MEMBERS).await?;
    target.enable_communication(ctx).await?;
    send_modlog(ctx, "UNTIMEOUT", &target.user, reason.as_deref()).await?;
    ctx.say(format!("Timeout of {} is removed!", target.user.name))
        .await?;
    Ok(())
}

/// Kick a guild member from the server.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "KICK_MEMBERS"
)]
pub(crate) async fn kick(
    ctx: Context<'_>,
    #[description = "Member to kick"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::KICK_MEMBERS).await?;
    target
        .kick_with_reason(ctx, reason.as_deref().unwrap_or(""))
        .await?;
    send_modlog(ctx, "KICK", &target.user, reason.as_deref()).await?;
    ctx.say(format!("{} is kicked!", target.user.name)).await?;
    Ok(())
}

/// Change or clear a guild member's nickname.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "MANAGE_NICKNAMES"
)]
pub(crate) async fn nick(
    ctx: Context<'_>,
    #[description = "Member whose nickname to change"] user: User,
    #[description = "Optional nickname; omit to reset"] name: Option<String>,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let mut target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::MANAGE_NICKNAMES).await?;
    let builder = EditMember::new().nickname(name.clone().unwrap_or_default());
    target.edit(ctx, builder).await?;
    send_modlog(ctx, "NICK", &target.user, None).await?;
    ctx.say(format!(
        "Successfully {} nickname of {}.",
        if name.is_some() { "changed" } else { "reset" },
        target.user.name
    ))
    .await?;
    Ok(())
}
