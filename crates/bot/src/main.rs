use std::{collections::HashMap, fmt, sync::OnceLock, time::Duration};

use chrono::Utc;
use dynamo_access::command_access_for_context;
use dynamo_config::{AppConfig, CommandSyncConfig, DiscordConfig};
use dynamo_module_kit::{CommandCatalog, GatewayIntents, ModuleCatalog};
use dynamo_observability::{
    StartupPhase, StartupReport, StartupStatus, catalog_startup_summary, format_gateway_intents,
    format_preview_kv_list, format_preview_list, init_tracing, scope_startup_summary,
};
use dynamo_ops::{COMMAND_SYNC_PROVIDER_ID, CommandSyncStateStore};
use dynamo_persistence_api::Persistence;
use dynamo_registry::aggregate_intents;
use dynamo_runtime_api::{AppState, Error};
use dynamo_services_api::ServiceRegistry;
use dynamo_settings::{DeploymentSettings, GuildSettings};
use poise::{CreateReply, FrameworkError, serenity_prelude as serenity};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

const GIVEAWAY_POLL_INTERVAL_SECONDS: u64 = 15;
const CLEARED_COMMAND_FINGERPRINT: &str = "<cleared>";

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
                spawn_exchange_rate_refresh_loop(app_state.clone());
                spawn_giveaway_poll_loop(ctx.clone(), app_state.clone());

                build_bot_runtime_report(
                    &discord_config,
                    &command_sync_config,
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

    let mut client = serenity::ClientBuilder::new(config.discord.token, intents)
        .framework(framework)
        .await?;

    client.start().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
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

async fn build_bot_runtime_report(
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
        let access = command_access_for_context(ctx).await?;
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

fn command_scope_needs_sync(
    cached_fingerprint: Option<&str>,
    current_fingerprint: &str,
    manual_request_pending: bool,
) -> bool {
    manual_request_pending || cached_fingerprint != Some(current_fingerprint)
}

fn command_scope_needs_clear(cached_fingerprint: Option<&str>) -> bool {
    cached_fingerprint != Some(CLEARED_COMMAND_FINGERPRINT)
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
        let mut warning_throttle = WarningThrottle::default();
        loop {
            tokio::time::sleep(interval).await;
            if let Err(error) = sync_registered_commands(&ctx, &discord_config, &data).await {
                if let Some(suppressed_repetitions) = warning_throttle.record_error(&error) {
                    warn!(
                        ?error,
                        suppressed_repetitions, "failed to sync application commands"
                    );
                }
            } else {
                warning_throttle.record_success();
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
    let mut sync_state = load_command_sync_state(&data.persistence).await?;
    let mut sync_state_dirty = false;
    let all_cached_guilds = ctx.cache.guilds().into_iter().collect::<Vec<_>>();

    if discord_config.register_globally {
        let global_commands = dynamo_app::create_application_commands_for_scope(&deployment, None);
        let global_command_count = global_commands.len();
        let global_fingerprint = dynamo_app::application_command_fingerprint(&global_commands);
        let manual_request_pending = sync_state.global.has_pending_request();

        let should_sync = {
            let fingerprints = command_sync_fingerprints().lock().await;
            command_scope_needs_sync(
                fingerprints.global.as_deref(),
                &global_fingerprint,
                manual_request_pending,
            )
        };

        if should_sync {
            if let Err(error) =
                serenity::Command::set_global_commands(&ctx.http, global_commands).await
            {
                sync_state
                    .global
                    .mark_failure(Utc::now(), error.to_string());
                sync_state_dirty = true;
                if sync_state_dirty {
                    save_command_sync_state(&data.persistence, &sync_state).await?;
                }
                return Err(error.into());
            }
            let mut fingerprints = command_sync_fingerprints().lock().await;
            fingerprints.global = Some(global_fingerprint.clone());
            drop(fingerprints);
            sync_state
                .global
                .mark_success(Utc::now(), global_fingerprint, global_command_count);
            sync_state_dirty = true;
            info!(
                command_count = global_command_count,
                "Synchronized global application commands"
            );
        }

        for guild_id in all_cached_guilds {
            let should_clear = {
                let fingerprints = command_sync_fingerprints().lock().await;
                command_scope_needs_clear(
                    fingerprints.guilds.get(&guild_id.get()).map(String::as_str),
                )
            };

            if should_clear {
                guild_id.set_commands(&ctx.http, vec![]).await?;
                let mut fingerprints = command_sync_fingerprints().lock().await;
                fingerprints
                    .guilds
                    .insert(guild_id.get(), CLEARED_COMMAND_FINGERPRINT.to_string());
                info!(
                    guild_id = guild_id.get(),
                    command_count = 0,
                    "Cleared guild-specific commands"
                );
            }
        }
    } else {
        let should_clear_global = {
            let fingerprints = command_sync_fingerprints().lock().await;
            command_scope_needs_clear(fingerprints.global.as_deref())
        };

        if should_clear_global {
            serenity::Command::set_global_commands(&ctx.http, vec![]).await?;
            let mut fingerprints = command_sync_fingerprints().lock().await;
            fingerprints.global = Some(CLEARED_COMMAND_FINGERPRINT.to_string());
            info!("Cleared global application commands");
        }

        for guild_id in guild_ids_for_sync(ctx, discord_config, &sync_state) {
            let guild_settings = data
                .persistence
                .guild_settings_or_default(guild_id.get())
                .await?;
            let guild_commands = dynamo_app::create_application_commands_for_scope(
                &deployment,
                Some(&guild_settings),
            );
            let guild_command_count = guild_commands.len();
            let guild_fingerprint = dynamo_app::application_command_fingerprint(&guild_commands);
            let manual_request_pending = sync_state
                .guild(guild_id.get())
                .map(|state| state.has_pending_request())
                .unwrap_or(false);

            let should_sync = {
                let fingerprints = command_sync_fingerprints().lock().await;
                command_scope_needs_sync(
                    fingerprints.guilds.get(&guild_id.get()).map(String::as_str),
                    &guild_fingerprint,
                    manual_request_pending,
                )
            };

            if should_sync {
                if let Err(error) = guild_id.set_commands(&ctx.http, guild_commands).await {
                    sync_state
                        .guild_mut(guild_id.get())
                        .mark_failure(Utc::now(), error.to_string());
                    sync_state_dirty = true;
                    if sync_state_dirty {
                        save_command_sync_state(&data.persistence, &sync_state).await?;
                    }
                    return Err(error.into());
                }
                let mut fingerprints = command_sync_fingerprints().lock().await;
                fingerprints
                    .guilds
                    .insert(guild_id.get(), guild_fingerprint.clone());
                drop(fingerprints);
                sync_state.guild_mut(guild_id.get()).mark_success(
                    Utc::now(),
                    guild_fingerprint,
                    guild_command_count,
                );
                sync_state_dirty = true;
                info!(
                    guild_id = guild_id.get(),
                    command_count = guild_command_count,
                    "Synchronized guild application commands"
                );
            }
        }
    }

    if sync_state_dirty {
        save_command_sync_state(&data.persistence, &sync_state).await?;
    }

    Ok(())
}

fn guild_ids_for_sync(
    ctx: &serenity::Context,
    discord_config: &DiscordConfig,
    sync_state: &CommandSyncStateStore,
) -> Vec<serenity::GuildId> {
    if !discord_config.register_globally {
        let mut guild_ids = std::collections::BTreeSet::new();
        if let Some(dev_guild_id) = discord_config.dev_guild_id {
            guild_ids.insert(dev_guild_id);
        }
        guild_ids.extend(sync_state.pending_guild_ids());
        return guild_ids.into_iter().map(serenity::GuildId::new).collect();
    }

    ctx.cache.guilds().into_iter().collect()
}

async fn load_command_sync_state(
    persistence: &Persistence,
) -> Result<CommandSyncStateStore, Error> {
    Ok(persistence
        .load_provider_state(COMMAND_SYNC_PROVIDER_ID)
        .await?
        .and_then(|value| serde_json::from_value::<CommandSyncStateStore>(value).ok())
        .unwrap_or_default())
}

async fn save_command_sync_state(
    persistence: &Persistence,
    state: &CommandSyncStateStore,
) -> Result<(), Error> {
    persistence
        .save_provider_state(COMMAND_SYNC_PROVIDER_ID, serde_json::to_value(state)?)
        .await
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
        let mut warning_throttle = WarningThrottle::default();
        loop {
            tokio::time::sleep(interval).await;
            if let Err(error) = dynamo_module_giveaway::poll_due_giveaways(&ctx, &data).await {
                if let Some(suppressed_repetitions) = warning_throttle.record_error(&error) {
                    warn!(
                        ?error,
                        suppressed_repetitions, "failed to poll due giveaways"
                    );
                }
            } else {
                warning_throttle.record_success();
            }
        }
    });
}

fn exchange_rate_refresh_started() -> &'static OnceLock<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    &STARTED
}

fn spawn_exchange_rate_refresh_loop(data: AppState) {
    if exchange_rate_refresh_started().set(()).is_err() {
        return;
    }

    let Some(service) = data.services.exchange_rates.clone() else {
        return;
    };

    tokio::spawn(async move {
        if let Err(error) = service.refresh_cache().await {
            warn!(?error, "failed to preflight exchange-rate data");
        }

        let interval =
            Duration::from_secs(dynamo_provider_tossinvest::exchange_refresh_interval_seconds());
        let mut warning_throttle = WarningThrottle::default();
        loop {
            tokio::time::sleep(interval).await;
            if let Err(error) = service.refresh_cache().await {
                if let Some(suppressed_repetitions) = warning_throttle.record_error(&error) {
                    warn!(
                        ?error,
                        suppressed_repetitions, "failed to refresh exchange-rate data"
                    );
                }
            } else {
                warning_throttle.record_success();
            }
        }
    });
}

const WARNING_THROTTLE_REPEAT_LOG_INTERVAL: u64 = 60;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum WarningThrottleAction {
    Log { suppressed_repetitions: u64 },
    Suppress,
}

#[derive(Debug, Default)]
struct WarningThrottle {
    last_fingerprint: Option<String>,
    suppressed_repetitions: u64,
}

impl WarningThrottle {
    fn record_success(&mut self) {
        self.last_fingerprint = None;
        self.suppressed_repetitions = 0;
    }

    fn record_error(&mut self, error: &(impl fmt::Display + ?Sized)) -> Option<u64> {
        match self.record(&error.to_string()) {
            WarningThrottleAction::Log {
                suppressed_repetitions,
            } => Some(suppressed_repetitions),
            WarningThrottleAction::Suppress => None,
        }
    }

    fn record(&mut self, fingerprint: &str) -> WarningThrottleAction {
        if self.last_fingerprint.as_deref() == Some(fingerprint) {
            self.suppressed_repetitions = self.suppressed_repetitions.saturating_add(1);
            if self
                .suppressed_repetitions
                .is_multiple_of(WARNING_THROTTLE_REPEAT_LOG_INTERVAL)
            {
                return WarningThrottleAction::Log {
                    suppressed_repetitions: self.suppressed_repetitions,
                };
            }
            return WarningThrottleAction::Suppress;
        }

        let suppressed_repetitions = self.suppressed_repetitions;
        self.last_fingerprint = Some(fingerprint.to_string());
        self.suppressed_repetitions = 0;
        WarningThrottleAction::Log {
            suppressed_repetitions,
        }
    }
}

#[cfg(test)]
mod tests {
    use dynamo_services_api::ServiceRegistry;

    use super::{
        WarningThrottle, WarningThrottleAction, collect_service_labels, command_scope_needs_sync,
    };

    #[test]
    fn warning_throttle_logs_first_error_and_suppresses_identical_repeats() {
        let mut throttle = WarningThrottle::default();

        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 0
            }
        );
        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Suppress
        );
    }

    #[test]
    fn warning_throttle_logs_when_error_changes_with_suppressed_count() {
        let mut throttle = WarningThrottle::default();

        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 0
            }
        );
        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Suppress
        );
        assert_eq!(
            throttle.record("audit write failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 1
            }
        );
    }

    #[test]
    fn warning_throttle_logs_same_error_after_success() {
        let mut throttle = WarningThrottle::default();

        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 0
            }
        );
        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Suppress
        );

        throttle.record_success();

        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 0
            }
        );
    }

    #[test]
    fn command_scope_sync_is_skipped_when_cached_fingerprint_matches() {
        assert!(!command_scope_needs_sync(
            Some("application-command-v1:abc"),
            "application-command-v1:abc",
            false
        ));
    }

    #[test]
    fn service_labels_omit_exchange_when_exchange_service_is_disabled() {
        let labels = collect_service_labels(&ServiceRegistry::new(None, None));

        assert!(!labels.contains(&"exchange_rates".to_string()));
    }

    #[test]
    fn command_scope_sync_is_required_when_cached_fingerprint_differs() {
        assert!(command_scope_needs_sync(
            Some("application-command-v1:old"),
            "application-command-v1:new",
            false
        ));
    }

    #[test]
    fn command_scope_sync_is_required_when_manual_request_is_pending() {
        assert!(command_scope_needs_sync(
            Some("application-command-v1:abc"),
            "application-command-v1:abc",
            true
        ));
    }
}
