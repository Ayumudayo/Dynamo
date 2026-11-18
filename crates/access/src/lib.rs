use dynamo_enablement::{
    ResolvedCommandState, ResolvedModuleState, resolve_command_state, resolve_module_state,
};
use dynamo_module_kit::{CommandCatalog, ModuleCatalog};
use dynamo_runtime_api::{AppState, Context, Error};
use dynamo_settings::{DeploymentSettings, GuildSettings};

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

    let is_guild_blocked =
        guild.is_some_and(|g| deployment.blocked_guild_ids.contains(&g.guild_id));
    let denial_reason = if is_guild_blocked {
        Some("This server is currently restricted by the bot owner.".to_string())
    } else if !state.installed {
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

    let is_guild_blocked =
        guild.is_some_and(|g| deployment.blocked_guild_ids.contains(&g.guild_id));
    let denial_reason = if is_guild_blocked {
        Some("This server is currently restricted by the bot owner.".to_string())
    } else if state.effective_enabled {
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
    let mut access =
        module_access_for_app(ctx.data(), module_id, ctx.guild_id().map(|id| id.get())).await?;
    if dynamo_runtime_api::is_korean_locale(ctx.locale()) && access.denial_reason.is_some() {
        let is_guild_blocked = if let Some(guild_id) = ctx.guild_id() {
            ctx.data()
                .deployment_settings_cached()
                .await?
                .blocked_guild_ids
                .contains(&guild_id.get())
        } else {
            false
        };
        let reason = if is_guild_blocked {
            "이 서버는 현재 봇 소유자에 의해 사용이 제한되어 있습니다.".to_string()
        } else if !access.state.installed {
            format!(
                "`{}` 모듈이 현재 배포에 설치되어 있지 않습니다.",
                access.state.module.display_name
            )
        } else if !access.state.deployment_enabled {
            format!(
                "`{}` 모듈이 전역 설정에서 비활성화되어 있습니다.",
                access.state.module.display_name
            )
        } else {
            format!(
                "`{}` 모듈이 이 서버에서 비활성화되어 있습니다.",
                access.state.module.display_name
            )
        };
        access.denial_reason = Some(reason);
    }
    Ok(access)
}

pub async fn module_access_for_app(
    data: &AppState,
    module_id: &str,
    guild_id: Option<u64>,
) -> Result<ModuleAccess, Error> {
    let deployment = data.deployment_settings_cached().await?;
    let guild = match guild_id {
        Some(guild_id) => Some(data.guild_settings_cached(guild_id).await?),
        None => None,
    };
    module_access_for_state(&data.module_catalog, &deployment, guild.as_ref(), module_id)
}

pub async fn command_access_for_context(ctx: Context<'_>) -> Result<CommandAccess, Error> {
    let command_id = ctx.command().qualified_name.replace(' ', "::");
    let mut access =
        command_access_for_app(ctx.data(), ctx.guild_id().map(|id| id.get()), &command_id).await?;
    if dynamo_runtime_api::is_korean_locale(ctx.locale()) && access.denial_reason.is_some() {
        let is_guild_blocked = if let Some(guild_id) = ctx.guild_id() {
            ctx.data()
                .deployment_settings_cached()
                .await?
                .blocked_guild_ids
                .contains(&guild_id.get())
        } else {
            false
        };
        let reason = if is_guild_blocked {
            "이 서버는 현재 봇 소유자에 의해 사용이 제한되어 있습니다.".to_string()
        } else if !access.state.module_effective_enabled {
            format!(
                "`{}` 모듈이 현재 범위에서 비활성화되어 있습니다.",
                access.state.command.module_display_name
            )
        } else if !access.state.installed {
            format!(
                "`{}` 명령어가 현재 배포에 설치되어 있지 않습니다.",
                access.state.command.display_name
            )
        } else if !access.state.deployment_enabled {
            format!(
                "`{}` 명령어가 전역 설정에서 비활성화되어 있습니다.",
                access.state.command.display_name
            )
        } else {
            format!(
                "`{}` 명령어가 이 서버에서 비활성화되어 있습니다.",
                access.state.command.display_name
            )
        };
        access.denial_reason = Some(reason);
    }
    Ok(access)
}

pub async fn command_access_for_app(
    data: &AppState,
    guild_id: Option<u64>,
    command_id: &str,
) -> Result<CommandAccess, Error> {
    let deployment = data.deployment_settings_cached().await?;
    let guild = match guild_id {
        Some(guild_id) => Some(data.guild_settings_cached(guild_id).await?),
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

#[cfg(test)]
mod tests {
    use dynamo_module_kit::{
        CommandCatalog, CommandCatalogEntry, CommandDescriptor, ModuleCatalog, ModuleCatalogEntry,
        ModuleCategory, ModuleDescriptor, SettingsSchema,
    };
    use dynamo_settings::{DeploymentSettings, GuildSettings};

    use super::{command_access_for_state, module_access_for_state};

    #[test]
    fn blocked_guild_is_denied_for_modules_and_commands() {
        let module = ModuleDescriptor {
            id: "info",
            display_name: "Info",
            description: "Info module",
            category: ModuleCategory::Info,
            enabled_by_default: true,
            required_intents_bits: 0,
        };
        let module_catalog = ModuleCatalog {
            entries: vec![ModuleCatalogEntry {
                module,
                settings: SettingsSchema::empty(),
            }],
        };
        let command_catalog = CommandCatalog {
            entries: vec![CommandCatalogEntry {
                command: CommandDescriptor {
                    id: "ping".to_string(),
                    module_id: "info",
                    module_display_name: "Info",
                    top_level_name: "ping".to_string(),
                    display_name: "ping".to_string(),
                    qualified_name: "ping".to_string(),
                    category: None,
                    description: None,
                },
                settings: SettingsSchema::empty(),
            }],
        };
        let deployment = DeploymentSettings {
            blocked_guild_ids: vec![42],
            ..DeploymentSettings::default()
        };
        let guild = GuildSettings::for_guild(42);

        let mod_access =
            module_access_for_state(&module_catalog, &deployment, Some(&guild), "info")
                .expect("module access");
        assert!(!mod_access.allowed());

        let cmd_access = command_access_for_state(
            &module_catalog,
            &command_catalog,
            &deployment,
            Some(&guild),
            "ping",
        )
        .expect("command access");
        assert!(!cmd_access.allowed());
    }
}
