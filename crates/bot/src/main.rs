use std::{collections::HashMap, sync::OnceLock, time::Duration};

use dynamo_core::{
    AppConfig, AppState, CommandCatalog, CommandSyncConfig, DeploymentSettings, DiscordConfig,
    Error, GatewayIntents, GuildSettings, ModuleCatalog, OptionalModulesConfig, Persistence,
    ServiceRegistry, StartupPhase, StartupReport, StartupStatus, aggregate_intents,
    catalog_startup_summary, format_gateway_intents, format_kv_list, scope_startup_summary,
};
use poise::{CreateReply, FrameworkError, serenity_prelude as serenity};
use songbird::SerenityInit;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

const GIVEAWAY_POLL_INTERVAL_SECONDS: u64 = 15;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = AppConfig::from_env()?;
    let registry = dynamo_app::module_registry_with_optional(&config.optional_modules);
    let persistence = dynamo_app::persistence_from_env().await?;
    let services = dynamo_app::services_from_persistence(&persistence)?;
    let manifests = registry.manifests();
    let commands = registry.commands();
    let intents = aggregate_intents(manifests.iter().copied());
    let setup_catalog = registry.catalog().clone();
    let setup_command_catalog = registry.command_catalog().clone();
    let discord_config = config.discord.clone();
    let command_sync_config = config.commands.clone();
    let optional_modules = config.optional_modules.clone();
    let setup_persistence = persistence.clone();
    let setup_services = services.clone();
    let startup_deployment = persistence.deployment_settings_or_default().await?;
    let startup_guild_settings = if config.discord.register_globally {
        None
    } else {
        Some(
            persistence
                .guild_settings_or_default(config.discord.dev_guild_id.unwrap_or_default())
                .await?,
        )
    };

    build_bot_preconnect_report(
        &config,
        &setup_catalog,
        &setup_command_catalog,
        intents,
        &persistence,
        &services,
        &startup_deployment,
        startup_guild_settings.as_ref(),
    )
    .log();

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            event_handler,
            on_error: framework_on_error,
            command_check: Some(command_check),
            commands,
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            let discord_config = discord_config.clone();
            let command_sync_config = command_sync_config.clone();
            let optional_modules = optional_modules.clone();
            let setup_catalog = setup_catalog.clone();
            let setup_command_catalog = setup_command_catalog.clone();
            let setup_persistence = setup_persistence.clone();
            let setup_services = setup_services.clone();

            Box::pin(async move {
                let app_state = AppState::new(
                    setup_catalog,
                    setup_command_catalog,
                    setup_persistence,
                    setup_services,
                );

                sync_registered_commands(ctx, &discord_config, &app_state).await?;
                spawn_command_sync_loop(
                    ctx.clone(),
                    discord_config.clone(),
                    command_sync_config.sync_interval_seconds,
                    app_state.clone(),
                );
                if optional_modules.giveaway_enabled {
                    spawn_giveaway_poll_loop(ctx.clone(), app_state.clone());
                }

                build_bot_runtime_report(
                    &discord_config,
                    &command_sync_config,
                    &optional_modules,
                    &app_state,
                    &ready.user.name,
                )
                .await?
                .log();

                let _ = framework;
                Ok(app_state)
            })
        })
        .build();

    let mut client_builder = serenity::ClientBuilder::new(config.discord.token, intents);
    client_builder = client_builder.register_songbird();

    let mut client = client_builder.framework(framework).await?;

    client.start().await?;
    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stdout)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_bot=info,dynamo_core=info,poise=info".into()),
        )
        .try_init();
}

