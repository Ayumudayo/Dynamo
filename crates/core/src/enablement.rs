use serde::Serialize;

use crate::{
    CommandCatalog, CommandDescriptor, DeploymentSettings, GuildSettings, ModuleCatalog,
    ModuleDescriptor,
};

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedModuleState {
    pub module: ModuleDescriptor,
    pub installed: bool,
    pub deployment_enabled: bool,
    pub guild_enabled: bool,
    pub effective_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedCommandState {
    pub command: CommandDescriptor,
    pub module_effective_enabled: bool,
    pub installed: bool,
    pub deployment_enabled: bool,
    pub guild_enabled: bool,
    pub effective_enabled: bool,
}

pub fn resolve_module_states(
    catalog: &ModuleCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> Vec<ResolvedModuleState> {
    catalog
        .entries
        .iter()
        .map(|entry| {
            let deployment_settings = deployment.modules.get(entry.module.id);
            let guild_settings = guild.and_then(|settings| settings.modules.get(entry.module.id));

            let installed = deployment_settings.map(|s| s.installed).unwrap_or(true);
            let deployment_enabled = deployment_settings
                .map(|s| s.enabled)
                .unwrap_or(entry.module.enabled_by_default);
            let guild_enabled = guild_settings.map(|s| s.enabled).unwrap_or(true);
            let effective_enabled = installed && deployment_enabled && guild_enabled;

            ResolvedModuleState {
                module: entry.module.clone(),
                installed,
                deployment_enabled,
                guild_enabled,
                effective_enabled,
            }
        })
        .collect()
}

pub fn resolve_module_state(
    catalog: &ModuleCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    module_id: &str,
) -> Option<ResolvedModuleState> {
    resolve_module_states(catalog, deployment, guild)
        .into_iter()
        .find(|state| state.module.id == module_id)
}

pub fn resolve_command_states(
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> Vec<ResolvedCommandState> {
    let module_states = resolve_module_states(module_catalog, deployment, guild);

    command_catalog
        .entries
        .iter()
        .filter_map(|entry| {
            let module_state = module_states
                .iter()
                .find(|state| state.module.id == entry.command.module_id)?;
            let deployment_settings = deployment.commands.get(&entry.command.id);
            let guild_settings =
                guild.and_then(|settings| settings.commands.get(&entry.command.id));

            let installed = deployment_settings.map(|s| s.installed).unwrap_or(true);
            let deployment_enabled = deployment_settings.map(|s| s.enabled).unwrap_or(true);
            let guild_enabled = guild_settings.map(|s| s.enabled).unwrap_or(true);
            let effective_enabled =
                module_state.effective_enabled && installed && deployment_enabled && guild_enabled;

            Some(ResolvedCommandState {
                command: entry.command.clone(),
                module_effective_enabled: module_state.effective_enabled,
                installed,
                deployment_enabled,
                guild_enabled,
                effective_enabled,
            })
        })
        .collect()
}

pub fn resolve_command_state(
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    command_id: &str,
) -> Option<ResolvedCommandState> {
    resolve_command_states(module_catalog, command_catalog, deployment, guild)
        .into_iter()
        .find(|state| state.command.id == command_id)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{
        CommandCatalog, CommandCatalogEntry, CommandDescriptor, DeploymentModuleSettings,
        DeploymentSettings, GuildCommandSettings, GuildModuleSettings, GuildSettings,
        ModuleCatalog, ModuleCatalogEntry, ModuleCategory, ModuleDescriptor, SettingsSchema,
    };

    use super::{resolve_command_states, resolve_module_states};

    fn catalog() -> ModuleCatalog {
        ModuleCatalog {
            entries: vec![ModuleCatalogEntry {
                module: ModuleDescriptor {
                    id: "info",
                    display_name: "Info",
                    description: "info module",
                    category: ModuleCategory::Info,
                    enabled_by_default: true,
                    required_intents_bits: 1,
                },
                settings: SettingsSchema::empty(),
            }],
        }
    }

    fn command_catalog() -> CommandCatalog {
        CommandCatalog {
            entries: vec![CommandCatalogEntry {
                command: CommandDescriptor {
                    id: "ping".to_string(),
                    module_id: "info",
                    module_display_name: "Info",
                    top_level_name: "ping".to_string(),
                    display_name: "/ping".to_string(),
                    qualified_name: "ping".to_string(),
                    category: Some("Info".to_string()),
                    description: Some("info module".to_string()),
                },
                settings: SettingsSchema::empty(),
            }],
        }
    }

    #[test]
    fn defaults_to_enabled_when_no_overrides_exist() {
        let states = resolve_module_states(&catalog(), &DeploymentSettings::default(), None);
        assert_eq!(states.len(), 1);
        assert!(states[0].effective_enabled);
    }

    #[test]
    fn deployment_disable_overrides_manifest_default() {
        let mut deployment = DeploymentSettings::default();
        deployment.modules.insert(
            "info".to_string(),
            DeploymentModuleSettings {
                installed: true,
                enabled: false,
            },
        );

        let states = resolve_module_states(&catalog(), &deployment, None);
        assert!(!states[0].effective_enabled);
        assert!(!states[0].deployment_enabled);
    }

    #[test]
    fn guild_disable_overrides_deployment_enablement() {
        let mut guild_modules = BTreeMap::new();
        guild_modules.insert(
            "info".to_string(),
            GuildModuleSettings {
                enabled: false,
                configuration: serde_json::Value::Null,
            },
        );

        let guild = GuildSettings {
            guild_id: 1,
            modules: guild_modules,
            commands: BTreeMap::new(),
        };

        let states =
            resolve_module_states(&catalog(), &DeploymentSettings::default(), Some(&guild));
        assert!(!states[0].effective_enabled);
        assert!(!states[0].guild_enabled);
    }

    #[test]
    fn command_disable_overrides_module_enablement() {
        let mut guild_commands = BTreeMap::new();
        guild_commands.insert(
            "ping".to_string(),
            GuildCommandSettings {
                enabled: false,
                configuration: serde_json::Value::Null,
            },
        );

        let guild = GuildSettings {
            guild_id: 1,
            modules: BTreeMap::new(),
            commands: guild_commands,
        };

        let states = resolve_command_states(
            &catalog(),
            &command_catalog(),
            &DeploymentSettings::default(),
            Some(&guild),
        );
        assert_eq!(states.len(), 1);
        assert!(!states[0].effective_enabled);
        assert!(!states[0].guild_enabled);
        assert!(states[0].module_effective_enabled);
    }
}
