use crate::{
    commands::{etf, stock},
    constants::MODULE_ID,
    settings::etf_ticker_field,
};
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsSchema,
    SettingsSection,
};
use dynamo_runtime_api::{AppState, Error};

pub struct StockModule;

impl Module<AppState, Error> for StockModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Stock",
            "Stock and ETF quote commands with refresh sessions.",
            ModuleCategory::Stocks,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![stock(), etf()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        crate::settings::settings_schema()
    }

    fn command_settings_schema(&self, command_id: &str) -> SettingsSchema {
        match command_id {
            "etf" => SettingsSchema {
                sections: vec![SettingsSection {
                    id: "etf",
                    title: "ETF Tickers",
                    description: Some("Set up to five ETF tickers for /etf in display order."),
                    fields: vec![
                        etf_ticker_field("ticker_1", "Ticker 1"),
                        etf_ticker_field("ticker_2", "Ticker 2"),
                        etf_ticker_field("ticker_3", "Ticker 3"),
                        etf_ticker_field("ticker_4", "Ticker 4"),
                        etf_ticker_field("ticker_5", "Ticker 5"),
                    ],
                }],
            },
            _ => SettingsSchema::empty(),
        }
    }
}
