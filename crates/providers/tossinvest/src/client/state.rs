use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::header::HeaderValue;
use tokio::sync::Mutex;

use crate::{TossInvestConfig, models::OAuth2TokenResponse, rate_limit::TossRateLimiter};

use super::TOKEN_REFRESH_SKEW_SECONDS;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ClientIdentity {
    base_url: String,
    client_id: String,
}
impl ClientIdentity {
    fn from_config(config: &TossInvestConfig) -> Self {
        Self {
            base_url: config.base_url().trim_end_matches('/').to_string(),
            client_id: config.client_id().to_string(),
        }
    }
}

pub(super) struct SharedClientState {
    pub(super) access_token: Mutex<Option<CachedAccessToken>>,
    pub(super) rate_limiter: TossRateLimiter,
    #[cfg(test)]
    pub(super) next_refresh_token: Mutex<Option<CachedAccessToken>>,
}
impl Default for SharedClientState {
    fn default() -> Self {
        Self {
            access_token: Mutex::new(None),
            rate_limiter: TossRateLimiter::new(),
            #[cfg(test)]
            next_refresh_token: Mutex::new(None),
        }
    }
}

#[derive(Clone)]
pub(super) struct CachedAccessToken {
    access_token: String,
    token_type: String,
    expires_at: DateTime<Utc>,
}
impl CachedAccessToken {
    #[cfg(test)]
    pub(super) fn new(access_token: &str, token_type: &str, expires_at: DateTime<Utc>) -> Self {
        Self {
            access_token: access_token.to_string(),
            token_type: token_type.to_string(),
            expires_at,
        }
    }
    pub(super) fn from_oauth_response(
        response: OAuth2TokenResponse,
        fetched_at: DateTime<Utc>,
    ) -> Result<Self> {
        let expires_in_seconds = i64::try_from(response.expires_in)
            .map_err(|_| anyhow!("Toss OAuth token expiry overflowed i64"))?;
        let expires_at = fetched_at
            .checked_add_signed(TimeDelta::seconds(expires_in_seconds))
            .ok_or_else(|| anyhow!("Toss OAuth token expiry exceeded chrono range"))?;
        Ok(Self {
            access_token: response.access_token,
            token_type: response.token_type,
            expires_at,
        })
    }
    pub(super) fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now + TimeDelta::seconds(TOKEN_REFRESH_SKEW_SECONDS)
    }
    pub(super) fn authorization_header_value(&self) -> Result<HeaderValue> {
        HeaderValue::from_str(&format!("{} {}", self.token_type, self.access_token))
            .context("failed to build Toss authorization header")
    }
}

pub(super) fn shared_state_for(config: &TossInvestConfig) -> Arc<SharedClientState> {
    static SHARED_CLIENT_STATES: OnceLock<
        StdMutex<BTreeMap<ClientIdentity, Weak<SharedClientState>>>,
    > = OnceLock::new();
    let identity = ClientIdentity::from_config(config);
    let registry = SHARED_CLIENT_STATES.get_or_init(|| StdMutex::new(BTreeMap::new()));
    let mut registry = registry
        .lock()
        .expect("shared TossInvest client registry should not be poisoned");
    if let Some(existing) = registry.get(&identity).and_then(Weak::upgrade) {
        return existing;
    }
    registry.retain(|_, state| state.upgrade().is_some());
    let state = Arc::new(SharedClientState::default());
    registry.insert(identity, Arc::downgrade(&state));
    state
}

#[cfg(test)]
impl super::TossInvestClient {
    pub(super) async fn test_set_cached_token(
        &self,
        access_token: &str,
        token_type: &str,
        expires_at: DateTime<Utc>,
    ) {
        let mut cached = self.shared_state.access_token.lock().await;
        *cached = Some(CachedAccessToken::new(access_token, token_type, expires_at));
    }

    pub(super) async fn test_has_cached_token(&self) -> bool {
        self.shared_state.access_token.lock().await.is_some()
    }

    pub(super) async fn test_set_next_refresh_token(
        &self,
        access_token: &str,
        token_type: &str,
        expires_at: DateTime<Utc>,
    ) {
        let mut next_refresh_token = self.shared_state.next_refresh_token.lock().await;
        *next_refresh_token = Some(CachedAccessToken::new(access_token, token_type, expires_at));
    }
}
