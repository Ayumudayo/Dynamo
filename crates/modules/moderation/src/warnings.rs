use std::cmp::Reverse;

use chrono::{Duration as ChronoDuration, Utc};
use dynamo_runtime_api::{Context, Error};
use poise::{
    CreateReply,
    serenity_prelude::{CreateEmbed, CreateEmbedFooter, Permissions, Timestamp, User},
};
use tracing::warn;

use crate::{
    modlog::send_modlog,
    module::DEFAULT_TIMEOUT_HOURS,
    settings::{MaxWarnAction, load_settings},
    target::{
        add_warning_log, authorize_warning_history_clear, ensure_actor_permission_for_left_member,
        ensure_bot_escalation_allowed, ensure_moderatable, is_unknown_member_error,
        moderation_target_facts, require_author_member,
    },
};

const MAX_LISTED_WARNINGS: usize = 25;

/// Issue a warning to a guild member.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
pub(crate) async fn warn(
    ctx: Context<'_>,
    #[description = "Member to warn"] user: User,
    #[description = "Optional warning reason"] reason: Option<String>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let mut target = guild_id.member(ctx, user.id).await?;
    let issuer = require_author_member(ctx).await?;
    ensure_moderatable(ctx, &issuer, &target, Permissions::MODERATE_MEMBERS).await?;
    add_warning_log(ctx, target.user.id, reason.clone()).await?;
    let escalation_note = maybe_apply_max_warn(ctx, &mut target).await;
    send_modlog(ctx, "WARN", &target.user, reason.as_deref()).await?;
    let reply = match escalation_note {
        Some(note) => format!("{} is warned! {note}", target.user.name),
        None => format!("{} is warned!", target.user.name),
    };
    ctx.send(CreateReply::default().content(reply).ephemeral(true))
        .await?;
    Ok(())
}

