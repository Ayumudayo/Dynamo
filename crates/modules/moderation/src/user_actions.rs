use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{Permissions, User, UserId};

use crate::{
    access::ensure_module_access,
    modlog::send_modlog,
    target::{ensure_moderatable, require_author_member},
};

/// Ban a user from the server.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "BAN_MEMBERS"
)]
pub(crate) async fn ban(
    ctx: Context<'_>,
    #[description = "User to ban"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    if let Ok(target_member) = guild_id.member(ctx, user.id).await {
        let issuer = require_author_member(ctx).await?;
        ensure_moderatable(ctx, &issuer, &target_member, Permissions::BAN_MEMBERS).await?;
    }
    guild_id
        .ban_with_reason(ctx, user.id, 0, reason.as_deref().unwrap_or(""))
        .await?;
    send_modlog(ctx, "BAN", &user, reason.as_deref()).await?;
    ctx.say(format!("{} is banned!", user.name)).await?;
    Ok(())
}

/// Unban a user by their Discord user ID.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "BAN_MEMBERS"
)]
pub(crate) async fn unban(
    ctx: Context<'_>,
    #[description = "User ID to unban"] user_id: String,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let user_id = parse_user_id(&user_id)?;
    guild_id.unban(ctx, user_id).await?;
    let user = user_id
        .to_user(ctx)
        .await
        .unwrap_or_else(|_| fallback_user(user_id));
    send_modlog(ctx, "UNBAN", &user, reason.as_deref()).await?;
    ctx.say(format!("{} is unbanned!", user.name)).await?;
    Ok(())
}

/// Ban and immediately unban a user to remove recent messages.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "BAN_MEMBERS"
)]
pub(crate) async fn softban(
    ctx: Context<'_>,
    #[description = "Member to softban"] user: User,
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
    ensure_moderatable(ctx, &issuer, &target, Permissions::BAN_MEMBERS).await?;
    guild_id
        .ban_with_reason(ctx, user.id, 7, reason.as_deref().unwrap_or(""))
        .await?;
    guild_id.unban(ctx, user.id).await?;
    send_modlog(ctx, "SOFTBAN", &target.user, reason.as_deref()).await?;
    ctx.say(format!("{} is soft-banned!", target.user.name))
        .await?;
    Ok(())
}

pub(crate) fn parse_user_id(input: &str) -> Result<UserId, Error> {
    let trimmed = input
        .trim()
        .trim_start_matches("<@")
        .trim_start_matches('!')
        .trim_end_matches('>');
    Ok(UserId::new(trimmed.parse::<u64>().map_err(|error| {
        anyhow::anyhow!("Invalid user id `{input}`: {error}")
    })?))
}

fn fallback_user(user_id: UserId) -> User {
    let mut user = User::default();
    user.id = user_id;
    user.name = user_id.to_string();
    user
}
