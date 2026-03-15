use dynamo_core::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingOption,
    SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};

const MODULE_ID: &str = "music";

pub struct MusicModule;

impl Module for MusicModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Music",
            "Optional music playback module reserved for first-party opt-in deployments.",
            ModuleCategory::Music,
            false,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        Vec::new()
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "music",
                title: "Music",
                description: Some("Optional music module configuration."),
                fields: vec![
                    SettingsField {
                        key: "default_source",
                        label: "Default search source",
                        help_text: Some("Preferred upstream source for search-based playback."),
                        required: false,
                        kind: SettingsFieldKind::Select {
                            options: vec![
                                SettingOption {
                                    label: "YouTube",
                                    value: "youtube",
                                },
                                SettingOption {
                                    label: "YouTube Music",
                                    value: "youtube_music",
                                },
                                SettingOption {
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
            }],
        }
    }
}
