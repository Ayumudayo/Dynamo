use dynamo_core::{
    Context, DiscordCommand, Error, GatewayIntents, GuildModuleSettings, Module, ModuleCategory,
    ModuleManifest, MusicBackendKind, SettingsField, SettingsFieldKind, SettingsSchema,
    SettingsSection, module_access_for_context,
};
use serde::{Deserialize, Serialize};

const MODULE_ID: &str = "music";
const DEFAULT_AUTO_LEAVE_SECONDS: u64 = 180;

pub struct MusicModule;

impl Module for MusicModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Music",
            "Optional music module with Songbird default and Lavalink-capable backend settings.",
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
                    description: Some("Select the runtime backend and common playback defaults."),
                    fields: vec![
                        SettingsField {
                            key: "backend",
                            label: "Backend",
                            help_text: Some(
                                "Songbird is the default in-process backend; Lavalink uses an external server.",
                            ),
                            required: false,
                            kind: SettingsFieldKind::Select {
                                options: vec![
                                    dynamo_core::SettingOption {
                                        label: "Songbird",
                                        value: "songbird",
                                    },
                                    dynamo_core::SettingOption {
                                        label: "Lavalink",
                                        value: "lavalink",
                                    },
                                ],
                            },
                        },
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
                SettingsSection {
                    id: "lavalink",
                    title: "Lavalink",
                    description: Some("External Lavalink node connection settings."),
                    fields: vec![
                        SettingsField {
                            key: "lavalink.host",
                            label: "Host",
                            help_text: Some("Lavalink server hostname or IP."),
                            required: false,
                            kind: SettingsFieldKind::Text,
                        },
                        SettingsField {
                            key: "lavalink.port",
                            label: "Port",
                            help_text: Some("Lavalink TCP port."),
                            required: false,
                            kind: SettingsFieldKind::Integer,
                        },
                        SettingsField {
                            key: "lavalink.password",
                            label: "Password",
                            help_text: Some("Lavalink authorization password."),
                            required: false,
                            kind: SettingsFieldKind::Text,
                        },
                        SettingsField {
                            key: "lavalink.secure",
                            label: "Secure",
                            help_text: Some("Use HTTPS/WSS for Lavalink traffic."),
                            required: false,
                            kind: SettingsFieldKind::Toggle,
                        },
                        SettingsField {
                            key: "lavalink.resume_key",
                            label: "Resume key",
                            help_text: Some("Optional resume key or session label."),
                            required: false,
                            kind: SettingsFieldKind::Text,
                        },
                    ],
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct MusicSettings {
    backend: MusicBackendKind,
    default_source: MusicSourceKind,
    auto_leave_seconds: u64,
    songbird: SongbirdSettings,
    lavalink: LavalinkSettings,
}

impl Default for MusicSettings {
    fn default() -> Self {
        Self {
            backend: MusicBackendKind::Songbird,
            default_source: MusicSourceKind::Youtube,
            auto_leave_seconds: DEFAULT_AUTO_LEAVE_SECONDS,
            songbird: SongbirdSettings::default(),
            lavalink: LavalinkSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct SongbirdSettings {
    ytdlp_program: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct LavalinkSettings {
    host: Option<String>,
    port: Option<u16>,
    password: Option<String>,
    secure: bool,
    resume_key: Option<String>,
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
    subcommands("music_status")
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
    let runtime_status = match &ctx.data().services.music {
        Some(service) => {
            let status = service.status().await?;
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
            format!("`{:?}`", settings.backend),
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
            "Lavalink",
            format!(
                "host: {}\nport: {}\npassword: {}\nsecure: {}\nresume_key: {}",
                settings.lavalink.host.as_deref().unwrap_or("unset"),
                settings
                    .lavalink
                    .port
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unset".to_string()),
                if settings.lavalink.password.is_some() {
                    "configured"
                } else {
                    "unset"
                },
                settings.lavalink.secure,
                settings.lavalink.resume_key.as_deref().unwrap_or("unset"),
            ),
            false,
        )
        .field("Runtime", runtime_status, false);

    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{MusicBackendKind, MusicSettings, MusicSourceKind};

    #[test]
    fn music_settings_accept_nested_backend_shape() {
        let settings: MusicSettings = serde_json::from_value(serde_json::json!({
            "backend": "lavalink",
            "default_source": "youtube_music",
            "auto_leave_seconds": 90,
            "songbird": { "ytdlp_program": "yt-dlp" },
            "lavalink": {
                "host": "127.0.0.1",
                "port": 2333,
                "password": "youshallnotpass",
                "secure": false,
                "resume_key": "dynamo"
            }
        }))
        .expect("settings");

        assert_eq!(settings.backend, MusicBackendKind::Lavalink);
        assert_eq!(settings.default_source, MusicSourceKind::YoutubeMusic);
        assert_eq!(settings.auto_leave_seconds, 90);
        assert_eq!(settings.lavalink.port, Some(2333));
    }
}