fn build_bot_preconnect_report(
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
    let sync_target = if config.discord.register_globally {
        "global application commands".to_string()
    } else {
        format!(
            "guild {}",
            config
                .discord
                .dev_guild_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        )
    };

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
        .detail(
            "optional_module_flags",
            format!("giveaway={}", config.optional_modules.giveaway_enabled),
        )
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
        .detail("module_ids", catalog_summary.module_ids.join(", "))
        .detail(
            "leaf_command_count",
            catalog_summary.discovered_leaf_command_count.to_string(),
        )
        .detail(
            "per_module_command_counts",
            format_kv_list(&catalog_summary.per_module_command_counts),
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
                repositories.join(", ")
            },
        )
        .detail(
            "services_wired",
            if services_wired.is_empty() {
                "none".to_string()
            } else {
                services_wired.join(", ")
            },
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
                scope_summary.active_module_ids.join(", ")
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

async fn build_bot_runtime_report(
    discord_config: &DiscordConfig,
    command_sync_config: &CommandSyncConfig,
    optional_modules: &OptionalModulesConfig,
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
    report.add_phase(
        StartupPhase::new(
            "runtime",
            if optional_modules.giveaway_enabled {
                StartupStatus::Ok
            } else {
                StartupStatus::Warn
            },
            format!(
                "user={ready_user} active={} giveaway={}",
                scope_summary.active_command_count,
                if optional_modules.giveaway_enabled {
                    "on"
                } else {
                    "off"
                }
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
            if optional_modules.giveaway_enabled {
                format!("enabled every {}s", GIVEAWAY_POLL_INTERVAL_SECONDS)
            } else {
                "disabled (optional module not enabled)".to_string()
            },
        )
        .detail(
            "sync_target",
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
            },
        )
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
    labels
}

fn collect_service_labels(services: &ServiceRegistry) -> Vec<String> {
    let mut labels = Vec::new();
    if services.stock_quotes.is_some() {
        labels.push("stock_quotes".to_string());
    }
    if services.music.is_some() {
        labels.push("music".to_string());
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

fn event_handler<'a>(
    ctx: &'a serenity::Context,
    event: &'a serenity::FullEvent,
    _framework: poise::FrameworkContext<'a, AppState, Error>,
    data: &'a AppState,
) -> poise::BoxFuture<'a, Result<(), Error>> {
    Box::pin(async move { dynamo_app::handle_framework_event(ctx, event, data).await })
}

fn framework_on_error(error: FrameworkError<'_, AppState, Error>) -> poise::BoxFuture<'_, ()> {
    Box::pin(async move {
        match error {
            FrameworkError::Command { ctx, error, .. } => {
                error!(
                    command = ctx.command().qualified_name,
                    ?error,
                    "command execution failed"
                );

                let user_message = format!("Command failed: {error}");
                if let Err(send_error) = ctx
                    .send(CreateReply::default().content(user_message).ephemeral(true))
                    .await
                {
                    if send_error.to_string().contains("Unknown interaction") {
                        warn!(
                            command = ctx.command().qualified_name,
                            ?send_error,
                            "failed to deliver command error because the interaction expired"
                        );
                    } else {
                        error!(?send_error, "failed to send command failure");
                    }
                }
            }
            FrameworkError::CommandCheckFailed {
                ctx,
                error: Some(error),
                ..
            } => {
                if let Err(send_error) = ctx
                    .send(
                        CreateReply::default()
                            .content(error.to_string())
                            .ephemeral(true),
                    )
                    .await
                {
                    error!(?send_error, "failed to send command check failure");
                }
            }
            other => {
                if let Err(error) = poise::builtins::on_error(other).await {
                    error!(?error, "framework error handler failed");
                }
            }
        }
    })
}

fn command_check(
    ctx: poise::Context<'_, AppState, Error>,
) -> poise::BoxFuture<'_, Result<bool, Error>> {
    Box::pin(async move {
        let access = dynamo_core::command_access_for_context(ctx).await?;
        if access.allowed() {
            Ok(true)
        } else {
            Err(anyhow::anyhow!(
                access
                    .denial_reason
                    .unwrap_or_else(|| "This command is disabled.".to_string())
            ))
        }
    })
}

#[derive(Debug, Default)]
struct CommandSyncFingerprints {
    global: Option<String>,
    guilds: HashMap<u64, String>,
}

fn command_sync_fingerprints() -> &'static Mutex<CommandSyncFingerprints> {
    static STATE: OnceLock<Mutex<CommandSyncFingerprints>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(CommandSyncFingerprints::default()))
}

fn command_sync_started() -> &'static OnceLock<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    &STARTED
}

