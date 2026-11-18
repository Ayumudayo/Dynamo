use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::{Arc, OnceLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
use dynamo_ops::{
    ANNOUNCEMENT_QUEUE_PROVIDER_ID, AnnouncementStatus, BOT_HEARTBEAT_PROVIDER_ID,
    BotHeartbeatSnapshot, OwnerAnnouncement, OwnerAnnouncementStore,
};
use dynamo_runtime_api::AppState;
use poise::serenity_prelude as serenity;
use tokio::sync::{RwLock, watch};
use tracing::warn;

use crate::warning_throttle::WarningThrottle;

pub(crate) const GIVEAWAY_POLL_INTERVAL_SECONDS: u64 = 15;
pub(crate) const BOT_HEARTBEAT_INTERVAL_SECONDS: u64 = 20;
pub(crate) const ANNOUNCEMENT_POLL_INTERVAL_SECONDS: u64 = 15;

#[derive(Debug, Clone, Default)]
pub(crate) struct ExchangeRefreshRuntimeStatus {
    pub(crate) active: bool,
    pub(crate) last_refreshed_at: Option<DateTime<Utc>>,
    pub(crate) last_error: Option<String>,
}

fn exchange_refresh_status() -> &'static Arc<RwLock<ExchangeRefreshRuntimeStatus>> {
    static STATUS: OnceLock<Arc<RwLock<ExchangeRefreshRuntimeStatus>>> = OnceLock::new();
    STATUS.get_or_init(|| Arc::new(RwLock::new(ExchangeRefreshRuntimeStatus::default())))
}

async fn shutdown_wait(duration: Duration, shutdown_rx: &mut watch::Receiver<bool>) -> bool {
    if *shutdown_rx.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        changed = shutdown_rx.changed() => changed.is_err() || *shutdown_rx.borrow(),
    }
}

async fn is_module_installed(data: &AppState, module_id: &str) -> bool {
    match data.deployment_settings_cached().await {
        Ok(deployment) => deployment
            .modules
            .get(module_id)
            .map(|module| module.installed && module.enabled)
            .unwrap_or(true),
        Err(error) => {
            warn!(
                module_id,
                ?error,
                "failed to check module installation state"
            );
            true
        }
    }
}

fn giveaway_poll_started() -> &'static OnceLock<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    &STARTED
}

