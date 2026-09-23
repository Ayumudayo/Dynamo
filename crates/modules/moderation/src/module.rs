use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsSchema,
};
use dynamo_runtime_api::{AppState, Error};

use crate::{
    member_actions::{kick, nick, timeout, untimeout},
    settings::settings_schema,
    user_actions::{ban, softban, unban},
    warnings::{warn, warnings},
};

pub(crate) const MODULE_ID: &str = "moderation";
pub(crate) const DEFAULT_TIMEOUT_HOURS: i64 = 24;

pub struct ModerationModule;

impl Module<AppState, Error> for ModerationModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Moderation",
            "Slash-first moderation commands with warning ledger support.",
            ModuleCategory::Moderation,
            true,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_MEMBERS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![
            warn(),
            warnings(),
            timeout(),
            untimeout(),
            kick(),
            ban(),
            unban(),
            softban(),
            nick(),
        ]
    }

    fn settings_schema(&self) -> SettingsSchema {
        settings_schema()
    }
}
