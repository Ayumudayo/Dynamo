use crate::commands::{maint, pll, wtinv};
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsSchema,
};
use dynamo_runtime_api::{AppState, Error};

pub struct GameInfoModule;

impl Module<AppState, Error> for GameInfoModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            crate::constants::MODULE_ID,
            "Game Info",
            "Game utility commands and referral links.",
            ModuleCategory::GameInfo,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![wtinv(), maint(), pll()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        crate::referral::settings_schema()
    }
}