pub(crate) fn spawn_giveaway_poll_loop(
    ctx: serenity::Context,
    data: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    if giveaway_poll_started().set(()).is_err() {
        return;
    }

    tokio::spawn(async move {
        let interval = Duration::from_secs(GIVEAWAY_POLL_INTERVAL_SECONDS);
        let mut warning_throttle = WarningThrottle::default();
        loop {
            if shutdown_wait(interval, &mut shutdown_rx).await {
                break;
            }
            if !is_module_installed(&data, "giveaway").await {
                continue;
            }
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

pub(crate) fn spawn_exchange_rate_refresh_loop(
    data: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    if exchange_rate_refresh_started().set(()).is_err() {
        return;
    }

    let Some(service) = data.services.exchange_rates.clone() else {
        return;
    };

    tokio::spawn(async move {
        if is_module_installed(&data, "currency").await {
            {
                let mut status = exchange_refresh_status().write().await;
                status.active = true;
            }
            match service.refresh_cache().await {
                Ok(_) => {
                    let mut status = exchange_refresh_status().write().await;
                    status.last_refreshed_at = Some(Utc::now());
                    status.last_error = None;
                }
                Err(error) => {
                    warn!(?error, "failed to preflight exchange-rate data");
                    let mut status = exchange_refresh_status().write().await;
                    status.last_error = Some(error.to_string());
                }
            }
        }

        let interval =
            Duration::from_secs(dynamo_provider_tossinvest::exchange_refresh_interval_seconds());
        let mut warning_throttle = WarningThrottle::default();
        loop {
            if shutdown_wait(interval, &mut shutdown_rx).await {
                break;
            }
            if !is_module_installed(&data, "currency").await {
                continue;
            }
            match service.refresh_cache().await {
                Ok(_) => {
                    warning_throttle.record_success();
                    let mut status = exchange_refresh_status().write().await;
                    status.active = true;
                    status.last_refreshed_at = Some(Utc::now());
                    status.last_error = None;
                }
                Err(error) => {
                    if let Some(suppressed_repetitions) = warning_throttle.record_error(&error) {
                        warn!(
                            ?error,
                            suppressed_repetitions, "failed to refresh exchange-rate data"
                        );
                    }
                    let mut status = exchange_refresh_status().write().await;
                    status.last_error = Some(error.to_string());
                }
            }
        }
    });
}

fn heartbeat_started() -> &'static OnceLock<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    &STARTED
}

pub(crate) fn spawn_bot_heartbeat_loop(
    ctx: serenity::Context,
    data: AppState,
    bot_username: String,
    register_globally: bool,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    if heartbeat_started().set(()).is_err() {
        return;
    }

    let started_at = Utc::now();
    tokio::spawn(async move {
        let interval = Duration::from_secs(BOT_HEARTBEAT_INTERVAL_SECONDS);
        let mut warning_throttle = WarningThrottle::default();
        loop {
            let exchange_status = exchange_refresh_status().read().await.clone();
            let google_translate_configured = std::env::var("GOOGLE_TRANSLATE_API_KEY")
                .ok()
                .is_some_and(|value| !value.trim().is_empty());
            let snapshot = BotHeartbeatSnapshot {
                bot_username: Some(bot_username.clone()),
                started_at,
                updated_at: Utc::now(),
                connected_guilds: ctx.cache.guilds().len(),
                loaded_modules: data.module_catalog.entries.len(),
                loaded_commands: data.command_catalog.entries.len(),
                register_globally,
                tossinvest_wired: data.services.stock_quotes.is_some()
                    || data.services.exchange_rates.is_some(),
                exchange_refresh_active: exchange_status.active,
                last_exchange_refresh_at: exchange_status.last_refreshed_at,
                last_exchange_error: exchange_status.last_error,
                google_translate_configured,
            };

            match serde_json::to_value(snapshot) {
                Ok(value) => {
                    if let Err(error) = data
                        .persistence
                        .save_provider_state(BOT_HEARTBEAT_PROVIDER_ID, value)
                        .await
                    {
                        if let Some(suppressed_repetitions) = warning_throttle.record_error(&error)
                        {
                            warn!(
                                ?error,
                                suppressed_repetitions, "failed to save bot heartbeat"
                            );
                        }
                    } else {
                        warning_throttle.record_success();
                    }
                }
                Err(error) => {
                    warn!(?error, "failed to serialize bot heartbeat");
                }
            }

            if shutdown_wait(interval, &mut shutdown_rx).await {
                break;
            }
        }
    });
}

fn announcement_loop_started() -> &'static OnceLock<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    &STARTED
}

pub(crate) fn spawn_announcement_delivery_loop(
    ctx: serenity::Context,
    data: AppState,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    if announcement_loop_started().set(()).is_err() {
        return;
    }

    tokio::spawn(async move {
        let interval = Duration::from_secs(ANNOUNCEMENT_POLL_INTERVAL_SECONDS);
        let mut warning_throttle = WarningThrottle::default();
        loop {
            if shutdown_wait(interval, &mut shutdown_rx).await {
                break;
            }
            if let Err(error) = process_pending_announcements(&ctx, &data).await {
                if let Some(suppressed_repetitions) = warning_throttle.record_error(&error) {
                    warn!(
                        ?error,
                        suppressed_repetitions, "failed to deliver owner announcements"
                    );
                }
            } else {
                warning_throttle.record_success();
            }
        }
    });
}

async fn process_pending_announcements(
    ctx: &serenity::Context,
    data: &AppState,
) -> anyhow::Result<()> {
    let Some(raw) = data
        .persistence
        .load_provider_state(ANNOUNCEMENT_QUEUE_PROVIDER_ID)
        .await?
    else {
        return Ok(());
    };
    let mut store: OwnerAnnouncementStore = serde_json::from_value(raw).unwrap_or_default();
    if !store
        .entries
        .iter()
        .any(|entry| entry.status == AnnouncementStatus::Queued)
    {
        return Ok(());
    }

    let deployment = data.deployment_settings_cached().await?;
    let all_guilds = data.persistence.list_all_guild_settings().await?;
    let targets = all_guilds
        .into_iter()
        .filter_map(|guild| {
            if deployment.blocked_guild_ids.contains(&guild.guild_id) {
                return None;
            }
            guild
                .announcement_channel_id
                .and_then(NonZeroU64::new)
                .map(|channel_id| (guild.guild_id, serenity::ChannelId::from(channel_id)))
        })
        .collect::<Vec<_>>();

    let mut processed_by_id: HashMap<String, OwnerAnnouncement> = HashMap::new();
    for entry in &mut store.entries {
        if entry.status != AnnouncementStatus::Queued {
            continue;
        }
        entry.targeted_guilds = targets.len();
        if targets.is_empty() {
            entry.status = AnnouncementStatus::NoSubscribers;
            entry.processed_at = Some(Utc::now());
            processed_by_id.insert(entry.id.clone(), entry.clone());
            continue;
        }

        let mut delivered = 0usize;
        let mut failed = 0usize;
        let mut last_error = None;
        for (guild_id, channel_id) in &targets {
            let mut embed = serenity::CreateEmbed::new()
                .description(&entry.body)
                .color(entry.tone.embed_color())
                .footer(serenity::CreateEmbedFooter::new("Dynamo Operations Notice"))
                .timestamp(serenity::Timestamp::from(entry.created_at));
            if !entry.title.trim().is_empty() {
                embed = embed.title(&entry.title);
            }
            let message = serenity::CreateMessage::new().embed(embed);
            match channel_id.send_message(&ctx.http, message).await {
                Ok(_) => delivered += 1,
                Err(error) => {
                    failed += 1;
                    last_error = Some(format!("guild {guild_id}: {error}"));
                }
            }
        }
        entry.delivered_guilds = delivered;
        entry.failed_guilds = failed;
        entry.last_error = last_error;
        entry.processed_at = Some(Utc::now());
        entry.status = if failed == 0 {
            AnnouncementStatus::Delivered
        } else {
            AnnouncementStatus::PartialFailure
        };
        processed_by_id.insert(entry.id.clone(), entry.clone());
    }

    if !processed_by_id.is_empty() {
        let mut latest_store = data
            .persistence
            .load_provider_state(ANNOUNCEMENT_QUEUE_PROVIDER_ID)
            .await?
            .and_then(|value| serde_json::from_value::<OwnerAnnouncementStore>(value).ok())
            .unwrap_or_default();
        for latest_entry in &mut latest_store.entries {
            if let Some(updated) = processed_by_id.get(&latest_entry.id) {
                *latest_entry = updated.clone();
            }
        }
        data.persistence
            .save_provider_state(
                ANNOUNCEMENT_QUEUE_PROVIDER_ID,
                serde_json::to_value(latest_store)?,
            )
            .await?;
    }

    Ok(())
}
