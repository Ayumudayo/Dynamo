use crate::{
    AppState, CommandCatalog, Context, DeploymentSettings, Error, GuildSettings, ModuleCatalog,
    ResolvedCommandState, ResolvedModuleState, resolve_command_state, resolve_module_state,
};

#[derive(Debug, Clone)]
pub struct ModuleAccess {
    pub state: ResolvedModuleState,
    pub denial_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommandAccess {
    pub state: ResolvedCommandState,
    pub denial_reason: Option<String>,
}

impl ModuleAccess {
    pub fn allowed(&self) -> bool {
        self.denial_reason.is_none()
    }
}

impl CommandAccess {
    pub fn allowed(&self) -> bool {
        self.denial_reason.is_none()
    }
}

pub fn module_access_for_state(
    catalog: &ModuleCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    module_id: &str,
) -> Result<ModuleAccess, Error> {
    let state = resolve_module_state(catalog, deployment, guild, module_id)
        .ok_or_else(|| anyhow::anyhow!("unknown module id `{module_id}`"))?;

    let denial_reason = if !state.installed {
        Some(format!(
            "The `{}` module is not installed for this deployment.",
            state.module.id
        ))
    } else if !state.deployment_enabled {
        Some(format!(
            "The `{}` module is disabled for this deployment.",
            state.module.id
        ))
    } else if guild.is_some() && !state.guild_enabled {
        Some(format!(
            "The `{}` module is disabled for this guild.",
            state.module.id
        ))
    } else {
        None
    };

    Ok(ModuleAccess {
        state,
        denial_reason,
    })
}

pub fn command_access_for_state(
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    command_id: &str,
) -> Result<CommandAccess, Error> {
    let state = resolve_command_state(
        module_catalog,
        command_catalog,
        deployment,
        guild,
        command_id,
    )
    .ok_or_else(|| anyhow::anyhow!("unknown command id `{command_id}`"))?;

    let denial_reason = if state.effective_enabled {
        None
    } else if !state.module_effective_enabled {
        Some(format!(
            "The `{}` module is currently disabled in this scope.",
            state.command.module_display_name
        ))
    } else if !state.installed {
        Some(format!(
            "The `{}` command is not installed in this deployment.",
            state.command.display_name
        ))
    } else if !state.deployment_enabled {
        Some(format!(
            "The `{}` command is disabled for this deployment.",
            state.command.display_name
        ))
    } else {
        Some(format!(
            "The `{}` command is disabled in this guild.",
            state.command.display_name
        ))
    };

    Ok(CommandAccess {
        state,
        denial_reason,
    })
}

pub async fn module_access_for_context(
    ctx: Context<'_>,
    module_id: &str,
) -> Result<ModuleAccess, Error> {
    let guild_id = ctx.guild_id().map(|id| id.get());
    let deployment = ctx
        .data()
        .persistence
        .deployment_settings_or_default()
        .await?;
    let guild = match guild_id {
        Some(guild_id) => Some(
            ctx.data()
                .persistence
                .guild_settings_or_default(guild_id)
                .await?,
        ),
        None => None,
    };
    module_access_for_state(
        &ctx.data().module_catalog,
        &deployment,
        guild.as_ref(),
        module_id,
    )
}

pub async fn module_access_for_app(
    data: &AppState,
    module_id: &str,
    guild_id: Option<u64>,
) -> Result<ModuleAccess, Error> {
    let deployment = data.persistence.deployment_settings_or_default().await?;
    let guild = match guild_id {
        Some(guild_id) => Some(data.persistence.guild_settings_or_default(guild_id).await?),
        None => None,
    };
    module_access_for_state(&data.module_catalog, &deployment, guild.as_ref(), module_id)
}

pub async fn command_access_for_context(ctx: Context<'_>) -> Result<CommandAccess, Error> {
    let guild_id = ctx.guild_id().map(|id| id.get());
    let deployment = ctx
        .data()
        .persistence
        .deployment_settings_or_default()
        .await?;
    let guild = match guild_id {
        Some(guild_id) => Some(
            ctx.data()
                .persistence
                .guild_settings_or_default(guild_id)
                .await?,
        ),
        None => None,
    };
    let command_id = ctx.command().qualified_name.replace(' ', "::");

    command_access_for_state(
        &ctx.data().module_catalog,
        &ctx.data().command_catalog,
        &deployment,
        guild.as_ref(),
        &command_id,
    )
}

pub async fn command_access_for_app(
    data: &AppState,
    guild_id: Option<u64>,
    command_id: &str,
) -> Result<CommandAccess, Error> {
    let deployment = data.persistence.deployment_settings_or_default().await?;
    let guild = match guild_id {
        Some(guild_id) => Some(data.persistence.guild_settings_or_default(guild_id).await?),
        None => None,
    };

    command_access_for_state(
        &data.module_catalog,
        &data.command_catalog,
        &deployment,
        guild.as_ref(),
        command_id,
    )
}
