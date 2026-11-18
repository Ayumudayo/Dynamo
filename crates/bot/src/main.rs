mod background;
mod command_sync;
mod framework;
mod reports;
mod warning_throttle;

use dynamo_config::AppConfig;
use dynamo_module_kit::GatewayIntents;
use dynamo_observability::init_tracing;
use dynamo_registry::aggregate_intents;
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude as serenity;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::{
    background::{
        spawn_announcement_delivery_loop, spawn_bot_heartbeat_loop,
        spawn_exchange_rate_refresh_loop, spawn_giveaway_poll_loop,
    },
    command_sync::{spawn_command_sync_loop, sync_registered_commands},
    framework::{command_check, event_handler, framework_on_error},
    reports::{build_bot_preconnect_report, build_bot_runtime_report},
};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = AppConfig::from_env()?;
    let registry = dynamo_app::module_registry();
    let persistence = dynamo_app::persistence_from_env().await?;
    let services = dynamo_app::services_from_persistence(&persistence)?;
    let manifests = registry.manifests();
    let commands = registry.commands();
    let setup_catalog = registry.catalog().clone();
    let setup_command_catalog = registry.command_catalog().clone();
    let discord_config = config.discord.clone();
    let command_sync_config = config.commands.clone();
    let setup_persistence = persistence.clone();
    let setup_services = services.clone();
    let startup_deployment = persistence.deployment_settings_or_default().await?;
    let intents = aggregate_intents(manifests.iter().copied().filter(|manifest| {
        startup_deployment
            .modules
            .get(manifest.id)
            .map(|module| module.installed)
            .unwrap_or(true)
    })) | GatewayIntents::GUILDS;
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

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let setup_shutdown_rx = shutdown_rx.clone();

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
            let shutdown_rx = setup_shutdown_rx.clone();

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
                    shutdown_rx.clone(),
                );
                spawn_exchange_rate_refresh_loop(app_state.clone(), shutdown_rx.clone());
                spawn_giveaway_poll_loop(ctx.clone(), app_state.clone(), shutdown_rx.clone());
                spawn_bot_heartbeat_loop(
                    ctx.clone(),
                    app_state.clone(),
                    ready.user.name.clone(),
                    discord_config.register_globally,
                    shutdown_rx.clone(),
                );
                spawn_announcement_delivery_loop(
                    ctx.clone(),
                    app_state.clone(),
                    shutdown_rx.clone(),
                );
                dynamo_module_gameinfo::spawn_maintenance_poll_loop();

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

    let shard_manager = client.shard_manager.clone();
    let shutdown_persistence = persistence.clone();
    let ctrl_c_shutdown_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("shutdown signal received; stopping background tasks and shards");
            let _ = ctrl_c_shutdown_tx.send(true);
            if let Err(error) = dynamo_app::flush_all_voice_sessions(&shutdown_persistence).await {
                warn!(?error, "failed to flush active voice sessions on shutdown");
            }
            shard_manager.shutdown_all().await;
        }
    });

    client.start().await?;
    let _ = shutdown_tx.send(true);
    Ok(())
}
