use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use dynamo_core::{
    AppState, DiscordCommand, Error, GatewayIntents, InviteMemberRecord, Module, ModuleCategory,
    ModuleManifest, SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
    module_access_for_app,
};
use poise::serenity_prelude::{GuildId, InviteCreateEvent, InviteDeleteEvent, RichInvite, RoleId, User, UserId};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

const MODULE_ID: &str = "invite";
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

pub struct InviteModule;

impl Module for InviteModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Invite Tracking",
            "Tracks inviter attribution and reward roles for member joins and leaves.",
            ModuleCategory::Utility,
            true,
            GatewayIntents::GUILDS
                | GatewayIntents::GUILD_MEMBERS
                | GatewayIntents::GUILD_INVITES,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        Vec::new()
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
                        help_text: Some("Array of rank objects with `invites` and role `_id`/`role_id`."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                ],
            }],
        }
    }
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
    #[serde(alias = "_id", alias = "role_id")]
    role_id: u64,
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
    if let Some(guild_cache) = cache.get_mut(&guild_id.get()) {
        if let Some(invite) = guild_cache.get_mut(&event.code) {
            invite.deleted_timestamp = Some(now_unix());
        }
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

    let cached = {
        let cache = invite_cache().read().await;
        cache.get(&member.guild_id.get()).cloned()
    };
    let fresh = cache_guild_invites(ctx, member.guild_id).await?;
    let Some(cached) = cached else {
        return Ok(None);
    };

    let used_invite = find_used_invite(&cached, &fresh);
    let Some(used_invite) = used_invite else {
        return Ok(None);
    };

    let Some(repo) = data.persistence.invites.clone() else {
        return Ok(None);
    };

    let mut joined_record = repo
        .get_or_create(member.guild_id.get(), &member.user.id.get().to_string())
        .await?;
    joined_record.invite_data.inviter = used_invite.inviter_id.clone();
    joined_record.invite_data.code = Some(used_invite.code.clone());
    joined_record.updated_at = chrono::Utc::now();
    repo.save(joined_record).await?;

    let Some(inviter_id) = used_invite.inviter_id.as_deref() else {
        return Ok(None);
    };

    let mut inviter_record = repo.get_or_create(member.guild_id.get(), inviter_id).await?;
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

    let member_record = repo
        .get_or_create(guild_id.get(), &user.id.get().to_string())
        .await?;
    let Some(inviter_id) = member_record.invite_data.inviter.clone() else {
        return Ok(None);
    };

    let mut inviter_record = repo.get_or_create(guild_id.get(), &inviter_id).await?;
    inviter_record.invite_data.left += 1;
    inviter_record.updated_at = chrono::Utc::now();
    let inviter_record = repo.save(inviter_record).await?;

    apply_reward_roles(ctx, guild_id, &settings, &inviter_record).await?;
    Ok(Some(inviter_record))
}

async fn load_settings(data: &AppState, guild_id: u64) -> Result<InviteSettings, Error> {
    let guild_settings = data.persistence.guild_settings_or_default(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| serde_json::from_value::<InviteSettings>(module.configuration.clone()))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

async fn cache_guild_invites(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
) -> Result<HashMap<String, CachedInvite>, Error> {
    let invites = guild_id.invites(&ctx.http).await.unwrap_or_default();
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
        !fresh.contains_key(&invite.code) && invite.max_uses > 0 && invite.uses == invite.max_uses as u64 - 1
    })
}

async fn apply_reward_roles(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
    settings: &InviteSettings,
    inviter_record: &InviteMemberRecord,
) -> Result<(), Error> {
    let Ok(inviter_id) = inviter_record.member_id.parse::<u64>() else {
        return Ok(());
    };

    let Ok(member) = guild_id.member(ctx, UserId::new(inviter_id)).await else {
        return Ok(());
    };

    let effective = inviter_record.invite_data.effective();
    for reward in &settings.ranks {
        let role_id = RoleId::new(reward.role_id);
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

        let used = find_used_invite(&cached, &fresh).expect("invite should resolve");
        assert_eq!(used.code, "abc");
        assert_eq!(used.uses, 2);
    }
}
