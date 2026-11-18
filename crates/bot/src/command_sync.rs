use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    num::NonZeroU64,
    sync::OnceLock,
    time::Duration,
};

use chrono::{DateTime, Utc};
use dynamo_config::DiscordConfig;
use dynamo_ops::{
    COMMAND_SYNC_PROVIDER_ID, CommandSyncResult, CommandSyncScopeState, CommandSyncStateStore,
};
use dynamo_persistence_api::Persistence;
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude as serenity;
use tokio::sync::{Mutex, watch};
use tracing::{info, warn};

use crate::warning_throttle::WarningThrottle;

const CLEARED_COMMAND_FINGERPRINT: &str = "<cleared>";

#[derive(Debug, Default)]
struct CommandSyncFingerprints {
    seeded: bool,
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

async fn seed_fingerprints_from_store(sync_state: &CommandSyncStateStore) {
    let mut fingerprints = command_sync_fingerprints().lock().await;
    if fingerprints.seeded {
        return;
    }
    fingerprints.seeded = true;
    if sync_state.global.last_result == Some(CommandSyncResult::Success)
        && let Some(fp) = &sync_state.global.last_synced_fingerprint
    {
        fingerprints.global.get_or_insert_with(|| fp.clone());
    }
    for (&guild_id, state) in &sync_state.guilds {
        if state.last_result == Some(CommandSyncResult::Success)
            && let Some(fp) = &state.last_synced_fingerprint
        {
            fingerprints
                .guilds
                .entry(guild_id)
                .or_insert_with(|| fp.clone());
        }
    }
}

pub(crate) fn spawn_command_sync_loop(
    ctx: serenity::Context,
    discord_config: DiscordConfig,
    sync_interval_seconds: u64,
    data: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    if command_sync_started().set(()).is_err() {
        return;
    }

    tokio::spawn(async move {
        let interval = Duration::from_secs(sync_interval_seconds.max(5));
        let mut warning_throttle = WarningThrottle::default();
        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                    continue;
                }
            }
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

#[derive(Default)]
struct SyncUpdates {
    global: Option<(Option<DateTime<Utc>>, CommandSyncScopeState)>,
    guilds: BTreeMap<u64, (Option<DateTime<Utc>>, CommandSyncScopeState)>,
}

impl SyncUpdates {
    fn is_empty(&self) -> bool {
        self.global.is_none() && self.guilds.is_empty()
    }
}

fn merge_scope_state(
    target: &mut CommandSyncScopeState,
    handled_request_at: Option<DateTime<Utc>>,
    updated: CommandSyncScopeState,
) {
    target.last_synced_at = updated.last_synced_at.or(target.last_synced_at);
    target.last_synced_fingerprint = updated
        .last_synced_fingerprint
        .or(target.last_synced_fingerprint.take());
    target.last_result = updated.last_result;
    target.last_error = updated.last_error;
    target.last_submitted_top_level_commands = updated
        .last_submitted_top_level_commands
        .or(target.last_submitted_top_level_commands);
    if let Some(handled_at) = handled_request_at.or(updated.last_handled_request_at) {
        let next_handled = match target.last_handled_request_at {
            Some(prev) if prev > handled_at => prev,
            _ => handled_at,
        };
        target.last_handled_request_at = Some(next_handled);
    }
}

pub(crate) async fn sync_registered_commands(
    ctx: &serenity::Context,
    discord_config: &DiscordConfig,
    data: &AppState,
) -> Result<(), Error> {
    let deployment = data.deployment_settings_cached().await?;
    let mut sync_state = load_command_sync_state(&data.persistence).await?;
    seed_fingerprints_from_store(&sync_state).await;
    let mut updates = SyncUpdates::default();
    let all_cached_guilds = ctx.cache.guilds().into_iter().collect::<Vec<_>>();

    if discord_config.register_globally {
        let global_commands = dynamo_app::create_application_commands_for_scope(&deployment, None);
        let global_command_count = global_commands.len();
        let global_fingerprint = dynamo_app::application_command_fingerprint(&global_commands);
        let handled_request_at = sync_state.global.requested_at;
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
                updates.global = Some((handled_request_at, sync_state.global.clone()));
                save_command_sync_updates(&data.persistence, &updates).await?;
                return Err(error.into());
            }
            let mut fingerprints = command_sync_fingerprints().lock().await;
            fingerprints.global = Some(global_fingerprint.clone());
            drop(fingerprints);
            sync_state
                .global
                .mark_success(Utc::now(), global_fingerprint, global_command_count);
            updates.global = Some((handled_request_at, sync_state.global.clone()));
            info!(
                command_count = global_command_count,
                "Synchronized global application commands"
            );
        }

        let mut guilds_to_check = BTreeSet::new();
        for guild_id in &all_cached_guilds {
            guilds_to_check.insert(guild_id.get());
        }
        for guild_id in sync_state.pending_guild_ids() {
            if guild_id > 0 {
                guilds_to_check.insert(guild_id);
            }
        }

        for guild_id_raw in guilds_to_check {
            let Some(guild_id) = NonZeroU64::new(guild_id_raw).map(serenity::GuildId::from) else {
                continue;
            };
            let guild_handled_at = sync_state
                .guild(guild_id_raw)
                .and_then(|state| state.requested_at);
            let pending_request = sync_state
                .guild(guild_id_raw)
                .map(|state| state.has_pending_request())
                .unwrap_or(false);
            let should_clear = {
                let fingerprints = command_sync_fingerprints().lock().await;
                command_scope_needs_clear(
                    fingerprints.guilds.get(&guild_id_raw).map(String::as_str),
                )
            };

            if should_clear || pending_request {
                if let Err(error) = guild_id.set_commands(&ctx.http, vec![]).await {
                    warn!(
                        guild_id = guild_id_raw,
                        ?error,
                        "failed to clear guild-specific commands"
                    );
                    sync_state
                        .guild_mut(guild_id_raw)
                        .mark_failure(Utc::now(), error.to_string());
                    updates.guilds.insert(
                        guild_id_raw,
                        (guild_handled_at, sync_state.guild_mut(guild_id_raw).clone()),
                    );
                    continue;
                }
                let mut fingerprints = command_sync_fingerprints().lock().await;
                fingerprints
                    .guilds
                    .insert(guild_id_raw, CLEARED_COMMAND_FINGERPRINT.to_string());
                drop(fingerprints);
                sync_state.guild_mut(guild_id_raw).mark_success(
                    Utc::now(),
                    CLEARED_COMMAND_FINGERPRINT.to_string(),
                    0,
                );
                updates.guilds.insert(
                    guild_id_raw,
                    (guild_handled_at, sync_state.guild_mut(guild_id_raw).clone()),
                );
                info!(
                    guild_id = guild_id_raw,
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
            let guild_handled_at = sync_state
                .guild(guild_id.get())
                .and_then(|state| state.requested_at);
            if deployment.blocked_guild_ids.contains(&guild_id.get()) {
                let should_clear = {
                    let fingerprints = command_sync_fingerprints().lock().await;
                    command_scope_needs_clear(
                        fingerprints.guilds.get(&guild_id.get()).map(String::as_str),
                    )
                };
                if should_clear {
                    let _ = guild_id.set_commands(&ctx.http, vec![]).await;
                    let mut fingerprints = command_sync_fingerprints().lock().await;
                    fingerprints
                        .guilds
                        .insert(guild_id.get(), CLEARED_COMMAND_FINGERPRINT.to_string());
                    sync_state.guild_mut(guild_id.get()).mark_success(
                        Utc::now(),
                        CLEARED_COMMAND_FINGERPRINT.to_string(),
                        0,
                    );
                    updates.guilds.insert(
                        guild_id.get(),
                        (
                            guild_handled_at,
                            sync_state.guild_mut(guild_id.get()).clone(),
                        ),
                    );
                }
                continue;
            }

            let guild_settings = match data.guild_settings_cached(guild_id.get()).await {
                Ok(settings) => settings,
                Err(error) => {
                    warn!(
                        guild_id = guild_id.get(),
                        ?error,
                        "failed to load guild settings for command sync"
                    );
                    sync_state
                        .guild_mut(guild_id.get())
                        .mark_failure(Utc::now(), error.to_string());
                    updates.guilds.insert(
                        guild_id.get(),
                        (
                            guild_handled_at,
                            sync_state.guild_mut(guild_id.get()).clone(),
                        ),
                    );
                    continue;
                }
            };
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
                    warn!(
                        guild_id = guild_id.get(),
                        ?error,
                        "failed to sync guild application commands"
                    );
                    sync_state
                        .guild_mut(guild_id.get())
                        .mark_failure(Utc::now(), error.to_string());
                    updates.guilds.insert(
                        guild_id.get(),
                        (
                            guild_handled_at,
                            sync_state.guild_mut(guild_id.get()).clone(),
                        ),
                    );
                    continue;
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
                updates.guilds.insert(
                    guild_id.get(),
                    (
                        guild_handled_at,
                        sync_state.guild_mut(guild_id.get()).clone(),
                    ),
                );
                info!(
                    guild_id = guild_id.get(),
                    command_count = guild_command_count,
                    "Synchronized guild application commands"
                );
            }
        }
    }

    if !updates.is_empty() {
        save_command_sync_updates(&data.persistence, &updates).await?;
    }

    Ok(())
}

fn guild_ids_for_sync(
    ctx: &serenity::Context,
    discord_config: &DiscordConfig,
    sync_state: &CommandSyncStateStore,
) -> Vec<serenity::GuildId> {
    let mut guild_ids = BTreeSet::new();
    for guild_id in ctx.cache.guilds() {
        if guild_id.get() > 0 {
            guild_ids.insert(guild_id.get());
        }
    }
    if !discord_config.register_globally
        && let Some(dev_guild_id) = discord_config.dev_guild_id
        && dev_guild_id > 0
    {
        guild_ids.insert(dev_guild_id);
    }
    for guild_id in sync_state.pending_guild_ids() {
        if guild_id > 0 {
            guild_ids.insert(guild_id);
        }
    }
    guild_ids
        .into_iter()
        .filter_map(NonZeroU64::new)
        .map(serenity::GuildId::from)
        .collect()
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

async fn save_command_sync_updates(
    persistence: &Persistence,
    updates: &SyncUpdates,
) -> Result<(), Error> {
    let mut latest = load_command_sync_state(persistence).await?;
    if let Some((handled_request_at, global_update)) = &updates.global {
        merge_scope_state(
            &mut latest.global,
            *handled_request_at,
            global_update.clone(),
        );
    }
    for (&guild_id, (handled_request_at, guild_update)) in &updates.guilds {
        merge_scope_state(
            latest.guild_mut(guild_id),
            *handled_request_at,
            guild_update.clone(),
        );
    }
    persistence
        .save_provider_state(COMMAND_SYNC_PROVIDER_ID, serde_json::to_value(&latest)?)
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
