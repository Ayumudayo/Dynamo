mod auth;
mod response;
mod state;
mod transport;

use std::{fmt, sync::Arc, time::Duration};

use reqwest::Client;

use crate::{TossInvestConfig, rate_limit::TossRateLimiter};

pub use response::{TossInvestRequestError, TossInvestResponse};
use state::{SharedClientState, shared_state_for};

const OAUTH_TOKEN_PATH: &str = "/oauth2/token";
const TOKEN_REFRESH_SKEW_SECONDS: i64 = 60;
const TOSS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOSS_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const TOSS_TOTAL_DEADLINE: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct TossInvestClient {
    http_client: Client,
    config: TossInvestConfig,
    shared_state: Arc<SharedClientState>,
}

impl TossInvestClient {
    /// Reuses one in-process token cache and limiter per `(base_url, client_id)` identity
    /// so later provider tasks can safely construct shared service wrappers around one client.
    pub fn new(config: TossInvestConfig) -> Self {
        Self {
            http_client: Client::builder()
                .connect_timeout(TOSS_CONNECT_TIMEOUT)
                .timeout(TOSS_REQUEST_TIMEOUT)
                .build()
                .expect("fixed Toss HTTP client configuration should be valid"),
            shared_state: shared_state_for(&config),
            config,
        }
    }

    pub fn config(&self) -> &TossInvestConfig {
        &self.config
    }
    pub fn rate_limiter(&self) -> &TossRateLimiter {
        &self.shared_state.rate_limiter
    }
}

impl fmt::Debug for TossInvestClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TossInvestClient")
            .field("config", &self.config)
            .field("access_token", &"<redacted>")
            .field("rate_limiter", &self.shared_state.rate_limiter)
            .finish()
    }
}

#[cfg(test)]
mod auth_tests;
#[cfg(test)]
mod response_tests;
#[cfg(test)]
mod state_tests;
#[cfg(test)]
mod transport_tests;

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};
    use std::collections::BTreeMap;

    use super::TossInvestClient;

    fn test_config() -> crate::TossInvestConfig {
        crate::TossInvestConfig::from_map(&BTreeMap::from([
            (
                "TOSSINVEST_CLIENT_ID".to_string(),
                "client-id-debug-redaction".to_string(),
            ),
            (
                "TOSSINVEST_CLIENT_SECRET".to_string(),
                "client-secret".to_string(),
            ),
            (
                "TOSSINVEST_BASE_URL".to_string(),
                "https://openapi.tossinvest.com".to_string(),
            ),
        ]))
        .unwrap()
    }

    #[tokio::test]
    async fn oauth_client_debug_redacts_token_and_secret_values() {
        let client = TossInvestClient::new(test_config());

        client
            .test_set_cached_token(
                "cached-token",
                "Bearer",
                Utc::now() + TimeDelta::seconds(3600),
            )
            .await;

        let debug = format!("{client:?}");
        assert!(!debug.contains("client-secret"));
        assert!(!debug.contains("cached-token"));
        assert!(debug.contains("<redacted>"));
    }
}
