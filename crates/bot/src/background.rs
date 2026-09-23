use std::{sync::OnceLock, time::Duration};

use dynamo_runtime_api::AppState;
use poise::serenity_prelude as serenity;
use tracing::warn;

use crate::warning_throttle::WarningThrottle;

pub(crate) const GIVEAWAY_POLL_INTERVAL_SECONDS: u64 = 15;

fn giveaway_poll_started() -> &'static OnceLock<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    &STARTED
}

pub(crate) fn spawn_giveaway_poll_loop(ctx: serenity::Context, data: AppState) {
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

pub(crate) fn spawn_exchange_rate_refresh_loop(data: AppState) {
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
