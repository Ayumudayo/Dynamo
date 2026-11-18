use chrono::{Duration as ChronoDuration, Utc};
use dynamo_runtime_api::{Context, Error};
use poise::{
    CreateReply,
    serenity_prelude::{EditMember, Permissions, Timestamp, User},
};

use crate::{
    modlog::send_modlog,
    target::{ensure_moderatable, require_author_member},
};

const MAX_TIMEOUT_SECONDS: u64 = 28 * 24 * 60 * 60;
const MAX_NICKNAME_CHARS: usize = 32;

/// Timeout a guild member for a specific duration.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
pub(crate) async fn timeout(
    ctx: Context<'_>,
    #[description = "Member to timeout"] user: User,
    #[description = "Duration like 1h, 30m, 2d"] duration: String,
    #[description = "Optional timeout reason"] reason: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let mut target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::MODERATE_MEMBERS).await?;
    let duration = humantime::parse_duration(&duration)
        .map_err(|error| anyhow::anyhow!("Invalid duration: {error}"))?;
    let secs = duration.as_secs();
    anyhow::ensure!(
        (1..=MAX_TIMEOUT_SECONDS).contains(&secs),
        "Timeout duration must be between 1s and 28d."
    );
    let expires_at = Utc::now() + ChronoDuration::from_std(duration)?;
    let timestamp = Timestamp::from_unix_timestamp(expires_at.timestamp())?;
    target
        .disable_communication_until_datetime(ctx, timestamp)
        .await?;
    send_modlog(ctx, "TIMEOUT", &target.user, reason.as_deref()).await?;
    ctx.send(
        CreateReply::default()
            .content(format!("{} is timed out!", target.user.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Remove an active timeout from a guild member.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
pub(crate) async fn untimeout(
    ctx: Context<'_>,
    #[description = "Member to remove timeout from"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let mut target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::MODERATE_MEMBERS).await?;
    target.enable_communication(ctx).await?;
    send_modlog(ctx, "UNTIMEOUT", &target.user, reason.as_deref()).await?;
    ctx.send(
        CreateReply::default()
            .content(format!("Timeout of {} is removed!", target.user.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Kick a guild member from the server.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    default_member_permissions = "KICK_MEMBERS",
    required_permissions = "KICK_MEMBERS"
)]
pub(crate) async fn kick(
    ctx: Context<'_>,
    #[description = "Member to kick"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
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
    ctx.send(
        CreateReply::default()
            .content(format!("{} is kicked!", target.user.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Change or clear a guild member's nickname.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    default_member_permissions = "MANAGE_NICKNAMES",
    required_permissions = "MANAGE_NICKNAMES"
)]
pub(crate) async fn nick(
    ctx: Context<'_>,
    #[description = "Member whose nickname to change"] user: User,
    #[description = "Optional nickname; omit to reset"] name: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    if let Some(ref nickname) = name {
        anyhow::ensure!(
            nickname.chars().count() <= MAX_NICKNAME_CHARS,
            "Nickname must be {MAX_NICKNAME_CHARS} characters or fewer."
        );
    }
    let mut target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::MANAGE_NICKNAMES).await?;
    let builder = EditMember::new().nickname(name.clone().unwrap_or_default());
    target.edit(ctx, builder).await?;
    let modlog_reason = match name.as_deref() {
        Some(nickname) => format!("Changed to `{nickname}`"),
        None => "Cleared nickname".to_string(),
    };
    send_modlog(ctx, "NICK", &target.user, Some(&modlog_reason)).await?;
    ctx.send(
        CreateReply::default()
            .content(format!(
                "Successfully {} nickname of {}.",
                if name.is_some() { "changed" } else { "reset" },
                target.user.name
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}
