use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    retry_after_delay(headers).unwrap_or_else(|| group.local_retry_delay(attempt))
}

fn retry_after_delay(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if value.is_empty() {
        return None;
    }

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = DateTime::parse_from_rfc2822(value).ok()?;
    let delta = retry_at.with_timezone(&Utc).signed_duration_since(Utc::now());
    if delta <= chrono::TimeDelta::zero() {
        return Some(Duration::ZERO);
    }

    delta.to_std().ok()
}

#[cfg(test)]
mod tests {
    use super::{TossRateLimitGroup, retry_delay_for_too_many_requests};
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    use std::time::Duration;

    #[test]
    fn rate_limit_retry_delay_prefers_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));

        let delay =
            retry_delay_for_too_many_requests(TossRateLimitGroup::MarketData, &headers, 3);

        assert_eq!(delay, Duration::from_secs(7));
    }

    #[test]
    fn rate_limit_retry_delay_falls_back_to_group_backoff() {
        let delay =
            retry_delay_for_too_many_requests(TossRateLimitGroup::MarketData, &HeaderMap::new(), 2);

        assert_eq!(delay, Duration::from_millis(500));
    }
}
