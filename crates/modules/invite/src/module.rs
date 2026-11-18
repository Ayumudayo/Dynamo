use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use dynamo_access::module_access_for_app;
use dynamo_domain_invite::InviteMemberRecord;
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{
    AppState, Context, Error, decode_module_configuration, deserialize_optional_snowflake,
};
use poise::{
    CreateReply,
    serenity_prelude::{
        CreateEmbed, CreateEmbedFooter, GuildId, InviteCreateEvent, InviteDeleteEvent, RichInvite,
        RoleId, User, UserId,
    },
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

const MODULE_ID: &str = "invite";
const DELETED_INVITE_WINDOW_SECONDS: i64 = 60;

#[derive(Debug, Clone)]
struct CachedInvite {
    code: String,
    uses: u64,
    max_uses: u8,
    inviter_id: Option<String>,
    deleted_timestamp: Option<i64>,
}

fn invite_cache() -> &'static RwLock<HashMap<u64, HashMap<String, CachedInvite>>> {
    static CACHE: OnceLock<RwLock<HashMap<u64, HashMap<String, CachedInvite>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn guild_invite_lock(guild_id: u64) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<StdMutex<HashMap<u64, Weak<AsyncMutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(StdMutex::default)
        .lock()
        .expect("invite lock registry poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&guild_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(guild_id, Arc::downgrade(&lock));
    lock
}

pub struct InviteModule;

impl Module<AppState, Error> for InviteModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Invite Tracking",
            "Tracks inviter attribution and reward roles for member joins and leaves.",
            ModuleCategory::Utility,
            true,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_MEMBERS | GatewayIntents::GUILD_INVITES,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![invites(), invite_leaderboard(), invite_admin()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "invite_tracking",
                title: "Invite Tracking",
                description: Some("Configure invite tracking and reward role thresholds."),
                fields: vec![
                    SettingsField {
                        key: "tracking",
                        label: "Tracking enabled",
                        help_text: Some("Enable inviter attribution for member joins and leaves."),
                        required: false,
                        kind: SettingsFieldKind::Toggle,
                    },
                    SettingsField {
                        key: "ranks",
                        label: "Reward ranks",
                        help_text: Some(
                            "Configure automatic role rewards granted when a member reaches an invite count.",
                        ),
                        required: false,
                        kind: SettingsFieldKind::InviteRanks,
                    },
                ],
            }],
        }
    }
}

