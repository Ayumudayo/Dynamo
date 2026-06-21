use crate::commands::ticket;
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsSchema,
};
use dynamo_runtime_api::{AppState, Error};

pub struct TicketModule;

impl Module<AppState, Error> for TicketModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            crate::constants::MODULE_ID,
            "Ticket",
            "Ticket setup, channel creation, and close workflow.",
            ModuleCategory::Ticket,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![ticket()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        crate::settings::settings_schema()
    }
}
