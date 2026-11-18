use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{AppState, Error};

use crate::{commands::suggest, constants::MODULE_ID};

pub struct SuggestionModule;

impl Module<AppState, Error> for SuggestionModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Suggestion",
            "Guild suggestion board with approval and rejection workflows.",
            ModuleCategory::Suggestion,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![suggest()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "suggestions",
                title: "Suggestions",
                description: Some("Configure the suggestion board channels and moderator roles."),
                fields: vec![
                    SettingsField {
                        key: "channel_id",
                        label: "Suggestion channel",
                        help_text: Some("Guild text channel where new suggestions are posted."),
                        required: false,
                        kind: SettingsFieldKind::Channel,
                    },
                    SettingsField {
                        key: "approved_channel_id",
                        label: "Approved channel",
                        help_text: Some(
                            "Optional target channel for approved suggestions. Leave empty to edit in place.",
                        ),
                        required: false,
                        kind: SettingsFieldKind::Channel,
                    },
                    SettingsField {
                        key: "rejected_channel_id",
                        label: "Rejected channel",
                        help_text: Some(
                            "Optional target channel for rejected suggestions. Leave empty to edit in place.",
                        ),
                        required: false,
                        kind: SettingsFieldKind::Channel,
                    },
                    SettingsField {
                        key: "staff_role_ids",
                        label: "Staff roles",
                        help_text: Some("Roles that may moderate suggestions."),
                        required: false,
                        kind: SettingsFieldKind::RoleList,
                    },
                ],
            }],
        }
    }
}