fn spawn_command_sync_loop(
    ctx: serenity::Context,
    discord_config: DiscordConfig,
    sync_interval_seconds: u64,
    data: AppState,
) {
    if command_sync_started().set(()).is_err() {
        return;
    }

    tokio::spawn(async move {
        let interval = Duration::from_secs(sync_interval_seconds.max(5));
        loop {
            tokio::time::sleep(interval).await;
            if let Err(error) = sync_registered_commands(&ctx, &discord_config, &data).await {
                warn!(?error, "failed to sync application commands");
            }
        }
    });
}

async fn sync_registered_commands(
    ctx: &serenity::Context,
    discord_config: &DiscordConfig,
    data: &AppState,
) -> Result<(), Error> {
    let deployment = data.persistence.deployment_settings_or_default().await?;
    let all_cached_guilds = ctx.cache.guilds().into_iter().collect::<Vec<_>>();

    if discord_config.register_globally {
        let global_commands = dynamo_app::create_application_commands_for_scope(&deployment, None);
        let global_command_count = global_commands.len();
        let global_fingerprint = format!("{global_commands:#?}");

        {
            let mut fingerprints = command_sync_fingerprints().lock().await;
            if fingerprints.global.as_ref() != Some(&global_fingerprint) {
                serenity::Command::set_global_commands(&ctx.http, global_commands).await?;
                fingerprints.global = Some(global_fingerprint);
                info!(
                    command_count = global_command_count,
                    "Synchronized global application commands"
                );
            }
        }

        for guild_id in all_cached_guilds {
            let should_clear = {
                let fingerprints = command_sync_fingerprints().lock().await;
                fingerprints.guilds.get(&guild_id.get()) != Some(&"<cleared>".to_string())
            };

            if should_clear {
                guild_id.set_commands(&ctx.http, vec![]).await?;
                let mut fingerprints = command_sync_fingerprints().lock().await;
                fingerprints
                    .guilds
                    .insert(guild_id.get(), "<cleared>".to_string());
                info!(
                    guild_id = guild_id.get(),
                    command_count = 0,
                    "Cleared guild-specific commands"
                );
            }
        }
    } else {
        {
            let mut fingerprints = command_sync_fingerprints().lock().await;
            if fingerprints.global.as_deref() != Some("<cleared>") {
                serenity::Command::set_global_commands(&ctx.http, vec![]).await?;
                fingerprints.global = Some("<cleared>".to_string());
                info!("Cleared global application commands");
            }
        }

        for guild_id in guild_ids_for_sync(ctx, discord_config) {
            let guild_settings = data
                .persistence
                .guild_settings_or_default(guild_id.get())
                .await?;
            let guild_commands = dynamo_app::create_application_commands_for_scope(
                &deployment,
                Some(&guild_settings),
            );
            let guild_command_count = guild_commands.len();
            let guild_fingerprint = format!("{guild_commands:#?}");

            let should_sync = {
                let fingerprints = command_sync_fingerprints().lock().await;
                fingerprints.guilds.get(&guild_id.get()) != Some(&guild_fingerprint)
            };

            if should_sync {
                guild_id.set_commands(&ctx.http, guild_commands).await?;
                let mut fingerprints = command_sync_fingerprints().lock().await;
                fingerprints
                    .guilds
                    .insert(guild_id.get(), guild_fingerprint);
                info!(
                    guild_id = guild_id.get(),
                    command_count = guild_command_count,
                    "Synchronized guild application commands"
                );
            }
        }
    }

    Ok(())
}

fn guild_ids_for_sync(
    ctx: &serenity::Context,
    discord_config: &DiscordConfig,
) -> Vec<serenity::GuildId> {
    if !discord_config.register_globally {
        return discord_config
            .dev_guild_id
            .map(serenity::GuildId::new)
            .into_iter()
            .collect();
    }

    ctx.cache.guilds().into_iter().collect()
}

fn giveaway_poll_started() -> &'static OnceLock<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    &STARTED
}

fn spawn_giveaway_poll_loop(ctx: serenity::Context, data: AppState) {
    if giveaway_poll_started().set(()).is_err() {
        return;
    }

    tokio::spawn(async move {
        let interval = Duration::from_secs(GIVEAWAY_POLL_INTERVAL_SECONDS);
        loop {
            tokio::time::sleep(interval).await;
            if let Err(error) = dynamo_module_giveaway::poll_due_giveaways(&ctx, &data).await {
                warn!(?error, "failed to poll due giveaways");
            }
        }
    });
}
