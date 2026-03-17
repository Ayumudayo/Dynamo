use chrono::{Duration as ChronoDuration, Utc};
use dynamo_domain_moderation::WarningLogRecord;
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingOption,
    SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime::{AppState, Context, Error, module_access_for_context};
use poise::serenity_prelude::{
    CreateEmbed, CreateEmbedFooter, EditMember, Member, Permissions, Timestamp, User, UserId,
};
use serde::{Deserialize, Serialize};

const MODULE_ID: &str = "moderation";
const DEFAULT_TIMEOUT_HOURS: i64 = 24;

pub struct ModerationModule;

impl Module<AppState, Error> for ModerationModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Moderation",
            "Slash-first moderation commands with warning ledger support.",
            ModuleCategory::Moderation,
            true,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_MEMBERS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![
            warn(),
            warnings(),
            timeout(),
            untimeout(),
            kick(),
            ban(),
            unban(),
            softban(),
            nick(),
        ]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "moderation",
                title: "Moderation",
                description: Some("Configure modlog output and max-warning escalation."),
                fields: vec![
                    SettingsField {
                        key: "modlog_channel_id",
                        label: "Modlog channel ID",
                        help_text: Some("Optional channel for moderation action embeds."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "max_warn.limit",
                        label: "Max warning limit",
                        help_text: Some("Auto-action threshold for warnings. Set 0 to disable."),
                        required: false,
                        kind: SettingsFieldKind::Integer,
                    },
                    SettingsField {
                        key: "max_warn.action",
                        label: "Max warning action",
                        help_text: Some("Action to take when warning threshold is reached."),
                        required: false,
                        kind: SettingsFieldKind::Select {
                            options: vec![
                                SettingOption {
                                    label: "Timeout",
                                    value: "TIMEOUT",
                                },
                                SettingOption {
                                    label: "Kick",
                                    value: "KICK",
                                },
                                SettingOption {
                                    label: "Ban",
                                    value: "BAN",
                                },
                            ],
                        },
                    },
                ],
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ModerationSettings {
    #[serde(
        alias = "modlog_channel",
        alias = "modlog",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    modlog_channel_id: Option<u64>,
    max_warn: MaxWarnSettings,
}

impl Default for ModerationSettings {
    fn default() -> Self {
        Self {
            modlog_channel_id: None,
            max_warn: MaxWarnSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct MaxWarnSettings {
    limit: u64,
    action: MaxWarnAction,
}

impl Default for MaxWarnSettings {
    fn default() -> Self {
        Self {
            limit: 5,
            action: MaxWarnAction::Kick,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum MaxWarnAction {
    Timeout,
    #[default]
    Kick,
    Ban,
}

/// Issue a warning to a guild member.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "KICK_MEMBERS"
)]
async fn warn(
    ctx: Context<'_>,
    #[description = "Member to warn"] user: User,
    #[description = "Optional warning reason"] reason: Option<String>,
) -> Result<(), Error> {
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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
async fn warnings(_ctx: Context<'_>) -> Result<(), Error> {
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
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let Some(repo) = ctx.data().persistence.warning_logs.clone() else {
        ctx.say("Warning log repository is not configured.").await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    repo.clear_for_member(guild_id.get(), user.id.get()).await?;
    ctx.say(format!("{}'s warnings have been cleared.", user.name))
        .await?;
    Ok(())
}

/// Timeout a guild member for a specific duration.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "MODERATE_MEMBERS"
)]
async fn timeout(
    ctx: Context<'_>,
    #[description = "Member to timeout"] user: User,
    #[description = "Duration like 1h, 30m, 2d"] duration: String,
    #[description = "Optional timeout reason"] reason: Option<String>,
) -> Result<(), Error> {
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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
async fn untimeout(
    ctx: Context<'_>,
    #[description = "Member to remove timeout from"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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
async fn kick(
    ctx: Context<'_>,
    #[description = "Member to kick"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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

/// Ban a user from the server.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "BAN_MEMBERS"
)]
async fn ban(
    ctx: Context<'_>,
    #[description = "User to ban"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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
async fn unban(
    ctx: Context<'_>,
    #[description = "User ID to unban"] user_id: String,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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
async fn softban(
    ctx: Context<'_>,
    #[description = "Member to softban"] user: User,
    #[description = "Optional reason"] reason: Option<String>,
) -> Result<(), Error> {
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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

/// Change or clear a guild member's nickname.
#[poise::command(
    slash_command,
    guild_only,
    category = "Moderation",
    required_permissions = "MANAGE_NICKNAMES"
)]
async fn nick(
    ctx: Context<'_>,
    #[description = "Member whose nickname to change"] user: User,
    #[description = "Optional nickname; omit to reset"] name: Option<String>,
) -> Result<(), Error> {
    if let Some(reason_message) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason_message)
                .ephemeral(true),
        )
        .await?;
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

async fn load_settings(ctx: Context<'_>) -> Result<ModerationSettings, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(ModerationSettings::default());
    };
    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| serde_json::from_value::<ModerationSettings>(module.configuration.clone()))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

async fn require_author_member(ctx: Context<'_>) -> Result<Member, Error> {
    ctx.author_member()
        .await
        .map(|member| member.into_owned())
        .ok_or_else(|| anyhow::anyhow!("member missing"))
}

async fn add_warning_log(
    ctx: Context<'_>,
    member_id: UserId,
    reason: Option<String>,
) -> Result<WarningLogRecord, Error> {
    let Some(repo) = ctx.data().persistence.warning_logs.clone() else {
        return Err(anyhow::anyhow!("warning log repository is not configured"));
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Err(anyhow::anyhow!("guild id missing"));
    };
    let record = WarningLogRecord {
        guild_id: guild_id.get(),
        member_id: member_id.get(),
        reason,
        admin_id: ctx.author().id.get(),
        admin_tag: ctx.author().tag(),
        created_at: Utc::now(),
    };
    repo.add(record).await
}

async fn maybe_apply_max_warn(
    ctx: Context<'_>,
    issuer: &Member,
    target: &mut Member,
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

    let auto_reason = Some("Max warnings reached");
    match settings.max_warn.action {
        MaxWarnAction::Timeout => {
            ensure_moderatable(ctx, issuer, target, Permissions::MODERATE_MEMBERS).await?;
            let timestamp = Timestamp::from_unix_timestamp(
                (Utc::now() + ChronoDuration::hours(DEFAULT_TIMEOUT_HOURS)).timestamp(),
            )?;
            target
                .disable_communication_until_datetime(ctx, timestamp)
                .await?;
            send_modlog(ctx, "TIMEOUT", &target.user, auto_reason).await?;
        }
        MaxWarnAction::Kick => {
            ensure_moderatable(ctx, issuer, target, Permissions::KICK_MEMBERS).await?;
            target
                .kick_with_reason(ctx, auto_reason.unwrap_or(""))
                .await?;
            send_modlog(ctx, "KICK", &target.user, auto_reason).await?;
        }
        MaxWarnAction::Ban => {
            ensure_moderatable(ctx, issuer, target, Permissions::BAN_MEMBERS).await?;
            guild_id
                .ban_with_reason(ctx, target.user.id, 0, auto_reason.unwrap_or(""))
                .await?;
            send_modlog(ctx, "BAN", &target.user, auto_reason).await?;
        }
    }

    repo.clear_for_member(guild_id.get(), target.user.id.get())
        .await?;
    Ok(())
}

async fn send_modlog(
    ctx: Context<'_>,
    action: &str,
    user: &User,
    reason: Option<&str>,
) -> Result<(), Error> {
    let settings = load_settings(ctx).await?;
    let Some(channel_id) = settings.modlog_channel_id else {
        return Ok(());
    };

    let embed = CreateEmbed::new()
        .title(format!("Moderation - {action}"))
        .description(format!("{} [{}]", user.name, user.id))
        .field("Reason", reason.unwrap_or("No reason provided"), false)
        .footer(CreateEmbedFooter::new(format!(
            "By {} • {}",
            ctx.author().name,
            ctx.author().id
        )));

    poise::serenity_prelude::ChannelId::new(channel_id)
        .send_message(
            ctx,
            poise::serenity_prelude::CreateMessage::new().embed(embed),
        )
        .await?;
    Ok(())
}

async fn ensure_moderatable(
    ctx: Context<'_>,
    issuer: &Member,
    target: &Member,
    required_permission: Permissions,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Err(anyhow::anyhow!("guild id missing"));
    };
    let guild = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .ok_or_else(|| anyhow::anyhow!("guild cache entry missing"))?;

    if !issuer
        .permissions
        .unwrap_or_else(Permissions::empty)
        .contains(required_permission)
    {
        return Err(anyhow::anyhow!(
            "You do not have the required Discord permission."
        ));
    }

    let bot_member = guild
        .members
        .get(&ctx.serenity_context().cache.current_user().id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("bot member cache entry missing"))?;

    if !bot_member
        .permissions
        .unwrap_or_else(Permissions::empty)
        .contains(required_permission)
    {
        return Err(anyhow::anyhow!(
            "The bot does not have the required Discord permission."
        ));
    }

    if guild.owner_id != issuer.user.id
        && highest_role_position(&guild, issuer) <= highest_role_position(&guild, target)
    {
        return Err(anyhow::anyhow!(
            "You do not have permission to moderate this member."
        ));
    }

    if guild.owner_id != bot_member.user.id
        && highest_role_position(&guild, &bot_member) <= highest_role_position(&guild, target)
    {
        return Err(anyhow::anyhow!(
            "The bot cannot moderate this member due to role hierarchy."
        ));
    }

    Ok(())
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

fn parse_user_id(input: &str) -> Result<UserId, Error> {
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

fn deserialize_optional_snowflake<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };

    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(value) if value.trim().is_empty() => Ok(None),
        serde_json::Value::String(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("snowflake number must be an unsigned integer"))
            .map(Some),
        other => Err(serde::de::Error::custom(format!(
            "snowflake must be a string or number, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{ModerationSettings, parse_user_id};

    #[test]
    fn moderation_settings_accepts_nested_shape() {
        let settings: ModerationSettings = serde_json::from_value(serde_json::json!({
            "modlog_channel_id": "123",
            "max_warn": { "limit": 3, "action": "BAN" }
        }))
        .expect("settings");
        assert_eq!(settings.modlog_channel_id, Some(123));
        assert_eq!(settings.max_warn.limit, 3);
    }

    #[test]
    fn parses_user_id_from_mention() {
        assert_eq!(parse_user_id("<@123>").expect("user").get(), 123);
    }
}