/// View tracked invite statistics for yourself or another member.
#[poise::command(slash_command, guild_only, category = "Utility")]
async fn invites(
    ctx: Context<'_>,
    #[description = "Optional member to inspect"] user: Option<User>,
) -> Result<(), Error> {
    let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.invites.as_ref() else {
        ctx.send(
            CreateReply::default()
                .content(if ko {
                    "초대 저장소가 설정되어 있지 않습니다."
                } else {
                    "Invite repository is not configured."
                })
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let target = user.as_ref().unwrap_or_else(|| ctx.author());
    let record = repo
        .get_or_create(guild_id.get(), &target.id.get().to_string())
        .await?;
    let c = &record.invite_data;
    let embed = CreateEmbed::new()
        .title(if ko {
            format!("{} — 초대 현황", target.name)
        } else {
            format!("{} — Invites", target.name)
        })
        .thumbnail(target.face())
        .field(
            if ko {
                "유효 초대 수"
            } else {
                "Effective Invites"
            },
            c.effective().to_string(),
            false,
        )
        .field(
            if ko {
                "일반 (추적됨)"
            } else {
                "Regular (Tracked)"
            },
            c.tracked.to_string(),
            true,
        )
        .field(
            if ko {
                "보너스 (추가됨)"
            } else {
                "Bonus (Added)"
            },
            c.added.to_string(),
            true,
        )
        .field(if ko { "퇴장" } else { "Left" }, c.left.to_string(), true)
        .field(
            if ko { "허수 (자기 초대)" } else { "Fake" },
            c.fake.to_string(),
            true,
        );
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Show the top inviters in this server.
#[poise::command(
    slash_command,
    guild_only,
    rename = "invite-leaderboard",
    category = "Utility"
)]
async fn invite_leaderboard(ctx: Context<'_>) -> Result<(), Error> {
    let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.invites.as_ref() else {
        ctx.send(
            CreateReply::default()
                .content(if ko {
                    "초대 저장소가 설정되어 있지 않습니다."
                } else {
                    "Invite repository is not configured."
                })
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let entries = repo.leaderboard(guild_id.get(), 10).await?;
    if entries.is_empty() {
        ctx.say(if ko {
            "아직 이 서버에 기록된 초대 통계가 없습니다."
        } else {
            "No invite stats have been recorded in this server yet."
        })
        .await?;
        return Ok(());
    }
    let description = entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            if ko {
                format!(
                    "{}. <@{}> — **{}**회 초대",
                    idx + 1,
                    entry.member_id,
                    entry.invites
                )
            } else {
                format!(
                    "{}. <@{}> — **{}** invites",
                    idx + 1,
                    entry.member_id,
                    entry.invites
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let embed = CreateEmbed::new()
        .title(if ko {
            "초대 순위"
        } else {
            "Invite Leaderboard"
        })
        .description(description)
        .footer(CreateEmbedFooter::new(if ko {
            "상위 10명"
        } else {
            "Top 10 inviters"
        }));
    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Manage stored invite counts for server members.
#[poise::command(
    slash_command,
    guild_only,
    rename = "invite-admin",
    category = "Utility",
    subcommands("invite_admin_add", "invite_admin_remove", "invite_admin_reset"),
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn invite_admin(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add bonus invites to a member.
#[poise::command(
    slash_command,
    guild_only,
    rename = "add",
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn invite_admin_add(
    ctx: Context<'_>,
    #[description = "Target member"] user: User,
    #[description = "Positive number of invites to add"] amount: i64,
) -> Result<(), Error> {
    anyhow::ensure!(amount > 0, "Amount must be greater than zero.");
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.invites.as_ref() else {
        ctx.send(
            CreateReply::default()
                .content("Invite repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let mut record = repo
        .get_or_create(guild_id.get(), &user.id.get().to_string())
        .await?;
    record.invite_data.added = record.invite_data.added.saturating_add(amount as u64);
    record.updated_at = chrono::Utc::now();
    let saved = repo.save(record).await?;
    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    let _ = apply_reward_roles(ctx.serenity_context(), guild_id, &settings, &saved).await;
    ctx.send(
        CreateReply::default()
            .content(format!(
                "Added `{amount}` bonus invites to {}. Effective invites: `{}`.",
                user.name,
                saved.invite_data.effective()
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Deduct invites from a member.
#[poise::command(
    slash_command,
    guild_only,
    rename = "remove",
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn invite_admin_remove(
    ctx: Context<'_>,
    #[description = "Target member"] user: User,
    #[description = "Positive number of invites to deduct"] amount: i64,
) -> Result<(), Error> {
    anyhow::ensure!(amount > 0, "Amount must be greater than zero.");
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.invites.as_ref() else {
        ctx.send(
            CreateReply::default()
                .content("Invite repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let mut record = repo
        .get_or_create(guild_id.get(), &user.id.get().to_string())
        .await?;
    let delta = amount as u64;
    if record.invite_data.added >= delta {
        record.invite_data.added -= delta;
    } else {
        let remainder = delta - record.invite_data.added;
        record.invite_data.added = 0;
        record.invite_data.left = record.invite_data.left.saturating_add(remainder);
    }
    record.updated_at = chrono::Utc::now();
    let saved = repo.save(record).await?;
    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    let _ = apply_reward_roles(ctx.serenity_context(), guild_id, &settings, &saved).await;
    ctx.send(
        CreateReply::default()
            .content(format!(
                "Removed `{amount}` invites from {}. Effective invites: `{}`.",
                user.name,
                saved.invite_data.effective()
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Reset all tracked invite counters for a member.
#[poise::command(
    slash_command,
    guild_only,
    rename = "reset",
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn invite_admin_reset(
    ctx: Context<'_>,
    #[description = "Target member"] user: User,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.invites.as_ref() else {
        ctx.send(
            CreateReply::default()
                .content("Invite repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let mut record = repo
        .get_or_create(guild_id.get(), &user.id.get().to_string())
        .await?;
    record.invite_data = Default::default();
    record.updated_at = chrono::Utc::now();
    let saved = repo.save(record).await?;
    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    let _ = apply_reward_roles(ctx.serenity_context(), guild_id, &settings, &saved).await;
    ctx.send(
        CreateReply::default()
            .content(format!("Reset invite counters for {}.", user.name))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct InviteSettings {
    tracking: bool,
    ranks: Vec<InviteRewardRank>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct InviteRewardRank {
    invites: u64,
    #[serde(
        alias = "_id",
        alias = "role_id",
        deserialize_with = "deserialize_reward_role_id"
    )]
    role_id: u64,
}

fn deserialize_reward_role_id<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(deserialize_optional_snowflake(deserializer)?.unwrap_or(0))
}

pub async fn evict_guild_cache(guild_id: GuildId) {
    invite_cache().write().await.remove(&guild_id.get());
}

pub async fn preload_guild_cache(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    guild_id: GuildId,
) -> Result<(), Error> {
    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(());
    }

    let settings = load_settings(data, guild_id.get()).await?;
    if !settings.tracking {
        return Ok(());
    }

    let _ = cache_guild_invites(ctx, guild_id).await?;
    Ok(())
}

pub async fn handle_invite_create(
    _ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    event: &InviteCreateEvent,
) -> Result<(), Error> {
    let Some(guild_id) = event.guild_id else {
        return Ok(());
    };

    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(());
    }

    let mut cache = invite_cache().write().await;
    let guild_cache = cache.entry(guild_id.get()).or_default();
    guild_cache.insert(
        event.code.clone(),
        CachedInvite {
            code: event.code.clone(),
            uses: event.uses,
            max_uses: event.max_uses,
            inviter_id: event.inviter.as_ref().map(|user| user.id.get().to_string()),
            deleted_timestamp: None,
        },
    );

    Ok(())
}

pub async fn handle_invite_delete(
    _ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    event: &InviteDeleteEvent,
) -> Result<(), Error> {
    let Some(guild_id) = event.guild_id else {
        return Ok(());
    };

    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(());
    }

    let mut cache = invite_cache().write().await;
    if let Some(guild_cache) = cache.get_mut(&guild_id.get())
        && let Some(invite) = guild_cache.get_mut(&event.code)
    {
        invite.deleted_timestamp = Some(now_unix());
    }

    Ok(())
}

pub async fn track_joined_member(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    member: &poise::serenity_prelude::Member,
) -> Result<Option<InviteMemberRecord>, Error> {
    if member.user.bot {
        return Ok(None);
    }

    if module_access_for_app(data, MODULE_ID, Some(member.guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(None);
    }

    let settings = load_settings(data, member.guild_id.get()).await?;
    if !settings.tracking {
        return Ok(None);
    }

    let lock = guild_invite_lock(member.guild_id.get());
    let _guard = lock.lock().await;

    let cached = {
        let cache = invite_cache().read().await;
        cache.get(&member.guild_id.get()).cloned()
    };
    let fresh = cache_guild_invites(ctx, member.guild_id).await?;
    let Some(cached) = cached else {
        return Ok(None);
    };

    let used_invite = find_used_invite(&cached, &fresh, now_unix());
    let Some(used_invite) = used_invite else {
        return Ok(None);
    };

    let Some(repo) = data.persistence.invites.clone() else {
        return Ok(None);
    };

    let member_id_str = member.user.id.get().to_string();
    let mut joined_record = repo
        .get_or_create(member.guild_id.get(), &member_id_str)
        .await?;
    joined_record.invite_data.inviter = used_invite.inviter_id.clone();
    joined_record.invite_data.code = Some(used_invite.code.clone());
    joined_record.updated_at = chrono::Utc::now();

    let Some(inviter_id) = used_invite.inviter_id.as_deref() else {
        repo.save(joined_record).await?;
        return Ok(None);
    };

    if inviter_id == member_id_str {
        joined_record.invite_data.fake += 1;
        let saved = repo.save(joined_record).await?;
        return Ok(Some(saved));
    }

    repo.save(joined_record).await?;

    let mut inviter_record = repo
        .get_or_create(member.guild_id.get(), inviter_id)
        .await?;
    inviter_record.invite_data.tracked += 1;
    inviter_record.updated_at = chrono::Utc::now();
    let inviter_record = repo.save(inviter_record).await?;

    apply_reward_roles(ctx, member.guild_id, &settings, &inviter_record).await?;
    Ok(Some(inviter_record))
}

pub async fn track_left_member(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    guild_id: GuildId,
    user: &User,
) -> Result<Option<InviteMemberRecord>, Error> {
    if user.bot {
        return Ok(None);
    }

    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(None);
    }

    let settings = load_settings(data, guild_id.get()).await?;
    if !settings.tracking {
        return Ok(None);
    }

    let Some(repo) = data.persistence.invites.clone() else {
        return Ok(None);
    };

    let lock = guild_invite_lock(guild_id.get());
    let _guard = lock.lock().await;

    let user_id_str = user.id.get().to_string();
    let mut member_record = repo.get_or_create(guild_id.get(), &user_id_str).await?;
    let Some(inviter_id) = member_record.invite_data.inviter.take() else {
        return Ok(None);
    };
    member_record.invite_data.code = None;
    member_record.updated_at = chrono::Utc::now();
    repo.save(member_record).await?;

    if inviter_id == user_id_str {
        return Ok(None);
    }

    let mut inviter_record = repo.get_or_create(guild_id.get(), &inviter_id).await?;
    inviter_record.invite_data.left += 1;
    inviter_record.updated_at = chrono::Utc::now();
    let inviter_record = repo.save(inviter_record).await?;

    apply_reward_roles(ctx, guild_id, &settings, &inviter_record).await?;
    Ok(Some(inviter_record))
}

async fn load_settings(data: &AppState, guild_id: u64) -> Result<InviteSettings, Error> {
    let guild_settings = data.guild_settings_cached(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| decode_module_configuration::<InviteSettings>(&module.configuration))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

async fn cache_guild_invites(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
) -> Result<HashMap<String, CachedInvite>, Error> {
    let invites = guild_id.invites(&ctx.http).await?;
    let mut fresh = HashMap::new();
    for invite in invites {
        fresh.insert(invite.code.clone(), cache_invite(invite));
    }

    let mut cache = invite_cache().write().await;
    cache.insert(guild_id.get(), fresh.clone());
    Ok(fresh)
}

fn cache_invite(invite: RichInvite) -> CachedInvite {
    CachedInvite {
        code: invite.code,
        uses: invite.uses,
        max_uses: invite.max_uses,
        inviter_id: invite.inviter.map(|user| user.id.get().to_string()),
        deleted_timestamp: None,
    }
}

fn find_used_invite(
    cached: &HashMap<String, CachedInvite>,
    fresh: &HashMap<String, CachedInvite>,
    now: i64,
) -> Option<CachedInvite> {
    if let Some(invite) = fresh.values().find(|invite| {
        cached
            .get(&invite.code)
            .is_some_and(|cached_invite| cached_invite.uses < invite.uses)
    }) {
        return Some(invite.clone());
    }

    let mut deleted = cached.values().cloned().collect::<Vec<_>>();
    deleted.sort_by_key(|invite| std::cmp::Reverse(invite.deleted_timestamp.unwrap_or_default()));
    deleted.into_iter().find(|invite| {
        !fresh.contains_key(&invite.code)
            && invite.max_uses > 0
            && invite.uses == invite.max_uses as u64 - 1
            && invite
                .deleted_timestamp
                .is_some_and(|ts| now.saturating_sub(ts) <= DELETED_INVITE_WINDOW_SECONDS)
    })
}

async fn apply_reward_roles(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
    settings: &InviteSettings,
    inviter_record: &InviteMemberRecord,
) -> Result<(), Error> {
    let Some(inviter_id) = inviter_record
        .member_id
        .parse::<u64>()
        .ok()
        .and_then(NonZeroU64::new)
        .map(UserId::from)
    else {
        return Ok(());
    };

    let Ok(member) = guild_id.member(ctx, inviter_id).await else {
        return Ok(());
    };

    let effective = inviter_record.invite_data.effective();
    for reward in &settings.ranks {
        let Some(role_id) = std::num::NonZeroU64::new(reward.role_id).map(RoleId::from) else {
            continue;
        };
        let has_role = member.roles.contains(&role_id);
        if effective >= reward.invites as i64 && !has_role {
            let _ = member.add_role(ctx, role_id).await;
        } else if effective < reward.invites as i64 && has_role {
            let _ = member.remove_role(ctx, role_id).await;
        }
    }

    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{CachedInvite, InviteSettings, find_used_invite};
    use std::collections::HashMap;

    #[test]
    fn invite_settings_accepts_legacy_rank_shape() {
        let settings: InviteSettings = serde_json::from_value(serde_json::json!({
            "tracking": true,
            "ranks": [{ "invites": 5, "_id": 123 }]
        }))
        .expect("settings");

        assert!(settings.tracking);
        assert_eq!(settings.ranks.len(), 1);
        assert_eq!(settings.ranks[0].role_id, 123);
    }

    #[test]
    fn invite_settings_accepts_string_snowflake_role_ids() {
        let settings: InviteSettings = serde_json::from_value(serde_json::json!({
            "tracking": true,
            "ranks": [{ "invites": 10, "role_id": "123456789012345678" }]
        }))
        .expect("settings");

        assert!(settings.tracking);
        assert_eq!(settings.ranks.len(), 1);
        assert_eq!(settings.ranks[0].invites, 10);
        assert_eq!(settings.ranks[0].role_id, 123_456_789_012_345_678);
    }

    #[test]
    fn finds_invite_with_increased_uses() {
        let mut cached = HashMap::new();
        cached.insert(
            "abc".to_string(),
            CachedInvite {
                code: "abc".to_string(),
                uses: 1,
                max_uses: 0,
                inviter_id: Some("1".to_string()),
                deleted_timestamp: None,
            },
        );
        let mut fresh = HashMap::new();
        fresh.insert(
            "abc".to_string(),
            CachedInvite {
                code: "abc".to_string(),
                uses: 2,
                max_uses: 0,
                inviter_id: Some("1".to_string()),
                deleted_timestamp: None,
            },
        );

        let used = find_used_invite(&cached, &fresh, 1_000).expect("invite should resolve");
        assert_eq!(used.code, "abc");
        assert_eq!(used.uses, 2);
    }

    #[test]
    fn expired_deleted_invite_outside_60s_window_is_ignored() {
        let mut cached = HashMap::new();
        cached.insert(
            "old1use".to_string(),
            CachedInvite {
                code: "old1use".to_string(),
                uses: 0,
                max_uses: 1,
                inviter_id: Some("1".to_string()),
                deleted_timestamp: Some(1_000),
            },
        );
        let fresh = HashMap::new();
        assert!(find_used_invite(&cached, &fresh, 1_059).is_some());
        assert!(find_used_invite(&cached, &fresh, 1_061).is_none());
    }
}
