use dynamo_core::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection,
};

const MODULE_ID: &str = "giveaway";

pub struct GiveawayModule;

impl Module for GiveawayModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Giveaway",
            "Optional giveaway workflow module reserved for first-party opt-in deployments.",
            ModuleCategory::Giveaway,
            false,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_MEMBERS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        Vec::new()
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
                        key: "reaction_emoji",
                        label: "Entry reaction",
                        help_text: Some("Emoji shown for giveaway entry interactions."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                ],
            }],
        }
    }
}
