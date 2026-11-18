use std::num::NonZeroU64;

use dynamo_runtime_api::{Context, Error};
use poise::{
    CreateReply,
    serenity_prelude::{Permissions, User, UserId},
};

use crate::{
    modlog::send_modlog,
    target::{
        ensure_actor_and_bot_guild_permissions, ensure_moderatable, is_unknown_member_error,
        require_author_member,
    },
};

/// Ban a user from the server.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    default_member_permissions = "BAN_MEMBERS",
    required_permissions = "BAN_MEMBERS"
)]
pub(crate) async fn ban(
    ctx: Context<'_>,
    #[description = "User to ban"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let issuer = require_author_member(ctx).await?;
    match guild_id.member(ctx, user.id).await {
        Ok(target_member) => {
            ensure_moderatable(ctx, &issuer, &target_member, Permissions::BAN_MEMBERS).await?;
        }
        Err(error) if is_unknown_member_error(&error) => {
            ensure_actor_and_bot_guild_permissions(ctx, &issuer, user.id, Permissions::BAN_MEMBERS)
                .await?;
        }
        Err(error) => return Err(error.into()),
    }
    guild_id
        .ban_with_reason(ctx, user.id, 0, reason.as_deref().unwrap_or(""))
        .await?;
    send_modlog(ctx, "BAN", &user, reason.as_deref()).await?;
    ctx.send(
        CreateReply::default()
            .content(format!("{} is banned!", user.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Unban a user by their Discord user ID.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    default_member_permissions = "BAN_MEMBERS",
    required_permissions = "BAN_MEMBERS"
)]
pub(crate) async fn unban(
    ctx: Context<'_>,
    #[description = "User ID to unban"] user_id: String,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let user_id = parse_user_id(&user_id)?;
    let issuer = require_author_member(ctx).await?;
    ensure_actor_and_bot_guild_permissions(ctx, &issuer, user_id, Permissions::BAN_MEMBERS).await?;
    guild_id.unban(ctx, user_id).await?;
    let user = user_id
        .to_user(ctx)
        .await
        .unwrap_or_else(|_| fallback_user(user_id));
    send_modlog(ctx, "UNBAN", &user, reason.as_deref()).await?;
    ctx.send(
        CreateReply::default()
            .content(format!("{} is unbanned!", user.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Ban and immediately unban a user to remove recent messages.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    default_member_permissions = "BAN_MEMBERS",
    required_permissions = "BAN_MEMBERS"
)]
pub(crate) async fn softban(
    ctx: Context<'_>,
    #[description = "Member to softban"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
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
    ctx.send(
        CreateReply::default()
            .content(format!("{} is soft-banned!", target.user.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

pub(crate) fn parse_user_id(input: &str) -> Result<UserId, Error> {
    let trimmed = input
        .trim()
        .trim_start_matches("<@")
        .trim_start_matches('!')
        .trim_end_matches('>');
    let parsed = trimmed
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("Invalid user id `{input}`: {error}"))?;
    let non_zero = NonZeroU64::new(parsed)
        .ok_or_else(|| anyhow::anyhow!("Invalid user id `{input}`: id cannot be zero"))?;
    Ok(UserId::from(non_zero))
}

fn fallback_user(user_id: UserId) -> User {
    let mut user = User::default();
    user.id = user_id;
    user.name = user_id.to_string();
    user
}
