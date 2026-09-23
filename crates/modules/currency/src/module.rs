use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsSchema,
};
use dynamo_runtime_api::{AppState, Error};

#[path = "commands.rs"]
mod commands;
#[path = "currency.rs"]
mod currency;
#[path = "render.rs"]
mod render;
#[path = "settings.rs"]
mod settings;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub(super) const MODULE_ID: &str = "currency";

pub struct CurrencyModule;

impl Module<AppState, Error> for CurrencyModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Currency",
            "Exchange rate commands backed by Toss Invest midRate data.",
            ModuleCategory::Currency,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![commands::exchange(), commands::rate()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::empty()
    }

    fn command_settings_schema(&self, command_id: &str) -> SettingsSchema {
        settings::command_settings_schema(command_id)
    }
}
