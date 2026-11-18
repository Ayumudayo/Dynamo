use dynamo_domain_moderation::WarningLogRecord;
use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{
    Error as SerenityError, Guild, HttpError, Member, Permissions, UserId,
};

use crate::policy::{ModerationTargetDenial, ModerationTargetFacts, authorize_moderation_target};

pub(crate) fn is_unknown_member_error(error: &SerenityError) -> bool {
    matches!(
        error,
        SerenityError::Http(HttpError::UnsuccessfulRequest(resp)) if resp.error.code == 10007
    )
}

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
    let facts = moderation_target_facts(ctx, issuer, target).await?;
    authorize_moderation_target(&facts, required_permission).map_err(anyhow::Error::new)
}

pub(crate) fn authorize_warning_history_clear<T>(
    facts: &ModerationTargetFacts,
    clear_for_member: impl FnOnce() -> T,
) -> Result<T, Error> {
    authorize_moderation_target(facts, Permissions::MODERATE_MEMBERS)
        .map_err(anyhow::Error::new)?;
    Ok(clear_for_member())
}

async fn cached_or_fetch_bot_member(ctx: Context<'_>) -> Result<Member, Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild id missing"))?;
    let bot_id = ctx.serenity_context().cache.current_user().id;
    if let Some(member) = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .and_then(|guild| guild.members.get(&bot_id).cloned())
    {
        return Ok(member);
    }
    Ok(guild_id
        .member(&ctx.serenity_context().http, bot_id)
        .await?)
}

fn resolve_member_permissions(guild: &Guild, member: &Member) -> Permissions {
    member
        .permissions
        .unwrap_or_else(|| guild.member_permissions(member))
}

pub(crate) async fn ensure_actor_and_bot_guild_permissions(
    ctx: Context<'_>,
    issuer: &Member,
    target_id: UserId,
    required_permission: Permissions,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild id missing"))?;
    let bot_member = cached_or_fetch_bot_member(ctx).await?;
    let (actor_permissions, bot_permissions, guild_owner_id) = {
        let guild = ctx
            .serenity_context()
            .cache
            .guild(guild_id)
            .ok_or_else(|| anyhow::anyhow!("guild cache entry missing"))?;
        (
            resolve_member_permissions(&guild, issuer),
            guild.member_permissions(&bot_member),
            guild.owner_id,
        )
    };
    if !actor_permissions.contains(required_permission) {
        return Err(ModerationTargetDenial::ActorPermission.into());
    }
    if !bot_permissions.contains(required_permission) {
        return Err(ModerationTargetDenial::BotPermission.into());
    }
    if target_id == issuer.user.id {
        return Err(ModerationTargetDenial::SelfTarget.into());
    }
    if target_id == bot_member.user.id {
        return Err(ModerationTargetDenial::BotTarget.into());
    }
    if target_id == guild_owner_id {
        return Err(ModerationTargetDenial::GuildOwnerTarget.into());
    }
    Ok(())
}

pub(crate) async fn ensure_actor_permission_for_left_member(
    ctx: Context<'_>,
    issuer: &Member,
    target_id: UserId,
    required_permission: Permissions,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild id missing"))?;
    let actor_permissions = {
        let guild = ctx
            .serenity_context()
            .cache
            .guild(guild_id)
            .ok_or_else(|| anyhow::anyhow!("guild cache entry missing"))?;
        resolve_member_permissions(&guild, issuer)
    };
    if !actor_permissions.contains(required_permission) {
        return Err(ModerationTargetDenial::ActorPermission.into());
    }
    if target_id == issuer.user.id {
        return Err(ModerationTargetDenial::SelfTarget.into());
    }
    Ok(())
}

pub(crate) async fn ensure_bot_escalation_allowed(
    ctx: Context<'_>,
    target: &Member,
    required_permission: Permissions,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild id missing"))?;
    let bot_member = cached_or_fetch_bot_member(ctx).await?;
    let (bot_permissions, bot_top_role, target_top_role, guild_owner_id) = {
        let guild = ctx
            .serenity_context()
            .cache
            .guild(guild_id)
            .ok_or_else(|| anyhow::anyhow!("guild cache entry missing"))?;
        (
            guild.member_permissions(&bot_member),
            highest_role_position(&guild, &bot_member),
            highest_role_position(&guild, target),
            guild.owner_id,
        )
    };
    if !bot_permissions.contains(required_permission) {
        return Err(ModerationTargetDenial::BotPermission.into());
    }
    if target.user.id == bot_member.user.id {
        return Err(ModerationTargetDenial::BotTarget.into());
    }
    if target.user.id == guild_owner_id {
        return Err(ModerationTargetDenial::GuildOwnerTarget.into());
    }
    if bot_member.user.id != guild_owner_id && bot_top_role <= target_top_role {
        return Err(ModerationTargetDenial::BotHierarchy.into());
    }
    Ok(())
}

pub(crate) async fn moderation_target_facts(
    ctx: Context<'_>,
    issuer: &Member,
    target: &Member,
) -> Result<ModerationTargetFacts, Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild id missing"))?;
    let bot_member = cached_or_fetch_bot_member(ctx).await?;
    let guild = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .ok_or_else(|| anyhow::anyhow!("guild cache entry missing"))?;
    Ok(ModerationTargetFacts {
        actor_id: issuer.user.id,
        actor_permissions: resolve_member_permissions(&guild, issuer),
        actor_top_role: highest_role_position(&guild, issuer),
        bot_id: bot_member.user.id,
        bot_permissions: guild.member_permissions(&bot_member),
        bot_top_role: highest_role_position(&guild, &bot_member),
        target_id: target.user.id,
        target_top_role: highest_role_position(&guild, target),
        guild_owner_id: guild.owner_id,
    })
}

fn highest_role_position(guild: &Guild, member: &Member) -> i64 {
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
