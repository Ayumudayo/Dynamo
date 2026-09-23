use dynamo_config::{AppConfig, CommandSyncConfig, DiscordConfig};
use dynamo_module_kit::{CommandCatalog, GatewayIntents, ModuleCatalog};
use dynamo_observability::{
    StartupPhase, StartupReport, StartupStatus, catalog_startup_summary, format_gateway_intents,
    format_preview_kv_list, format_preview_list, scope_startup_summary,
};
use dynamo_persistence_api::Persistence;
use dynamo_runtime_api::{AppState, Error};
use dynamo_services_api::ServiceRegistry;
use dynamo_settings::{DeploymentSettings, GuildSettings};

use crate::background::GIVEAWAY_POLL_INTERVAL_SECONDS;

#[allow(clippy::too_many_arguments)]
pub(super) fn build_bot_preconnect_report(
    config: &AppConfig,
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    intents: GatewayIntents,
    persistence: &Persistence,
    services: &ServiceRegistry,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
) -> StartupReport {
    let catalog_summary = catalog_startup_summary(module_catalog, command_catalog);
    let scope_summary = scope_startup_summary(module_catalog, command_catalog, deployment, guild);
    let submitted_top_level_commands =
        dynamo_app::create_application_commands_for_scope(deployment, guild).len();
    let repositories = collect_persistence_labels(persistence);
    let services_wired = collect_service_labels(services);
    let command_scope = format_command_scope(&config.discord);
    let sync_target = format_sync_target(&config.discord);

    let mut report = StartupReport::new("bot");
    report.add_phase(
        StartupPhase::new(
            "config",
            StartupStatus::Ok,
            format!(
                "scope={command_scope} sync={}s intents={}",
                config.commands.sync_interval_seconds,
                format_gateway_intents(intents)
            ),
        )
        .detail("command_scope", command_scope)
        .detail(
            "dev_guild_id",
            config
                .discord
                .dev_guild_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
        )
        .detail(
            "sync_interval_seconds",
            config.commands.sync_interval_seconds.to_string(),
        )
        .detail("optional_module_flags", "none".to_string())
        .detail("aggregated_intents", format_gateway_intents(intents)),
    );
    report.add_phase(
        StartupPhase::new(
            "registry",
            StartupStatus::Ok,
            format!(
                "modules={} leaf_commands={}",
                catalog_summary.module_count, catalog_summary.discovered_leaf_command_count
            ),
        )
        .detail(
            "module_ids",
            format_preview_list(&catalog_summary.module_ids, 5),
        )
        .detail(
            "leaf_command_count",
            catalog_summary.discovered_leaf_command_count.to_string(),
        )
        .detail(
            "per_module_command_counts",
            format_preview_kv_list(&catalog_summary.per_module_command_counts, 5),
        ),
    );

    let persistence_status = if persistence.database_name.is_some() {
        StartupStatus::Ok
    } else {
        StartupStatus::Warn
    };
    report.add_phase(
        StartupPhase::new(
            "persistence",
            persistence_status,
            if let Some(database_name) = persistence.database_name.as_deref() {
                format!(
                    "db={} repos={} services={}",
                    database_name,
                    repositories.len(),
                    services_wired.len()
                )
            } else {
                format!(
                    "db=none repos={} services={}",
                    repositories.len(),
                    services_wired.len()
                )
            },
        )
        .detail(
            "database",
            persistence
                .database_name
                .clone()
                .unwrap_or_else(|| "none".to_string()),
        )
        .detail(
            "repositories_wired",
            if repositories.is_empty() {
                "none".to_string()
            } else {
                format_preview_list(&repositories, 5)
            },
        )
        .detail(
            "services_wired",
            if services_wired.is_empty() {
                "none".to_string()
            } else {
                format_preview_list(&services_wired, 5)
            },
        )
        .detail(
            "exchange_rate_targets",
            services
                .exchange_rates
                .as_ref()
                .map(|service| service.cache_target_count().to_string())
                .unwrap_or_else(|| "0".to_string()),
        ),
    );

    let sync_status = if scope_summary.active_command_count == 0 {
        StartupStatus::Warn
    } else {
        StartupStatus::Ok
    };
    report.add_phase(
        StartupPhase::new(
            "sync_target",
            sync_status,
            format!(
                "target={sync_target} active={} filtered={} top_level={}",
                scope_summary.active_command_count,
                scope_summary.filtered_command_count,
                submitted_top_level_commands
            ),
        )
        .detail("target", sync_target)
        .detail(
            "submitted_top_level_commands",
            submitted_top_level_commands.to_string(),
        )
        .detail(
            "discovered_leaf_commands",
            scope_summary.discovered_leaf_command_count.to_string(),
        )
        .detail(
            "active_leaf_commands",
            scope_summary.active_command_count.to_string(),
        )
        .detail(
            "filtered_leaf_commands",
            scope_summary.filtered_command_count.to_string(),
        )
        .detail(
            "active_modules",
            if scope_summary.active_module_ids.is_empty() {
                "none".to_string()
            } else {
                format_preview_list(&scope_summary.active_module_ids, 5)
            },
        )
        .detail(
            "disabled_modules",
            scope_summary.disabled_module_count.to_string(),
        )
        .detail(
            "disabled_commands",
            scope_summary.disabled_command_count.to_string(),
        ),
    );

    report
}

