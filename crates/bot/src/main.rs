use std::{collections::HashMap, sync::OnceLock, time::Duration};

use dynamo_core::{AppConfig, AppState, DiscordConfig, Error, aggregate_intents};
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
    let loaded_modules = setup_catalog
        .entries
        .iter()
        .map(|entry| entry.module.id)
        .collect::<Vec<_>>()
        .join(", ");
    let discord_config = config.discord.clone();
    let command_sync_config = config.commands.clone();
    let optional_modules = config.optional_modules.clone();
    let setup_persistence = persistence.clone();
    let setup_services = services.clone();

    if let Some(database_name) = persistence.database_name.as_deref() {
        info!(database = %database_name, "MongoDB persistence initialized");
    }
    info!(
        command_scope = if config.discord.register_globally {
            "global"
        } else {
            "guild"
        },
        dev_guild_id = ?config.discord.dev_guild_id,
        sync_interval_seconds = config.commands.sync_interval_seconds,
        module_count = setup_catalog.entries.len(),
        command_count = setup_command_catalog.entries.len(),
        modules = %loaded_modules,
        giveaway_enabled = config.optional_modules.giveaway_enabled,
        "Bot runtime configured"
    );

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
                info!(
                    user = %ready.user.name,
                    modules = setup_catalog.entries.len(),
                    "Connected to Discord"
                );

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
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_bot=info,dynamo_core=info,poise=info".into()),
        )
        .try_init();
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
        let global_fingerprint = format!("{global_commands:#?}");

        {
            let mut fingerprints = command_sync_fingerprints().lock().await;
            if fingerprints.global.as_ref() != Some(&global_fingerprint) {
                serenity::Command::set_global_commands(&ctx.http, global_commands).await?;
                fingerprints.global = Some(global_fingerprint);
                info!("Synchronized global application commands");
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
                info!(guild_id = guild_id.get(), "Cleared guild-specific commands");
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
