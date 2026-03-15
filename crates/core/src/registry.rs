use std::time::Instant;

use crate::{
    DiscordCommand, GatewayIntents, Module, ModuleCatalog, ModuleCatalogEntry, ModuleManifest,
};

pub struct ModuleRegistry {
    modules: Vec<Box<dyn Module>>,
    catalog: ModuleCatalog,
}

impl ModuleRegistry {
    pub fn new(modules: Vec<Box<dyn Module>>) -> Self {
        let catalog = ModuleCatalog {
            entries: modules
                .iter()
                .map(|module| ModuleCatalogEntry {
                    module: module.manifest().into(),
                    settings: module.settings_schema(),
                })
                .collect(),
        };

        Self { modules, catalog }
    }

    pub fn commands(&self) -> Vec<DiscordCommand> {
        self.modules
            .iter()
            .flat_map(|module| module.commands())
            .collect()
    }

    pub fn manifests(&self) -> Vec<ModuleManifest> {
        self.modules
            .iter()
            .map(|module| module.manifest())
            .collect()
    }

    pub fn catalog(&self) -> &ModuleCatalog {
        &self.catalog
    }
}

#[derive(Debug)]
pub struct AppState {
    pub started_at: Instant,
    pub module_catalog: ModuleCatalog,
}

impl AppState {
    pub fn new(module_catalog: ModuleCatalog) -> Self {
        Self {
            started_at: Instant::now(),
            module_catalog,
        }
    }
}

pub fn aggregate_intents(manifests: impl IntoIterator<Item = ModuleManifest>) -> GatewayIntents {
    manifests
        .into_iter()
        .fold(GatewayIntents::GUILDS, |intents, manifest| {
            intents | manifest.required_intents
        })
}
