use dynamo_domain_moderation::WarningLogRecord;
use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{Member, Permissions, UserId};

use crate::policy::{ModerationTargetFacts, authorize_moderation_target};

pub(crate) async fn require_author_member(ctx: Context<'_>) -> Result<Member, Error> {
    ctx.author_member()
        .await
        .map(|member| member.into_owned())
        .ok_or_else(|| anyhow::anyhow!("member missing"))
}

pub(crate) async fn ensure_moderatable(
    ctx: Context<'_>,
    issuer: &Member,
    target: &Member,
    required_permission: Permissions,
) -> Result<(), Error> {
    let facts = moderation_target_facts(ctx, issuer, target)?;
    authorize_moderation_target(&facts, required_permission).map_err(anyhow::Error::new)
}

pub(crate) fn authorize_warning_history_clear<T>(
    facts: &ModerationTargetFacts,
    clear_for_member: impl FnOnce() -> T,
) -> Result<T, Error> {
    authorize_moderation_target(facts, Permissions::KICK_MEMBERS).map_err(anyhow::Error::new)?;
    Ok(clear_for_member())
}

pub(crate) fn moderation_target_facts(
    ctx: Context<'_>,
    issuer: &Member,
    target: &Member,
) -> Result<ModerationTargetFacts, Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild id missing"))?;
    let guild = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .ok_or_else(|| anyhow::anyhow!("guild cache entry missing"))?;
    let bot_member = guild
        .members
        .get(&ctx.serenity_context().cache.current_user().id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("bot member cache entry missing"))?;
    Ok(ModerationTargetFacts {
        actor_id: issuer.user.id,
        actor_permissions: issuer.permissions.unwrap_or_else(Permissions::empty),
        actor_top_role: highest_role_position(&guild, issuer),
        bot_id: bot_member.user.id,
        bot_permissions: bot_member.permissions.unwrap_or_else(Permissions::empty),
        bot_top_role: highest_role_position(&guild, &bot_member),
        target_id: target.user.id,
        target_is_bot: target.user.bot,
        target_top_role: highest_role_position(&guild, target),
        guild_owner_id: guild.owner_id,
    })
}

fn highest_role_position(guild: &poise::serenity_prelude::Guild, member: &Member) -> i64 {
    member
        .roles
        .iter()
        .filter_map(|role_id| guild.roles.get(role_id))
        .map(|role| role.position as i64)
        .max()
        .unwrap_or(0)
}

pub(crate) async fn add_warning_log(
    ctx: Context<'_>,
    member_id: UserId,
    reason: Option<String>,
) -> Result<WarningLogRecord, Error> {
    let repo = ctx
        .data()
        .persistence
        .warning_logs
        .clone()
        .ok_or_else(|| anyhow::anyhow!("warning log repository is not configured"))?;
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild id missing"))?;
    repo.add(WarningLogRecord {
        guild_id: guild_id.get(),
        member_id: member_id.get(),
        reason,
        admin_id: ctx.author().id.get(),
        admin_tag: ctx.author().tag(),
        created_at: chrono::Utc::now(),
    })
    .await
}
