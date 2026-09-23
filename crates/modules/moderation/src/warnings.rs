use chrono::{Duration as ChronoDuration, Utc};
use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{CreateEmbed, Permissions, Timestamp, User};

use crate::{
    access::ensure_module_access,
    modlog::send_modlog,
    module::DEFAULT_TIMEOUT_HOURS,
    settings::{MaxWarnAction, load_settings},
    target::{
        add_warning_log, authorize_warning_history_clear, ensure_moderatable,
        moderation_target_facts, require_author_member,
    },
};

/// Issue a warning to a guild member.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "KICK_MEMBERS"
)]
pub(crate) async fn warn(
    ctx: Context<'_>,
    #[description = "Member to warn"] user: User,
    #[description = "Optional warning reason"] reason: Option<String>,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let mut target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::KICK_MEMBERS).await?;
    add_warning_log(ctx, target.user.id, reason.clone()).await?;
    maybe_apply_max_warn(ctx, &issuer, &mut target).await?;
    send_modlog(ctx, "WARN", &target.user, reason.as_deref()).await?;
    ctx.say(format!("{} is warned!", target.user.name)).await?;
    Ok(())
}

/// Manage the stored warning history for a guild member.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    subcommands("warnings_list", "warnings_clear"),
    required_permissions = "KICK_MEMBERS"
)]
pub(crate) async fn warnings(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// List the warnings currently stored for a guild member.
#[poise::command(
    slash_command,
    guild_only,
    rename = "list",
    required_permissions = "KICK_MEMBERS"
)]
async fn warnings_list(
    ctx: Context<'_>,
    #[description = "Member to inspect"] user: User,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(repo) = ctx.data().persistence.warning_logs.clone() else {
        ctx.say("Warning log repository is not configured.").await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let logs = repo.list_for_member(guild_id.get(), user.id.get()).await?;
    if logs.is_empty() {
        ctx.say(format!("{} has no warnings.", user.name)).await?;
        return Ok(());
    }
    let description = logs
        .iter()
        .enumerate()
        .map(|(index, log)| {
            format!(
                "{}. {} [by {}]",
                index + 1,
                log.reason.as_deref().unwrap_or("No reason provided"),
                log.admin_tag
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let embed = CreateEmbed::new()
        .title(format!("{}'s warnings", user.name))
        .description(description);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Clear all stored warnings for a guild member.
#[poise::command(
    slash_command,
    guild_only,
    rename = "clear",
    required_permissions = "KICK_MEMBERS"
)]
async fn warnings_clear(
    ctx: Context<'_>,
    #[description = "Member whose warnings should be cleared"] user: User,
) -> Result<(), Error> {
    if !ensure_module_access(ctx).await? {
        return Ok(());
    }
    let Some(repo) = ctx.data().persistence.warning_logs.clone() else {
        ctx.say("Warning log repository is not configured.").await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    let facts = moderation_target_facts(ctx, &issuer, &target)?;
    let clear = authorize_warning_history_clear(&facts, || {
        repo.clear_for_member(guild_id.get(), user.id.get())
    })?;
    clear.await?;
    ctx.say(format!("{}'s warnings have been cleared.", user.name))
        .await?;
    Ok(())
}

async fn maybe_apply_max_warn(
    ctx: Context<'_>,
    issuer: &poise::serenity_prelude::Member,
    target: &mut poise::serenity_prelude::Member,
) -> Result<(), Error> {
    let settings = load_settings(ctx).await?;
    if settings.max_warn.limit == 0 {
        return Ok(());
    }
    let Some(repo) = ctx.data().persistence.warning_logs.clone() else {
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let warnings = repo
        .list_for_member(guild_id.get(), target.user.id.get())
        .await?;
    if warnings.len() < settings.max_warn.limit as usize {
        return Ok(());
    }

    let auto_reason = "Max warnings reached";
    match settings.max_warn.action {
        MaxWarnAction::Timeout => {
            ensure_moderatable(ctx, issuer, target, Permissions::MODERATE_MEMBERS).await?;
            let timestamp = Timestamp::from_unix_timestamp(
                (Utc::now() + ChronoDuration::hours(DEFAULT_TIMEOUT_HOURS)).timestamp(),
            )?;
            target
                .disable_communication_until_datetime(ctx, timestamp)
                .await?;
            send_modlog(ctx, "TIMEOUT", &target.user, Some(auto_reason)).await?;
        }
        MaxWarnAction::Kick => {
            ensure_moderatable(ctx, issuer, target, Permissions::KICK_MEMBERS).await?;
            target.kick_with_reason(ctx, auto_reason).await?;
            send_modlog(ctx, "KICK", &target.user, Some(auto_reason)).await?;
        }
        MaxWarnAction::Ban => {
            ensure_moderatable(ctx, issuer, target, Permissions::BAN_MEMBERS).await?;
            guild_id
                .ban_with_reason(ctx, target.user.id, 0, auto_reason)
                .await?;
            send_modlog(ctx, "BAN", &target.user, Some(auto_reason)).await?;
        }
    }
    repo.clear_for_member(guild_id.get(), target.user.id.get())
        .await?;
    Ok(())
}
