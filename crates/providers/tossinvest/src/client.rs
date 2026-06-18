use std::{fmt, sync::Arc};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::{
    Client, Method, RequestBuilder, Url,
    header::{ACCEPT, AUTHORIZATION, HeaderValue},
};
use tokio::sync::Mutex;

use crate::{
    TossInvestConfig,
    models::{OAuth2TokenResponse, TossErrorEnvelope, TossInvestApiError},
    rate_limit::{TossRateLimitGroup, TossRateLimiter},
};

const OAUTH_TOKEN_PATH: &str = "/oauth2/token";
const TOKEN_REFRESH_SKEW_SECONDS: i64 = 60;

#[derive(Clone)]
pub struct TossInvestClient {
    http_client: Client,
    config: TossInvestConfig,
    access_token: Arc<Mutex<Option<CachedAccessToken>>>,
    rate_limiter: TossRateLimiter,
}

impl TossInvestClient {
    pub fn new(config: TossInvestConfig) -> Self {
        Self {
            http_client: Client::new(),
            config,
            access_token: Arc::new(Mutex::new(None)),
            rate_limiter: TossRateLimiter::new(),
        }
    }

    pub fn config(&self) -> &TossInvestConfig {
        &self.config
    }

    pub fn rate_limiter(&self) -> &TossRateLimiter {
        &self.rate_limiter
    }

    pub async fn authenticated_request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let token = self.ensure_access_token().await?;
        let url = self.api_url(path)?;
        let authorization = token.authorization_header_value()?;

        Ok(self
            .http_client
            .request(method, url)
            .header(ACCEPT, "application/json")
            .header(AUTHORIZATION, authorization))
    }

    async fn ensure_access_token(&self) -> Result<CachedAccessToken> {
        {
            let cached = self.access_token.lock().await;
            if let Some(token) = cached.as_ref()
                && !token.needs_refresh(Utc::now())
            {
                return Ok(token.clone());
            }
        }

        self.refresh_access_token().await
    }

    async fn refresh_access_token(&self) -> Result<CachedAccessToken> {
        let mut cached = self.access_token.lock().await;
        let now = Utc::now();
        if let Some(token) = cached.as_ref()
            && !token.needs_refresh(now)
        {
            return Ok(token.clone());
        }

        self.rate_limiter.acquire(TossRateLimitGroup::Auth).await;
        let token_url = self.api_url(OAUTH_TOKEN_PATH)?;
        let response = self
            .http_client
            .post(token_url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.config.client_id()),
                ("client_secret", self.config.client_secret()),
            ])
            .send()
            .await
            .context("failed to request Toss OAuth token")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read Toss OAuth token response body")?;
        if !status.is_success() {
            return Err(build_oauth_error(status, &body));
        }

        let parsed = serde_json::from_str::<OAuth2TokenResponse>(&body)
            .context("failed to deserialize Toss OAuth token response")?;
        let refreshed = CachedAccessToken::from_oauth_response(parsed, now)?;
        *cached = Some(refreshed.clone());
        Ok(refreshed)
    }

    fn api_url(&self, path: &str) -> Result<Url> {
        let base_url = format!("{}/", self.config.base_url().trim_end_matches('/'));
        Url::parse(&base_url)
            .and_then(|url| url.join(path.trim_start_matches('/')))
            .map_err(|error| anyhow!("failed to build Toss API URL: {error}"))
    }
}

impl fmt::Debug for TossInvestClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TossInvestClient")
            .field("config", &self.config)
            .field("access_token", &"<redacted>")
            .field("rate_limiter", &self.rate_limiter)
            .finish()
    }
}

#[derive(Clone)]
struct CachedAccessToken {
    access_token: String,
    token_type: String,
    expires_at: DateTime<Utc>,
}

impl CachedAccessToken {
    #[cfg(test)]
    fn new(access_token: &str, token_type: &str, expires_at: DateTime<Utc>) -> Self {
        Self {
            access_token: access_token.to_string(),
            token_type: token_type.to_string(),
            expires_at,
        }
    }

    fn from_oauth_response(response: OAuth2TokenResponse, fetched_at: DateTime<Utc>) -> Result<Self> {
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

    fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now + TimeDelta::seconds(TOKEN_REFRESH_SKEW_SECONDS)
    }

    fn authorization_header_value(&self) -> Result<HeaderValue> {
        HeaderValue::from_str(&format!("{} {}", self.token_type, self.access_token))
            .context("failed to build Toss authorization header")
    }
}

fn build_oauth_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    if let Ok(envelope) = serde_json::from_str::<TossErrorEnvelope>(body) {
        let error = TossInvestApiError::from(envelope.error);
        return anyhow!(
            "Toss OAuth token request failed with status {status} (request_id: {}, code: {}, message: {})",
            error.request_id.as_deref().unwrap_or("unknown"),
            error.code.as_deref().unwrap_or("unknown"),
            error.message.as_deref().unwrap_or("unknown"),
        );
    }

    anyhow!("Toss OAuth token request failed with status {status}")
}

#[cfg(test)]
impl TossInvestClient {
    async fn test_set_cached_token(
        &self,
        access_token: &str,
        token_type: &str,
        expires_at: DateTime<Utc>,
    ) {
        let mut cached = self.access_token.lock().await;
        *cached = Some(CachedAccessToken::new(access_token, token_type, expires_at));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeDelta, Utc};
    use reqwest::{
        Method,
        header::{AUTHORIZATION, HeaderValue},
    };

    use super::{CachedAccessToken, TossInvestClient};

    #[test]
    fn oauth_cached_token_refreshes_when_expiring_within_60_seconds() {
        let now = Utc::now();
        let fresh_token =
            CachedAccessToken::new("fresh-token", "Bearer", now + TimeDelta::seconds(61));
        let expiring_token =
            CachedAccessToken::new("expiring-token", "Bearer", now + TimeDelta::seconds(60));

        assert!(!fresh_token.needs_refresh(now));
        assert!(expiring_token.needs_refresh(now));
    }

    #[tokio::test]
    async fn oauth_authenticated_request_uses_base_url_and_cached_token() {
        let config = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                ("TOSSINVEST_CLIENT_ID".to_string(), "client-id".to_string()),
                (
                    "TOSSINVEST_CLIENT_SECRET".to_string(),
                    "client-secret".to_string(),
                ),
                (
                    "TOSSINVEST_BASE_URL".to_string(),
                    "https://sandbox.openapi.tossinvest.example".to_string(),
                ),
            ]))
            .unwrap(),
        );

        config
            .test_set_cached_token(
                "cached-token",
                "Bearer",
                Utc::now() + TimeDelta::seconds(3600),
            )
            .await;

        let request = config
            .authenticated_request(Method::GET, "/api/v1/prices")
            .await
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            request.url().as_str(),
            "https://sandbox.openapi.tossinvest.example/api/v1/prices"
        );
        assert_eq!(
            request.headers().get(AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer cached-token"))
        );
    }

    #[tokio::test]
    async fn oauth_client_debug_redacts_token_and_secret_values() {
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                ("TOSSINVEST_CLIENT_ID".to_string(), "client-id".to_string()),
                (
                    "TOSSINVEST_CLIENT_SECRET".to_string(),
                    "client-secret".to_string(),
                ),
            ]))
            .unwrap(),
        );

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