/// Manage the stored warning history for a guild member.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    subcommands("warnings_list", "warnings_clear"),
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
pub(crate) async fn warnings(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// List the warnings currently stored for a guild member.
#[poise::command(
    slash_command,
    guild_only,
    rename = "list",
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
async fn warnings_list(
    ctx: Context<'_>,
    #[description = "User to inspect"] user: User,
) -> Result<(), Error> {
    let Some(repo) = ctx.data().persistence.warning_logs.clone() else {
        ctx.send(
            CreateReply::default()
                .content("Warning log repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let mut logs = repo.list_for_member(guild_id.get(), user.id.get()).await?;
    if logs.is_empty() {
        ctx.send(
            CreateReply::default()
                .content(format!("{} has no warnings.", user.name))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    logs.sort_by_key(|log| Reverse(log.created_at));
    let total_count = logs.len();
    let mut embed = CreateEmbed::new()
        .title(format!("{}'s warnings ({total_count})", user.name))
        .footer(CreateEmbedFooter::new(format!(
            "Showing latest {} of {total_count} warning(s)",
            total_count.min(MAX_LISTED_WARNINGS)
        )));
    for (index, log) in logs.iter().take(MAX_LISTED_WARNINGS).enumerate() {
        let mut reason = log
            .reason
            .as_deref()
            .unwrap_or("No reason provided")
            .to_string();
        if reason.chars().count() > 200 {
            reason = reason.chars().take(197).collect::<String>() + "...";
        }
        embed = embed.field(
            format!("#{} • {}", index + 1, log.created_at.format("%Y-%m-%d")),
            format!("{reason} [by {}]", log.admin_tag),
            false,
        );
    }
    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

/// Clear all stored warnings for a guild member.
#[poise::command(
    slash_command,
    guild_only,
    rename = "clear",
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
async fn warnings_clear(
    ctx: Context<'_>,
    #[description = "User whose warnings should be cleared"] user: User,
) -> Result<(), Error> {
    let Some(repo) = ctx.data().persistence.warning_logs.clone() else {
        ctx.send(
            CreateReply::default()
                .content("Warning log repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let issuer = require_author_member(ctx).await?;
    match guild_id.member(ctx, user.id).await {
        Ok(target) => {
            let facts = moderation_target_facts(ctx, &issuer, &target).await?;
            let clear = authorize_warning_history_clear(&facts, || {
                repo.clear_for_member(guild_id.get(), user.id.get())
            })?;
            clear.await?;
        }
        Err(error) if is_unknown_member_error(&error) => {
            ensure_actor_permission_for_left_member(
                ctx,
                &issuer,
                user.id,
                Permissions::MODERATE_MEMBERS,
            )
            .await?;
            repo.clear_for_member(guild_id.get(), user.id.get()).await?;
        }
        Err(error) => return Err(error.into()),
    }
    ctx.send(
        CreateReply::default()
            .content(format!("{}'s warnings have been cleared.", user.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

async fn maybe_apply_max_warn(
    ctx: Context<'_>,
    target: &mut poise::serenity_prelude::Member,
) -> Option<String> {
    let settings = match load_settings(ctx).await {
        Ok(settings) => settings,
        Err(error) => {
            warn!(?error, "failed to load settings for max-warn check");
            return None;
        }
    };
    let limit = settings.max_warn.limit as usize;
    if limit == 0 {
        return None;
    }
    let repo = ctx.data().persistence.warning_logs.clone()?;
    let guild_id = ctx.guild_id()?;
    let warnings = match repo
        .list_for_member(guild_id.get(), target.user.id.get())
        .await
    {
        Ok(warnings) => warnings,
        Err(error) => {
            warn!(?error, "failed to list warnings for max-warn check");
            return None;
        }
    };
    if warnings.is_empty() || !warnings.len().is_multiple_of(limit) {
        return None;
    }

    let auto_reason = format!("Max warnings reached ({})", warnings.len());
    let (action_label, required_perm) = match settings.max_warn.action {
        MaxWarnAction::Timeout => ("TIMEOUT", Permissions::MODERATE_MEMBERS),
        MaxWarnAction::Kick => ("KICK", Permissions::KICK_MEMBERS),
        MaxWarnAction::Ban => ("BAN", Permissions::BAN_MEMBERS),
    };
    if let Err(error) = ensure_bot_escalation_allowed(ctx, target, required_perm).await {
        warn!(
            guild_id = guild_id.get(),
            target_id = target.user.id.get(),
            ?error,
            "bot cannot execute max-warn escalation"
        );
        return Some(format!(
            "(Auto-escalation `{action_label}` skipped: {error})"
        ));
    }

    let action_result: Result<(), Error> = match settings.max_warn.action {
        MaxWarnAction::Timeout => {
            let timestamp = Timestamp::from_unix_timestamp(
                (Utc::now() + ChronoDuration::hours(DEFAULT_TIMEOUT_HOURS)).timestamp(),
            )
            .map_err(anyhow::Error::new);
            match timestamp {
                Ok(ts) => target
                    .disable_communication_until_datetime(ctx, ts)
                    .await
                    .map_err(Into::into),
                Err(err) => Err(err),
            }
        }
        MaxWarnAction::Kick => target
            .kick_with_reason(ctx, &auto_reason)
            .await
            .map_err(Into::into),
        MaxWarnAction::Ban => guild_id
            .ban_with_reason(ctx, target.user.id, 0, &auto_reason)
            .await
            .map_err(Into::into),
    };

    if let Err(error) = action_result {
        warn!(
            guild_id = guild_id.get(),
            target_id = target.user.id.get(),
            action = action_label,
            ?error,
            "failed to apply max-warn escalation"
        );
        return Some(format!("(Auto-escalation `{action_label}` failed.)"));
    }

    let _ = send_modlog(ctx, action_label, &target.user, Some(&auto_reason)).await;
    Some(format!("(Auto-escalation `{action_label}` applied.)"))
}
