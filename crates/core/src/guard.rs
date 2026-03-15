use crate::{
    Context, DeploymentSettings, Error, GuildSettings, ModuleCatalog, ResolvedModuleState,
    resolve_module_state,
};

#[derive(Debug, Clone)]
pub struct ModuleAccess {
    pub state: ResolvedModuleState,
    pub denial_reason: Option<String>,
}

impl ModuleAccess {
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

pub async fn module_access_for_context(
    ctx: Context<'_>,
    module_id: &str,
) -> Result<ModuleAccess, Error> {
    let guild_id = ctx.guild_id().map(|id| id.get());
    let deployment = ctx.data().persistence.deployment_settings_or_default().await?;
    let guild = match guild_id {
        Some(guild_id) => Some(ctx.data().persistence.guild_settings_or_default(guild_id).await?),
        None => None,
    };
    module_access_for_state(&ctx.data().module_catalog, &deployment, guild.as_ref(), module_id)
}
