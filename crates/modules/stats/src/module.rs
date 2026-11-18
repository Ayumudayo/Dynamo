use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use dynamo_access::module_access_for_app;
use dynamo_domain_stats::{MemberStatsDelta, MemberStatsRecord, StatsLeaderboardMetric};
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{
    AppState, Context, Error, decode_module_configuration, deserialize_optional_snowflake,
};
use poise::serenity_prelude::{
    ChannelId, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, CreateMessage, GuildId,
    Interaction, Message, User, UserId, VoiceState,
};
use rand::Rng;
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

impl Module<AppState, Error> for StatsModule {
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

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![rank(), leaderboard()]
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
                        kind: SettingsFieldKind::Textarea,
                    },
                    SettingsField {
                        key: "xp.channel",
                        label: "Level-up channel",
                        help_text: Some("Optional channel override for level-up messages."),
                        required: false,
                        kind: SettingsFieldKind::Channel,
                    },
                ],
            }],
        }
    }
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
enum LeaderboardMetricChoice {
    #[name = "XP"]
    Xp,
    #[name = "Messages"]
    Messages,
    #[name = "Voice Time"]
    Voice,
}

impl From<LeaderboardMetricChoice> for StatsLeaderboardMetric {
    fn from(choice: LeaderboardMetricChoice) -> Self {
        match choice {
            LeaderboardMetricChoice::Xp => Self::Xp,
            LeaderboardMetricChoice::Messages => Self::Messages,
            LeaderboardMetricChoice::Voice => Self::VoiceSeconds,
        }
    }
}

