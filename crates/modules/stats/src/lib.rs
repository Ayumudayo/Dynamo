use std::{
    collections::HashMap,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use dynamo_core::{
    AppState, DiscordCommand, Error, GatewayIntents, MemberStatsRecord, Module, ModuleCategory,
    ModuleManifest, SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
    module_access_for_app,
};
use poise::serenity_prelude::{
    ChannelId, CreateMessage, GuildId, Interaction, Message, UserId, VoiceState,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

const MODULE_ID: &str = "stats";
const DEFAULT_LEVEL_UP_MESSAGE: &str = "{member:tag}, You just advanced to **Level {level}**";
const DEFAULT_XP_COOLDOWN_SECONDS: u64 = 5;

fn xp_cooldowns() -> &'static Mutex<HashMap<String, i64>> {
    static COOLDOWNS: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    COOLDOWNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn voice_sessions() -> &'static Mutex<HashMap<String, i64>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct StatsModule;

impl Module for StatsModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Stats",
            "Guild member activity tracking for messages, interactions, XP, and voice time.",
            ModuleCategory::Utility,
            true,
            GatewayIntents::GUILDS
                | GatewayIntents::GUILD_MEMBERS
                | GatewayIntents::GUILD_MESSAGES
                | GatewayIntents::GUILD_VOICE_STATES,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        Vec::new()
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "stats",
                title: "Stats",
                description: Some("Configure XP tracking and level-up messaging."),
                fields: vec![
                    SettingsField {
                        key: "enabled",
                        label: "Enabled",
                        help_text: Some("Enable message, interaction, XP, and voice tracking."),
                        required: false,
                        kind: SettingsFieldKind::Toggle,
                    },
                    SettingsField {
                        key: "xp.message",
                        label: "Level-up message",
                        help_text: Some("Template used when a member levels up."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "xp.channel",
                        label: "Level-up channel ID",
                        help_text: Some("Optional channel override for level-up messages."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                ],
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct StatsSettings {
    enabled: bool,
    xp: StatsXpSettings,
}

impl Default for StatsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            xp: StatsXpSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct StatsXpSettings {
    message: String,
    #[serde(alias = "channel", deserialize_with = "deserialize_optional_snowflake")]
    channel_id: Option<u64>,
}

impl Default for StatsXpSettings {
    fn default() -> Self {
        Self {
            message: DEFAULT_LEVEL_UP_MESSAGE.to_string(),
            channel_id: None,
        }
    }
}

pub async fn handle_message(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    message: &Message,
) -> Result<(), Error> {
    let Some(guild_id) = message.guild_id else {
        return Ok(());
    };
    if message.author.bot {
        return Ok(());
    }

    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(());
    }

    let settings = load_settings(data, guild_id.get()).await?;
    if !settings.enabled {
        return Ok(());
    }

    let Some(repo) = data.persistence.member_stats.clone() else {
        return Ok(());
    };

    let mut record = repo
        .get_or_create(guild_id.get(), message.author.id.get())
        .await?;
    record.messages += 1;

    if should_award_xp(guild_id.get(), message.author.id.get()).await {
        record.xp += xp_to_add();
        if maybe_level_up(&mut record) {
            let content = render_level_up_message(
                &settings.xp.message,
                ctx,
                guild_id,
                message.author.id.get(),
                record.level,
            );
            let channel_id = settings.xp.channel_id.unwrap_or(message.channel_id.get());
            let _ = ChannelId::new(channel_id)
                .send_message(ctx, CreateMessage::new().content(content))
                .await;
        }
    }

    record.updated_at = chrono::Utc::now();
    repo.save(record).await?;
    Ok(())
}

pub async fn handle_interaction(
    _ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    interaction: &Interaction,
) -> Result<(), Error> {
    let (guild_id, member_id) = match interaction {
        Interaction::Command(command) => (
            command.guild_id.map(|id| id.get()),
            command.member.as_ref().map(|member| member.user.id.get()),
        ),
        Interaction::Component(component) => (
            component.guild_id.map(|id| id.get()),
            component.member.as_ref().map(|member| member.user.id.get()),
        ),
        Interaction::Modal(modal) => (
            modal.guild_id.map(|id| id.get()),
            modal.member.as_ref().map(|member| member.user.id.get()),
        ),
        _ => (None, None),
    };
    let (Some(guild_id), Some(member_id)) = (guild_id, member_id) else {
        return Ok(());
    };

    if module_access_for_app(data, MODULE_ID, Some(guild_id))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(());
    }

    let settings = load_settings(data, guild_id).await?;
    if !settings.enabled {
        return Ok(());
    }

    let Some(repo) = data.persistence.member_stats.clone() else {
        return Ok(());
    };
    let mut record = repo.get_or_create(guild_id, member_id).await?;

    match interaction {
        Interaction::Command(command) => {
            if command.data.kind == poise::serenity_prelude::CommandType::ChatInput {
                record.commands.slash += 1;
            } else if command.data.kind == poise::serenity_prelude::CommandType::Message {
                record.contexts.message += 1;
            } else if command.data.kind == poise::serenity_prelude::CommandType::User {
                record.contexts.user += 1;
            }
        }
        _ => {}
    }

    record.updated_at = chrono::Utc::now();
    repo.save(record).await?;
    Ok(())
}

pub async fn handle_voice_state_update(
    _ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    old: Option<&VoiceState>,
    new: &VoiceState,
) -> Result<(), Error> {
    let Some(guild_id) = new
        .guild_id
        .or_else(|| old.and_then(|state| state.guild_id))
    else {
        return Ok(());
    };

    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(());
    }

    let settings = load_settings(data, guild_id.get()).await?;
    if !settings.enabled {
        return Ok(());
    }

    let Some(repo) = data.persistence.member_stats.clone() else {
        return Ok(());
    };

    let is_bot = new
        .member
        .as_ref()
        .map(|member| member.user.bot)
        .or_else(|| old.and_then(|state| state.member.as_ref().map(|member| member.user.bot)))
        .unwrap_or(false);
    if is_bot {
        return Ok(());
    }

    let member_id = new.user_id.get();
    let key = format!("{}|{}", guild_id.get(), member_id);
    let old_channel = old.and_then(|state| state.channel_id);
    let new_channel = new.channel_id;
    let now = now_unix();

    if old_channel.is_none() && new_channel.is_some() {
        let mut record = repo.get_or_create(guild_id.get(), member_id).await?;
        record.voice.connections += 1;
        record.updated_at = chrono::Utc::now();
        repo.save(record).await?;
        voice_sessions().lock().await.insert(key, now);
        return Ok(());
    }

    if old_channel.is_some() && new_channel.is_none() {
        accumulate_voice_time(repo.as_ref(), guild_id.get(), member_id, &key, now).await?;
        return Ok(());
    }

    if old_channel.is_some() && new_channel.is_some() && old_channel != new_channel {
        accumulate_voice_time(repo.as_ref(), guild_id.get(), member_id, &key, now).await?;
        voice_sessions().lock().await.insert(key, now);
    }

    Ok(())
}

async fn load_settings(data: &AppState, guild_id: u64) -> Result<StatsSettings, Error> {
    let guild_settings = data.persistence.guild_settings_or_default(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| serde_json::from_value::<StatsSettings>(module.configuration.clone()))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

async fn should_award_xp(guild_id: u64, member_id: u64) -> bool {
    let key = format!("{}|{}", guild_id, member_id);
    let mut cache = xp_cooldowns().lock().await;
    let now = now_unix();
    let should_award = cache
        .get(&key)
        .map(|timestamp| now - *timestamp >= DEFAULT_XP_COOLDOWN_SECONDS as i64)
        .unwrap_or(true);
    if should_award {
        cache.insert(key, now);
    }
    should_award
}

fn maybe_level_up(record: &mut MemberStatsRecord) -> bool {
    let needed = record.level as u64 * record.level as u64 * 100;
    if record.xp <= needed {
        return false;
    }
    record.level += 1;
    record.xp -= needed;
    true
}

fn render_level_up_message(
    template: &str,
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
    member_id: u64,
    level: u32,
) -> String {
    let guild = ctx.cache.guild(guild_id);
    let guild_name = guild
        .as_ref()
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| guild_id.to_string());
    let member_count = guild.as_ref().map(|guild| guild.member_count).unwrap_or(0);
    let member = guild
        .as_ref()
        .and_then(|guild| guild.members.get(&UserId::new(member_id)).cloned());
    let display_name = member
        .as_ref()
        .map(|member| member.display_name().to_string())
        .unwrap_or_else(|| member_id.to_string());
    let tag = member
        .as_ref()
        .map(|member| member.user.tag())
        .unwrap_or_else(|| member_id.to_string());

    template
        .replace("\\n", "\n")
        .replace("{server}", &guild_name)
        .replace("{count}", &member_count.to_string())
        .replace("{member:id}", &member_id.to_string())
        .replace("{member:name}", &display_name)
        .replace("{member:mention}", &format!("<@{member_id}>"))
        .replace("{member:tag}", &tag)
        .replace("{level}", &level.to_string())
}

async fn accumulate_voice_time(
    repo: &dyn dynamo_core::MemberStatsRepository,
    guild_id: u64,
    member_id: u64,
    key: &str,
    now: i64,
) -> Result<(), Error> {
    let mut sessions = voice_sessions().lock().await;
    let Some(started_at) = sessions.remove(key) else {
        return Ok(());
    };
    drop(sessions);

    let mut record = repo.get_or_create(guild_id, member_id).await?;
    let elapsed = now.saturating_sub(started_at);
    record.voice.time_seconds += elapsed as u64;
    record.updated_at = chrono::Utc::now();
    repo.save(record).await?;
    Ok(())
}

fn xp_to_add() -> u64 {
    ((now_unix_nanos().unsigned_abs() % 20) + 1) as u64
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn now_unix_nanos() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as i128)
        .unwrap_or_default()
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
    use super::{StatsSettings, maybe_level_up};
    use dynamo_core::MemberStatsRecord;

    #[test]
    fn stats_settings_accepts_nested_xp_shape() {
        let settings: StatsSettings = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "xp": { "message": "hi", "channel": "123" }
        }))
        .expect("settings");
        assert!(settings.enabled);
        assert_eq!(settings.xp.channel_id, Some(123));
    }

    #[test]
    fn levels_up_when_xp_exceeds_threshold() {
        let mut record = MemberStatsRecord {
            guild_id: 1,
            member_id: 2,
            messages: 0,
            voice: Default::default(),
            commands: Default::default(),
            contexts: Default::default(),
            xp: 150,
            level: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        assert!(maybe_level_up(&mut record));
        assert_eq!(record.level, 2);
        assert_eq!(record.xp, 50);
    }
}
