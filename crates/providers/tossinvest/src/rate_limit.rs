use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderName, RETRY_AFTER};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
// This provider only covers Toss market-data endpoints for now.
pub enum TossRateLimitGroup {
    Auth,
    MarketInfo,
    MarketData,
    MarketDataChart,
    Stock,
}

impl TossRateLimitGroup {
    pub fn local_spacing(self) -> Duration {
        match self {
            Self::Auth => Duration::from_millis(250),
            Self::MarketInfo => Duration::from_millis(400),
            Self::MarketData => Duration::from_millis(125),
            Self::MarketDataChart => Duration::from_millis(250),
            Self::Stock => Duration::from_millis(250),
        }
    }

    pub fn local_retry_delay(self, attempt: u32) -> Duration {
        let multiplier = 1u32.checked_shl(attempt.min(4)).unwrap_or(16);
        let delay = self.local_spacing().saturating_mul(multiplier);
        delay.min(Duration::from_secs(5))
    }
}

#[derive(Clone, Default)]
pub struct TossRateLimiter {
    next_allowed_at: Arc<Mutex<BTreeMap<TossRateLimitGroup, Instant>>>,
}

impl TossRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn acquire(&self, group: TossRateLimitGroup) {
        let delay = {
            let mut next_allowed_at = self.next_allowed_at.lock().await;
            let now = Instant::now();
            let next_slot = next_allowed_at.entry(group).or_insert(now);
            if *next_slot > now {
                let delay = *next_slot - now;
                *next_slot += group.local_spacing();
                delay
            } else {
                *next_slot = now + group.local_spacing();
                Duration::ZERO
            }
        };

        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }

    pub fn retry_delay_for_too_many_requests(
        &self,
        group: TossRateLimitGroup,
        headers: &HeaderMap,
        attempt: u32,
    ) -> Duration {
        retry_delay_for_too_many_requests(group, headers, attempt)
    }

    pub async fn observe_too_many_requests(
        &self,
        group: TossRateLimitGroup,
        headers: &HeaderMap,
        attempt: u32,
    ) -> Duration {
        let jitter_percent = {
            let mut rng = rand::thread_rng();
            rng.gen_range(50..=100)
        };
        self.observe_too_many_requests_at(
            group,
            headers,
            attempt,
            Instant::now(),
            Utc::now(),
            jitter_percent,
        )
        .await
    }

    async fn observe_too_many_requests_at(
        &self,
        group: TossRateLimitGroup,
        headers: &HeaderMap,
        attempt: u32,
        now_instant: Instant,
        now_utc: DateTime<Utc>,
        jitter_percent: u32,
    ) -> Duration {
        let delay = retry_delay_for_too_many_requests_at(
            group,
            headers,
            attempt,
            now_utc,
            jitter_percent,
        );
        let mut next_allowed_at = self.next_allowed_at.lock().await;
        let next_slot = next_allowed_at.entry(group).or_insert(now_instant);
        let base = (*next_slot).max(now_instant);
        *next_slot = base + delay;
        delay
    }
}

#[cfg(test)]
impl TossRateLimiter {
    pub(crate) async fn test_has_scheduled_slot(&self, group: TossRateLimitGroup) -> bool {
        self.next_allowed_at.lock().await.contains_key(&group)
    }

    pub(crate) async fn test_remaining_delay_from(
        &self,
        group: TossRateLimitGroup,
        now: Instant,
    ) -> Option<Duration> {
        self.next_allowed_at
            .lock()
            .await
            .get(&group)
            .copied()
            .map(|next_allowed| next_allowed.saturating_duration_since(now))
    }
}

impl std::fmt::Debug for TossRateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TossRateLimiter").finish_non_exhaustive()
    }
}

pub fn retry_delay_for_too_many_requests(
    group: TossRateLimitGroup,
    headers: &HeaderMap,
    attempt: u32,
) -> Duration {
    retry_delay_for_too_many_requests_at(
        group,
        headers,
        attempt,
        Utc::now(),
        rand::thread_rng().gen_range(50..=100),
    )
}

