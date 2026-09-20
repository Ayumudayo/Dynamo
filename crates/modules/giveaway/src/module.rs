use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{AppState, Error};

use crate::commands::giveaway;

pub struct GiveawayModule;

impl Module<AppState, Error> for GiveawayModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            crate::constants::MODULE_ID,
            "Giveaway",
            "Optional first-party giveaway workflow with persisted entry tracking.",
            ModuleCategory::Giveaway,
            false,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_MEMBERS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![giveaway()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "giveaway",
                title: "Giveaway",
                description: Some("Optional giveaway module configuration."),
                fields: vec![
                    SettingsField {
                        key: "default_channel",
                        label: "Default channel ID",
                        help_text: Some("Primary channel for giveaway announcements."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "button_label",
                        label: "Entry button label",
                        help_text: Some("Button text shown on giveaway messages."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                ],
            }],
        }
    }
}
