use dynamo_core::{
    Context, DiscordCommand, Error, GatewayIntents, GuildModuleSettings, Module, ModuleCategory,
    ModuleManifest, MusicBackendConfig, MusicBackendKind, MusicQueueSnapshot, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection, module_access_for_context,
};
use poise::serenity_prelude::CreateEmbed;
use serde::{Deserialize, Serialize};

const MODULE_ID: &str = "music";
const DEFAULT_AUTO_LEAVE_SECONDS: u64 = 180;

pub struct MusicModule;

impl Module for MusicModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Music",
            "Music module with Songbird as the built-in playback backend.",
            ModuleCategory::Music,
            false,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![music()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![
                SettingsSection {
                    id: "music",
                    title: "Music",
                    description: Some("Configure Songbird playback defaults."),
                    fields: vec![
                        SettingsField {
                            key: "default_source",
                            label: "Default source",
                            help_text: Some("Preferred upstream source for search-based playback."),
                            required: false,
                            kind: SettingsFieldKind::Select {
                                options: vec![
                                    dynamo_core::SettingOption {
                                        label: "YouTube",
                                        value: "youtube",
                                    },
                                    dynamo_core::SettingOption {
                                        label: "YouTube Music",
                                        value: "youtube_music",
                                    },
                                    dynamo_core::SettingOption {
                                        label: "SoundCloud",
                                        value: "soundcloud",
                                    },
                                ],
                            },
                        },
                        SettingsField {
                            key: "auto_leave_seconds",
                            label: "Auto leave seconds",
                            help_text: Some("Idle timeout before disconnecting from voice."),
                            required: false,
                            kind: SettingsFieldKind::Integer,
                        },
                    ],
                },
                SettingsSection {
                    id: "songbird",
                    title: "Songbird",
                    description: Some("Songbird-specific local process settings."),
                    fields: vec![SettingsField {
                        key: "songbird.ytdlp_program",
                        label: "yt-dlp executable",
                        help_text: Some(
                            "Optional override for the local yt-dlp executable path or command name.",
                        ),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    }],
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct MusicSettings {
    default_source: MusicSourceKind,
    auto_leave_seconds: u64,
    songbird: SongbirdSettings,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            default_source: MusicSourceKind::Youtube,
            auto_leave_seconds: DEFAULT_AUTO_LEAVE_SECONDS,
            songbird: SongbirdSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SongbirdSettings {
    ytdlp_program: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum MusicSourceKind {
    #[default]
    Youtube,
    YoutubeMusic,
    Soundcloud,
}

#[poise::command(
    slash_command,
    guild_only,
    category = "Music",
    subcommands(
        "music_status",
        "music_join",
        "music_leave",
        "music_play",
        "music_pause",
        "music_resume",
        "music_skip",
        "music_stop",
        "music_queue"
    )
)]
async fn music(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(slash_command, guild_only, rename = "status")]
async fn music_status(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let settings = load_settings(ctx).await?;
    let config = settings.to_backend_config();
    let runtime_status = match &ctx.data().services.music {
        Some(service) => {
            let status = service.status(&config).await?;
            format!(
                "runtime backend: `{:?}`\nhealth: `{}`\nsummary: {}",
                status.backend, status.healthy, status.summary
            )
        }
        None => "runtime backend: `unwired`\nhealth: `false`\nsummary: backend abstraction is present but playback runtime is not wired in this build yet.".to_string(),
    };

    let embed = poise::serenity_prelude::CreateEmbed::new()
        .title("Music Backend Status")
        .field(
            "Configured Backend",
            "`Songbird`".to_string(),
            true,
        )
        .field(
            "Default Source",
            format!("`{:?}`", settings.default_source),
            true,
        )
        .field(
            "Auto Leave",
            format!("`{}s`", settings.auto_leave_seconds),
            true,
        )
        .field(
            "Songbird",
            format!(
                "yt-dlp: {}",
                settings
                    .songbird
                    .ytdlp_program
                    .as_deref()
                    .unwrap_or("system default")
            ),
            false,
        )
        .field(
            "External Node Guide",
            "Lavalink is documented as an external deployment guide only and is not configurable from this runtime.",
            false,
        )
        .field("Runtime", runtime_status, false);

    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, rename = "join")]
async fn music_join(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    ctx.defer().await?;

    let config = load_settings(ctx).await?.to_backend_config();
    let Some(service) = ctx.data().services.music.as_ref() else {
        ctx.say("Music runtime service is not configured in this build.")
            .await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(voice_channel_id) = author_voice_channel_id(ctx) else {
        ctx.say("You need to join a voice channel first.").await?;
        return Ok(());
    };

    service
        .join(
            ctx.serenity_context(),
            guild_id.get(),
            voice_channel_id,
            ctx.channel_id().get(),
            &config,
        )
        .await?;
    ctx.say(format!("Joined <#{}>.", voice_channel_id)).await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, rename = "leave")]
async fn music_leave(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    ctx.defer().await?;

    let config = load_settings(ctx).await?.to_backend_config();
    let Some(service) = ctx.data().services.music.as_ref() else {
        ctx.say("Music runtime service is not configured in this build.")
            .await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    service
        .leave(ctx.serenity_context(), guild_id.get(), &config)
        .await?;
    ctx.say("Disconnected from voice.").await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, rename = "play")]
async fn music_play(
    ctx: Context<'_>,
    #[description = "YouTube URL or search query"] query: String,
) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    ctx.defer().await?;

    let config = load_settings(ctx).await?.to_backend_config();
    let Some(service) = ctx.data().services.music.as_ref() else {
        ctx.say("Music runtime service is not configured in this build.")
            .await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(voice_channel_id) = author_voice_channel_id(ctx) else {
        ctx.say("You need to join a voice channel first.").await?;
        return Ok(());
    };

    let result = service
        .play(
            ctx.serenity_context(),
            guild_id.get(),
            voice_channel_id,
            ctx.channel_id().get(),
            &query,
            &ctx.author().name,
            &config,
        )
        .await?;

    let embed = CreateEmbed::new()
        .title(if result.started_immediately {
            "Now Playing"
        } else {
            "Added To Queue"
        })
        .description(result.track.title.clone())
        .field("Source", result.track.source.clone(), true)
        .field(
            "Duration",
            result
                .track
                .duration_seconds
                .map(|value| format!("{}s", value))
                .unwrap_or_else(|| "unknown".to_string()),
            true,
        );
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, rename = "pause")]
async fn music_pause(ctx: Context<'_>) -> Result<(), Error> {
    run_queue_action(ctx, QueueAction::Pause).await
}

#[poise::command(slash_command, guild_only, rename = "resume")]
async fn music_resume(ctx: Context<'_>) -> Result<(), Error> {
    run_queue_action(ctx, QueueAction::Resume).await
}

#[poise::command(slash_command, guild_only, rename = "skip")]
async fn music_skip(ctx: Context<'_>) -> Result<(), Error> {
    run_queue_action(ctx, QueueAction::Skip).await
}

#[poise::command(slash_command, guild_only, rename = "stop")]
async fn music_stop(ctx: Context<'_>) -> Result<(), Error> {
    run_queue_action(ctx, QueueAction::Stop).await
}

#[poise::command(slash_command, guild_only, rename = "queue")]
async fn music_queue(ctx: Context<'_>) -> Result<(), Error> {
    run_queue_action(ctx, QueueAction::Queue).await
}

async fn load_settings(ctx: Context<'_>) -> Result<MusicSettings, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(MusicSettings::default());
    };

    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;

    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(parse_music_settings)
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

fn parse_music_settings(module: &GuildModuleSettings) -> Result<MusicSettings, Error> {
    Ok(serde_json::from_value::<MusicSettings>(
        module.configuration.clone(),
    )?)
}

impl MusicSettings {
    fn to_backend_config(&self) -> MusicBackendConfig {
        MusicBackendConfig {
            backend: MusicBackendKind::Songbird,
            default_source: format!("{:?}", self.default_source).to_ascii_lowercase(),
            auto_leave_seconds: self.auto_leave_seconds,
            songbird_ytdlp_program: self.songbird.ytdlp_program.clone(),
            lavalink_host: None,
            lavalink_port: None,
            lavalink_password: None,
            lavalink_secure: false,
            lavalink_resume_key: None,
        }
    }
}

fn author_voice_channel_id(ctx: Context<'_>) -> Option<u64> {
    let guild_id = ctx.guild_id()?;
    let guild = ctx.serenity_context().cache.guild(guild_id)?;
    guild
        .voice_states
        .get(&ctx.author().id)
        .and_then(|voice_state| voice_state.channel_id)
        .map(|channel_id| channel_id.get())
}

enum QueueAction {
    Pause,
    Resume,
    Skip,
    Stop,
    Queue,
}

async fn run_queue_action(ctx: Context<'_>, action: QueueAction) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    ctx.defer().await?;

    let config = load_settings(ctx).await?.to_backend_config();
    let Some(service) = ctx.data().services.music.as_ref() else {
        ctx.say("Music runtime service is not configured in this build.")
            .await?;
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let snapshot = match action {
        QueueAction::Pause => {
            service
                .pause(ctx.serenity_context(), guild_id.get(), &config)
                .await?
        }
        QueueAction::Resume => {
            service
                .resume(ctx.serenity_context(), guild_id.get(), &config)
                .await?
        }
        QueueAction::Skip => {
            service
                .skip(ctx.serenity_context(), guild_id.get(), &config)
                .await?
        }
        QueueAction::Stop => {
            service
                .stop(ctx.serenity_context(), guild_id.get(), &config)
                .await?
        }
        QueueAction::Queue => {
            service
                .queue(ctx.serenity_context(), guild_id.get(), &config)
                .await?
        }
    };
    let embed = render_queue_snapshot("Music Queue", &snapshot);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

fn render_queue_snapshot(title: &str, snapshot: &MusicQueueSnapshot) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title(title)
        .field("Backend", format!("`{:?}`", snapshot.backend), true)
        .field("Connected", snapshot.connected.to_string(), true)
        .field("Paused", snapshot.paused.to_string(), true);

    if let Some(current) = &snapshot.current {
        embed = embed.field(
            "Current",
            format!("{}\nrequested by `{}`", current.title, current.requested_by),
            false,
        );
    } else {
        embed = embed.field("Current", "Nothing playing", false);
    }

    if snapshot.upcoming.is_empty() {
        embed = embed.field("Up Next", "Queue is empty", false);
    } else {
        let queue_text = snapshot
            .upcoming
            .iter()
            .take(5)
            .enumerate()
            .map(|(index, track)| {
                format!("{}. {} (`{}`)", index + 1, track.title, track.requested_by)
            })
            .collect::<Vec<_>>()
            .join("\n");
        embed = embed.field("Up Next", queue_text, false);
    }

    embed
}

#[cfg(test)]
mod tests {
    use super::{MusicSettings, MusicSourceKind};

    #[test]
    fn music_settings_accept_songbird_shape() {
        let settings: MusicSettings = serde_json::from_value(serde_json::json!({
            "default_source": "youtube_music",
            "auto_leave_seconds": 90,
            "songbird": { "ytdlp_program": "yt-dlp" }
        }))
        .expect("settings");

        assert_eq!(settings.default_source, MusicSourceKind::YoutubeMusic);
        assert_eq!(settings.auto_leave_seconds, 90);
        assert_eq!(settings.songbird.ytdlp_program.as_deref(), Some("yt-dlp"));
    }
}
