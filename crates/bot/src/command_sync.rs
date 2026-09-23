use std::{collections::HashMap, sync::OnceLock, time::Duration};

use chrono::Utc;
use dynamo_config::DiscordConfig;
use dynamo_ops::{COMMAND_SYNC_PROVIDER_ID, CommandSyncStateStore};
use dynamo_persistence_api::Persistence;
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude as serenity;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::warning_throttle::WarningThrottle;

const CLEARED_COMMAND_FINGERPRINT: &str = "<cleared>";

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

pub(crate) fn spawn_command_sync_loop(
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

pub(crate) async fn sync_registered_commands(
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

#[cfg(test)]
mod tests {
    use super::command_scope_needs_sync;

    #[test]
    fn command_scope_sync_is_skipped_when_cached_fingerprint_matches() {
        assert!(!command_scope_needs_sync(
            Some("application-command-v1:abc"),
            "application-command-v1:abc",
            false
        ));
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
