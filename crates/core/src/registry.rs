use std::time::Instant;

use crate::{
    CommandCatalog, CommandCatalogEntry, CommandDescriptor, DiscordCommand, GatewayIntents, Module,
    ModuleCatalog, ModuleCatalogEntry, ModuleManifest, Persistence, ServiceRegistry,
};

pub struct ModuleRegistry {
    modules: Vec<Box<dyn Module>>,
    catalog: ModuleCatalog,
    command_catalog: CommandCatalog,
}

impl ModuleRegistry {
    pub fn new(modules: Vec<Box<dyn Module>>) -> Self {
        let mut command_entries = Vec::new();
        let catalog = ModuleCatalog {
            entries: modules
                .iter()
                .map(|module| ModuleCatalogEntry {
                    module: module.manifest().into(),
                    settings: module.settings_schema(),
                })
                .collect(),
        };

        for module in &modules {
            let manifest = module.manifest();
            for command in module.commands() {
                collect_command_entries(
                    module.as_ref(),
                    manifest,
                    &command,
                    &mut Vec::new(),
                    &mut command_entries,
                );
            }
        }

        Self {
            modules,
            catalog,
            command_catalog: CommandCatalog {
                entries: command_entries,
            },
        }
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

    pub fn command_catalog(&self) -> &CommandCatalog {
        &self.command_catalog
    }
}

#[derive(Clone)]
pub struct AppState {
    pub started_at: Instant,
    pub module_catalog: ModuleCatalog,
    pub command_catalog: CommandCatalog,
    pub persistence: Persistence,
    pub services: ServiceRegistry,
}

impl AppState {
    pub fn new(
        module_catalog: ModuleCatalog,
        command_catalog: CommandCatalog,
        persistence: Persistence,
        services: ServiceRegistry,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            module_catalog,
            command_catalog,
            persistence,
            services,
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

fn collect_command_entries(
    module: &dyn Module,
    manifest: ModuleManifest,
    command: &DiscordCommand,
    parent_segments: &mut Vec<String>,
    entries: &mut Vec<CommandCatalogEntry>,
) {
    parent_segments.push(command.name.clone());

    if command.subcommands.is_empty() {
        let qualified_name = parent_segments.join(" ");
        let command_id = parent_segments.join("::");
        entries.push(CommandCatalogEntry {
            command: CommandDescriptor {
                id: command_id.clone(),
                module_id: manifest.id,
                module_display_name: manifest.display_name,
                top_level_name: parent_segments.first().cloned().unwrap_or_default(),
                display_name: format!("/{}", qualified_name),
                qualified_name: qualified_name.clone(),
                category: command.category.clone(),
                description: command.description.clone(),
            },
            settings: module.command_settings_schema(&command_id),
        });
    } else {
        for subcommand in &command.subcommands {
            collect_command_entries(module, manifest, subcommand, parent_segments, entries);
        }
    }

    parent_segments.pop();
}