pub(super) async fn build_bot_runtime_report(
    discord_config: &DiscordConfig,
    command_sync_config: &CommandSyncConfig,
    app_state: &AppState,
    ready_user: &str,
) -> Result<StartupReport, Error> {
    let deployment = app_state
        .persistence
        .deployment_settings_or_default()
        .await?;
    let guild_settings = if discord_config.register_globally {
        None
    } else {
        Some(
            app_state
                .persistence
                .guild_settings_or_default(discord_config.dev_guild_id.unwrap_or_default())
                .await?,
        )
    };
    let scope_summary = scope_startup_summary(
        &app_state.module_catalog,
        &app_state.command_catalog,
        &deployment,
        guild_settings.as_ref(),
    );
    let submitted_top_level_commands =
        dynamo_app::create_application_commands_for_scope(&deployment, guild_settings.as_ref())
            .len();

    let mut report = StartupReport::new("bot");
    let exchange_rate_status = if let Some(service) = &app_state.services.exchange_rates {
        Some(service.cache_status().await?)
    } else {
        None
    };
    report.add_phase(
        StartupPhase::new(
            "runtime",
            StartupStatus::Ok,
            format!(
                "user={ready_user} active={} giveaway=on",
                scope_summary.active_command_count
            ),
        )
        .detail("ready_user", ready_user)
        .detail(
            "command_sync_loop",
            format!(
                "enabled every {}s",
                command_sync_config.sync_interval_seconds.max(5)
            ),
        )
        .detail(
            "giveaway_poll_loop",
            format!("enabled every {}s", GIVEAWAY_POLL_INTERVAL_SECONDS),
        )
        .detail(
            "exchange_rate_refresh_loop",
            if app_state.services.exchange_rates.is_some() {
                format!(
                    "enabled every {}s",
                    dynamo_provider_tossinvest::exchange_refresh_interval_seconds()
                )
            } else {
                "disabled".to_string()
            },
        )
        .detail(
            "exchange_rate_status",
            exchange_rate_status
                .as_ref()
                .map(|status| {
                    format!(
                        "targets={} last_refresh={} persisted={}",
                        status.target_currency_count,
                        status
                            .last_refresh_at
                            .map(|value| value.to_rfc3339())
                            .unwrap_or_else(|| "none".to_string()),
                        status.uses_persisted_cache
                    )
                })
                .unwrap_or_else(|| "not configured".to_string()),
        )
        .detail("sync_target", format_sync_target(discord_config))
        .detail(
            "submitted_top_level_commands",
            submitted_top_level_commands.to_string(),
        )
        .detail(
            "active_modules",
            scope_summary.active_module_count.to_string(),
        )
        .detail(
            "active_leaf_commands",
            scope_summary.active_command_count.to_string(),
        )
        .detail(
            "filtered_leaf_commands",
            scope_summary.filtered_command_count.to_string(),
        ),
    );

    Ok(report)
}

fn collect_persistence_labels(persistence: &Persistence) -> Vec<String> {
    let mut labels = Vec::new();
    if persistence.guild_settings.is_some() {
        labels.push("guild_settings".to_string());
    }
    if persistence.deployment_settings.is_some() {
        labels.push("deployment_settings".to_string());
    }
    if persistence.provider_state.is_some() {
        labels.push("provider_state".to_string());
    }
    if persistence.suggestions.is_some() {
        labels.push("suggestions".to_string());
    }
    if persistence.giveaways.is_some() {
        labels.push("giveaways".to_string());
    }
    if persistence.invites.is_some() {
        labels.push("invites".to_string());
    }
    if persistence.member_stats.is_some() {
        labels.push("member_stats".to_string());
    }
    if persistence.warning_logs.is_some() {
        labels.push("warning_logs".to_string());
    }
    if persistence.dashboard_audit_logs.is_some() {
        labels.push("dashboard_audit_logs".to_string());
    }
    labels
}

fn collect_service_labels(services: &ServiceRegistry) -> Vec<String> {
    let mut labels = Vec::new();
    if services.stock_quotes.is_some() {
        labels.push("stock_quotes".to_string());
    }
    if services.exchange_rates.is_some() {
        labels.push("exchange_rates".to_string());
    }
    labels
}

fn format_command_scope(discord_config: &DiscordConfig) -> String {
    if discord_config.register_globally {
        "global".to_string()
    } else {
        format!(
            "guild {}",
            discord_config
                .dev_guild_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        )
    }
}

fn format_sync_target(discord_config: &DiscordConfig) -> String {
    if discord_config.register_globally {
        "global application commands".to_string()
    } else {
        format!(
            "guild {}",
            discord_config
                .dev_guild_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        )
    }
}

#[cfg(test)]
mod tests {
    use dynamo_config::DiscordConfig;
    use dynamo_services_api::ServiceRegistry;

    use super::{collect_service_labels, format_command_scope, format_sync_target};

    #[test]
    fn service_labels_omit_exchange_when_exchange_service_is_disabled() {
        let labels = collect_service_labels(&ServiceRegistry::new(None, None));

        assert!(!labels.contains(&"exchange_rates".to_string()));
    }

    #[test]
    fn command_scope_uses_global_label_for_global_registration() {
        let config = DiscordConfig {
            token: "token".to_string(),
            register_globally: true,
            dev_guild_id: Some(42),
        };

        assert_eq!(format_command_scope(&config), "global");
        assert_eq!(format_sync_target(&config), "global application commands");
    }

    #[test]
    fn command_scope_uses_guild_label_for_guild_registration() {
        let config = DiscordConfig {
            token: "token".to_string(),
            register_globally: false,
            dev_guild_id: Some(42),
        };

        assert_eq!(format_command_scope(&config), "guild 42");
        assert_eq!(format_sync_target(&config), "guild 42");
    }
}