/// View level, XP, message count, and voice activity for a server member.
#[poise::command(slash_command, guild_only, category = "Utility")]
async fn rank(
    ctx: Context<'_>,
    #[description = "Optional member to inspect"] user: Option<User>,
) -> Result<(), Error> {
    let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    if !settings.enabled {
        ctx.send(
            poise::CreateReply::default()
                .content(if ko {
                    "이 서버에서는 활동 통계 추적이 비활성화되어 있습니다."
                } else {
                    "Stats tracking is disabled in this server."
                })
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let Some(repo) = ctx.data().persistence.member_stats.as_ref() else {
        ctx.send(
            poise::CreateReply::default()
                .content(if ko {
                    "통계 저장소가 설정되어 있지 않습니다."
                } else {
                    "Stats repository is not configured."
                })
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let target = user.as_ref().unwrap_or_else(|| ctx.author());
    let record = repo.get_or_create(guild_id.get(), target.id.get()).await?;
    let needed = MemberStatsRecord::xp_needed_for_level(record.level);
    let embed = CreateEmbed::new()
        .title(if ko {
            format!("{} — 활동 순위", target.name)
        } else {
            format!("{} — Rank", target.name)
        })
        .thumbnail(target.face())
        .field(
            if ko { "레벨" } else { "Level" },
            record.level.to_string(),
            true,
        )
        .field(
            if ko { "경험치" } else { "XP" },
            format!("{} / {}", record.xp, needed),
            true,
        )
        .field(
            if ko { "메시지" } else { "Messages" },
            record.messages.to_string(),
            true,
        )
        .field(
            if ko { "음성 활동" } else { "Voice Time" },
            format_voice_duration(record.voice.time_seconds),
            true,
        )
        .field(
            if ko {
                "슬래시 명령어"
            } else {
                "Slash Commands"
            },
            record.commands.slash.to_string(),
            true,
        );
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// View the top members in this server ranked by XP, messages, or voice time.
#[poise::command(slash_command, guild_only, category = "Utility")]
async fn leaderboard(
    ctx: Context<'_>,
    #[description = "Ranking metric (XP, Messages, or Voice Time)"] metric: Option<
        LeaderboardMetricChoice,
    >,
) -> Result<(), Error> {
    let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    if !settings.enabled {
        ctx.send(
            poise::CreateReply::default()
                .content(if ko {
                    "이 서버에서는 활동 통계 추적이 비활성화되어 있습니다."
                } else {
                    "Stats tracking is disabled in this server."
                })
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let Some(repo) = ctx.data().persistence.member_stats.as_ref() else {
        ctx.send(
            poise::CreateReply::default()
                .content(if ko {
                    "통계 저장소가 설정되어 있지 않습니다."
                } else {
                    "Stats repository is not configured."
                })
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let metric = metric.unwrap_or(LeaderboardMetricChoice::Xp);
    let entries = repo.leaderboard(guild_id.get(), metric.into(), 10).await?;
    if entries.is_empty() {
        ctx.say(if ko {
            "아직 이 서버에 기록된 활동 통계가 없습니다."
        } else {
            "No activity stats have been recorded in this server yet."
        })
        .await?;
        return Ok(());
    }
    let label = match (ko, metric) {
        (true, LeaderboardMetricChoice::Xp) => "경험치 순위",
        (true, LeaderboardMetricChoice::Messages) => "메시지 순위",
        (true, LeaderboardMetricChoice::Voice) => "음성 활동 순위",
        (false, LeaderboardMetricChoice::Xp) => "XP Leaderboard",
        (false, LeaderboardMetricChoice::Messages) => "Message Leaderboard",
        (false, LeaderboardMetricChoice::Voice) => "Voice Activity Leaderboard",
    };
    let description = entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let value = match (ko, metric) {
                (true, LeaderboardMetricChoice::Xp) => {
                    format!("레벨 {} • {} XP", entry.level, entry.xp)
                }
                (true, LeaderboardMetricChoice::Messages) => format!("메시지 {}개", entry.messages),
                (false, LeaderboardMetricChoice::Xp) => {
                    format!("Level {} • {} XP", entry.level, entry.xp)
                }
                (false, LeaderboardMetricChoice::Messages) => {
                    format!("{} messages", entry.messages)
                }
                (_, LeaderboardMetricChoice::Voice) => {
                    format_voice_duration(entry.voice.time_seconds)
                }
            };
            format!("{}. <@{}> — {value}", idx + 1, entry.member_id)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let embed = CreateEmbed::new()
        .title(label)
        .description(description)
        .footer(CreateEmbedFooter::new(if ko {
            "상위 10명"
        } else {
            "Top 10 members"
        }));
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

fn format_voice_duration(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct StatsSettings {
    enabled: bool,
    xp: StatsXpSettings,
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

    let xp = if should_award_xp(guild_id.get(), message.author.id.get()).await {
        xp_to_add()
    } else {
        0
    };
    let update = repo
        .apply_delta(
            guild_id.get(),
            message.author.id.get(),
            MemberStatsDelta {
                messages: 1,
                xp,
                ..Default::default()
            },
        )
        .await?;
    if update.leveled_up {
        let content = render_level_up_message(
            &settings.xp.message,
            ctx,
            guild_id,
            message.author.id.get(),
            update.record.level,
        );
        if let Some(channel_id) = settings
            .xp
            .channel_id
            .filter(|&id| id > 0)
            .or_else(|| Some(message.channel_id.get()).filter(|&id| id > 0))
            .and_then(NonZeroU64::new)
            .map(ChannelId::from)
        {
            let _ = channel_id
                .send_message(
                    ctx,
                    CreateMessage::new()
                        .content(content)
                        .allowed_mentions(CreateAllowedMentions::new().users([message.author.id])),
                )
                .await;
        }
    }
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
    let mut delta = MemberStatsDelta::default();

    if let Interaction::Command(command) = interaction {
        if command.data.kind == poise::serenity_prelude::CommandType::ChatInput {
            delta.slash_commands = 1;
        } else if command.data.kind == poise::serenity_prelude::CommandType::Message {
            delta.message_contexts = 1;
        } else if command.data.kind == poise::serenity_prelude::CommandType::User {
            delta.user_contexts = 1;
        } else {
            return Ok(());
        }
    } else {
        return Ok(());
    }
    repo.apply_delta(guild_id, member_id, delta).await?;
    Ok(())
}

pub async fn seed_voice_sessions_from_cache(
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
    if !settings.enabled {
        return Ok(());
    }

    let active_keys = {
        let Some(guild) = ctx.cache.guild(guild_id) else {
            return Ok(());
        };
        let afk_channel_id = guild
            .afk_metadata
            .as_ref()
            .map(|metadata| metadata.afk_channel_id);
        guild
            .voice_states
            .values()
            .filter_map(|vs| {
                let is_bot = guild
                    .members
                    .get(&vs.user_id)
                    .is_some_and(|member| member.user.bot);
                let active = !is_bot
                    && vs.channel_id.is_some_and(|ch| Some(ch) != afk_channel_id)
                    && !vs.deaf
                    && !vs.self_deaf;
                active.then(|| format!("{}|{}", guild_id.get(), vs.user_id.get()))
            })
            .collect::<Vec<_>>()
    };

    if active_keys.is_empty() {
        return Ok(());
    }
    let now = now_unix();
    let mut sessions = voice_sessions().lock().await;
    for key in active_keys {
        sessions.entry(key).or_insert(now);
    }
    Ok(())
}

pub async fn handle_voice_state_update(
    ctx: &poise::serenity_prelude::Context,
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

    let afk_channel_id = ctx
        .cache
        .guild(guild_id)
        .and_then(|guild| guild.afk_metadata.as_ref().map(|meta| meta.afk_channel_id));
    let member_id = new.user_id.get();
    let key = format!("{}|{}", guild_id.get(), member_id);
    let old_channel = old.and_then(|state| state.channel_id);
    let new_channel = new.channel_id;
    let new_active =
        new_channel.is_some_and(|ch| Some(ch) != afk_channel_id) && !new.deaf && !new.self_deaf;
    let session_active = voice_sessions().lock().await.contains_key(&key);
    let now = now_unix();

    if !session_active && new_active {
        if old_channel.is_none() {
            repo.apply_delta(
                guild_id.get(),
                member_id,
                MemberStatsDelta {
                    voice_connections: 1,
                    ..Default::default()
                },
            )
            .await?;
        }
        voice_sessions().lock().await.insert(key, now);
        return Ok(());
    }

    if session_active && !new_active {
        accumulate_voice_time(repo.as_ref(), guild_id.get(), member_id, &key, now).await?;
        return Ok(());
    }

    if session_active && new_active && old_channel != new_channel {
        accumulate_voice_time(repo.as_ref(), guild_id.get(), member_id, &key, now).await?;
        voice_sessions().lock().await.insert(key, now);
    }

    Ok(())
}

async fn load_settings(data: &AppState, guild_id: u64) -> Result<StatsSettings, Error> {
    let guild_settings = data.guild_settings_cached(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| decode_module_configuration::<StatsSettings>(&module.configuration))
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
    let member = NonZeroU64::new(member_id)
        .map(UserId::from)
        .and_then(|user_id| {
            guild
                .as_ref()
                .and_then(|guild| guild.members.get(&user_id).cloned())
        });
    let display_name = member
        .as_ref()
        .map(|member| member.display_name().to_string())
        .unwrap_or_else(|| member_id.to_string());
    let tag = member
        .as_ref()
        .map(|member| member.user.tag())
        .unwrap_or_else(|| member_id.to_string());
    let mention = format!("<@{member_id}>");

    template
        .replace("\\n", "\n")
        .replace("{server}", &guild_name)
        .replace("{count}", &member_count.to_string())
        .replace("{member:id}", &member_id.to_string())
        .replace("{member:name}", &display_name)
        .replace("{member:mention}", &mention)
        .replace("{user}", &mention)
        .replace("{member:tag}", &tag)
        .replace("{level}", &level.to_string())
}

async fn accumulate_voice_time(
    repo: &dyn dynamo_repositories::MemberStatsRepository,
    guild_id: u64,
    member_id: u64,
    key: &str,
    now: i64,
) -> Result<(), Error> {
    let started_at = {
        let mut sessions = voice_sessions().lock().await;
        sessions.remove(key)
    };
    let Some(started_at) = started_at else {
        return Ok(());
    };
    let elapsed = now.saturating_sub(started_at).max(0) as u64;
    if elapsed > 0 {
        repo.apply_delta(
            guild_id,
            member_id,
            MemberStatsDelta {
                voice_seconds: elapsed,
                ..Default::default()
            },
        )
        .await?;
    }
    Ok(())
}

pub async fn flush_all_voice_sessions(
    repo: &dyn dynamo_repositories::MemberStatsRepository,
) -> Result<(), Error> {
    let drained = {
        let mut sessions = voice_sessions().lock().await;
        sessions.drain().collect::<Vec<_>>()
    };
    let now = now_unix();
    let mut first_error = None;
    let mut failures = 0usize;
    for (key, started_at) in drained {
        let Some((guild_raw, member_raw)) = key.split_once('|') else {
            continue;
        };
        let (Ok(guild_id), Ok(member_id)) = (guild_raw.parse::<u64>(), member_raw.parse::<u64>())
        else {
            continue;
        };
        let elapsed = now.saturating_sub(started_at).max(0) as u64;
        if elapsed == 0 {
            continue;
        }
        if let Err(error) = repo
            .apply_delta(
                guild_id,
                member_id,
                MemberStatsDelta {
                    voice_seconds: elapsed,
                    ..Default::default()
                },
            )
            .await
        {
            failures += 1;
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error.context(format!(
            "failed to flush {failures} voice session(s) on shutdown"
        )));
    }
    Ok(())
}

fn xp_to_add() -> u64 {
    rand::thread_rng().gen_range(1..=20)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::StatsSettings;
    use dynamo_domain_stats::{MemberStatsDelta, MemberStatsRecord};

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

        record.xp = 140;
        assert!(
            record
                .apply_delta(
                    MemberStatsDelta {
                        xp: 10,
                        ..Default::default()
                    },
                    chrono::Utc::now()
                )
                .unwrap()
        );
        assert_eq!(record.level, 2);
        assert_eq!(record.xp, 50);
    }
}
