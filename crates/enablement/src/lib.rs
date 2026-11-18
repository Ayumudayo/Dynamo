use dynamo_module_kit::{CommandCatalog, CommandDescriptor, ModuleCatalog, ModuleDescriptor};
use dynamo_settings::{DeploymentSettings, GuildSettings};
use serde::Serialize;

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

fn resolve_module_entry(
    entry: &dynamo_module_kit::ModuleCatalogEntry,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> ResolvedModuleState {
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
}

fn resolve_command_entry(
    entry: &dynamo_module_kit::CommandCatalogEntry,
    module_effective_enabled: bool,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> ResolvedCommandState {
    let deployment_settings = deployment.commands.get(&entry.command.id);
    let guild_settings = guild.and_then(|settings| settings.commands.get(&entry.command.id));

    let installed = deployment_settings.map(|s| s.installed).unwrap_or(true);
    let deployment_enabled = deployment_settings.map(|s| s.enabled).unwrap_or(true);
    let guild_enabled = guild_settings.map(|s| s.enabled).unwrap_or(true);
    let effective_enabled =
        module_effective_enabled && installed && deployment_enabled && guild_enabled;

    ResolvedCommandState {
        command: entry.command.clone(),
        module_effective_enabled,
        installed,
        deployment_enabled,
        guild_enabled,
        effective_enabled,
    }
}

pub fn resolve_module_states(
    catalog: &ModuleCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> Vec<ResolvedModuleState> {
    catalog
        .entries
        .iter()
        .map(|entry| resolve_module_entry(entry, deployment, guild))
        .collect()
}

pub fn resolve_module_state(
    catalog: &ModuleCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    module_id: &str,
) -> Option<ResolvedModuleState> {
    catalog
        .find_by_id(module_id)
        .map(|entry| resolve_module_entry(entry, deployment, guild))
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
            Some(resolve_command_entry(
                entry,
                module_state.effective_enabled,
                deployment,
                guild,
            ))
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
    let entry = command_catalog.find_by_id(command_id)?;
    let module_entry = module_catalog.find_by_id(entry.command.module_id)?;
    let module_state = resolve_module_entry(module_entry, deployment, guild);
    Some(resolve_command_entry(
        entry,
        module_state.effective_enabled,
        deployment,
        guild,
    ))
}