fn retry_delay_for_too_many_requests_at(
    group: TossRateLimitGroup,
    headers: &HeaderMap,
    attempt: u32,
    now: DateTime<Utc>,
    jitter_percent: u32,
) -> Duration {
    retry_after_delay(headers, now)
        .or_else(|| x_rate_limit_reset_delay(headers))
        .unwrap_or_else(|| backoff_with_jitter(group, attempt, jitter_percent))
}

fn retry_after_delay(headers: &HeaderMap, now: DateTime<Utc>) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if value.is_empty() {
        return None;
    }

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = DateTime::parse_from_rfc2822(value).ok()?;
    positive_delay_from(retry_at.with_timezone(&Utc), now)
}

fn x_rate_limit_reset_delay(headers: &HeaderMap) -> Option<Duration> {
    let value = headers
        .get(HeaderName::from_static("x-ratelimit-reset"))?
        .to_str()
        .ok()?
        .trim();
    if value.is_empty() {
        return None;
    }

    let refill_seconds = value.parse::<u64>().ok()?;
    Some(Duration::from_secs(refill_seconds))
}

fn positive_delay_from(target: DateTime<Utc>, now: DateTime<Utc>) -> Option<Duration> {
    let delta = target.signed_duration_since(now);
    if delta <= chrono::TimeDelta::zero() {
        return Some(Duration::ZERO);
    }

    delta.to_std().ok()
}

fn backoff_with_jitter(
    group: TossRateLimitGroup,
    attempt: u32,
    jitter_percent: u32,
) -> Duration {
    let base_delay = group.local_retry_delay(attempt);
    let clamped_percent = jitter_percent.clamp(50, 100);
    let jittered_millis =
        (base_delay.as_millis() * u128::from(clamped_percent)) / u128::from(100_u32);
    Duration::from_millis(jittered_millis.min(u128::from(u64::MAX)) as u64)
}

#[cfg(test)]
mod tests {
    use super::{TossRateLimitGroup, TossRateLimiter, backoff_with_jitter, retry_delay_for_too_many_requests_at};
    use chrono::{DateTime, Utc};
    use reqwest::header::{HeaderMap, HeaderValue, HeaderName, RETRY_AFTER};
    use std::time::{Duration, Instant};

    #[test]
    fn rate_limit_retry_delay_prefers_retry_after_over_x_rate_limit_reset() {
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        headers.insert(
            HeaderName::from_static("x-ratelimit-reset"),
            HeaderValue::from_static("5"),
        );

        let delay = retry_delay_for_too_many_requests_at(
            TossRateLimitGroup::MarketData,
            &headers,
            3,
            now,
            100,
        );

        assert_eq!(delay, Duration::from_secs(7));
    }

    #[test]
    fn rate_limit_retry_delay_uses_x_rate_limit_reset_when_retry_after_missing() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-ratelimit-reset"),
            HeaderValue::from_static("5"),
        );

        let delay = retry_delay_for_too_many_requests_at(
            TossRateLimitGroup::MarketData,
            &headers,
            2,
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
            100,
        );

        assert_eq!(delay, Duration::from_secs(5));
    }

    #[test]
    fn rate_limit_retry_delay_falls_back_to_bounded_jittered_backoff() {
        let low = backoff_with_jitter(TossRateLimitGroup::MarketData, 2, 50);
        let high = backoff_with_jitter(TossRateLimitGroup::MarketData, 2, 100);

        assert_eq!(low, Duration::from_millis(250));
        assert_eq!(high, Duration::from_millis(500));
        assert_ne!(low, high);
    }

    #[tokio::test]
    async fn rate_limit_observed_cooldown_is_shared_across_clones() {
        let limiter = TossRateLimiter::new();
        let shared = limiter.clone();
        let now_instant = Instant::now();
        let now_utc = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-ratelimit-reset"),
            HeaderValue::from_static("5"),
        );

        let delay = limiter
            .observe_too_many_requests_at(
                TossRateLimitGroup::MarketData,
                &headers,
                0,
                now_instant,
                now_utc,
                100,
            )
            .await;

        assert_eq!(delay, Duration::from_secs(5));
        assert_eq!(
            shared
                .test_remaining_delay_from(TossRateLimitGroup::MarketData, now_instant)
                .await,
            Some(Duration::from_secs(5))
        );
    }
}
