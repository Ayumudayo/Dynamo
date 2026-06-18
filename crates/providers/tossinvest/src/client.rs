use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::{
    Client, Method, RequestBuilder, StatusCode, Url,
    header::{ACCEPT, AUTHORIZATION, HeaderValue},
};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;

use crate::{
    TossInvestConfig,
    models::{OAuth2ErrorResponse, OAuth2TokenResponse, TossErrorEnvelope, TossInvestApiError},
    rate_limit::{TossRateLimitGroup, TossRateLimiter},
};

const OAUTH_TOKEN_PATH: &str = "/oauth2/token";
const TOKEN_REFRESH_SKEW_SECONDS: i64 = 60;

#[derive(Debug, Clone)]
pub struct TossInvestResponse {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl TossInvestResponse {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn headers(&self) -> &reqwest::header::HeaderMap {
        &self.headers
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn text(&self) -> Result<String> {
        String::from_utf8(self.body.clone()).context("Toss response body was not valid UTF-8")
    }

    pub fn json<T>(&self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        serde_json::from_slice(&self.body).context("failed to deserialize Toss response body")
    }

    async fn from_reqwest(response: reqwest::Response) -> Result<Self> {
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .context("failed to read Toss API response body")?
            .to_vec();
        Ok(Self {
            status,
            headers,
            body,
        })
    }

    #[cfg(test)]
    fn test_json(status: StatusCode, body: &str) -> Self {
        Self {
            status,
            headers: reqwest::header::HeaderMap::new(),
            body: body.as_bytes().to_vec(),
        }
    }
}

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
            http_client: Client::new(),
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

    pub async fn send_authenticated(
        &self,
        group: TossRateLimitGroup,
        method: Method,
        path: &str,
    ) -> Result<TossInvestResponse> {
        self.send_authenticated_with(group, method, path, |request| async move {
            let response = request
                .send()
                .await
                .context("failed to send Toss API request")?;
            TossInvestResponse::from_reqwest(response).await
        })
        .await
    }

    async fn authenticated_request(
        &self,
        group: TossRateLimitGroup,
        method: Method,
        path: &str,
    ) -> Result<RequestBuilder> {
        reject_authenticated_group(group)?;
        let url = self.api_url(path)?;
        let token = self.ensure_access_token().await?;
        self.shared_state.rate_limiter.acquire(group).await;
        let authorization = token.authorization_header_value()?;

        Ok(self
            .http_client
            .request(method, url)
            .header(ACCEPT, "application/json")
            .header(AUTHORIZATION, authorization))
    }

    async fn send_authenticated_with<E, Fut>(
        &self,
        group: TossRateLimitGroup,
        method: Method,
        path: &str,
        mut execute: E,
    ) -> Result<TossInvestResponse>
    where
        E: FnMut(RequestBuilder) -> Fut,
        Fut: std::future::Future<Output = Result<TossInvestResponse>>,
    {
        for attempt in 0..=1 {
            let request = self
                .authenticated_request(group, method.clone(), path)
                .await?;
            let response = execute(request).await?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                self.shared_state
                    .rate_limiter
                    .observe_too_many_requests(group, response.headers(), attempt)
                    .await;
                return Ok(response);
            }

            if self
                .should_retry_unauthorized_response(&response, attempt == 0)
                .await?
            {
                continue;
            }

            return Ok(response);
        }

        Err(anyhow!("Toss authenticated send exhausted retry attempts"))
    }

    async fn ensure_access_token(&self) -> Result<CachedAccessToken> {
        {
            let cached = self.shared_state.access_token.lock().await;
            if let Some(token) = cached.as_ref()
                && !token.needs_refresh(Utc::now())
            {
                return Ok(token.clone());
            }
        }

        self.refresh_access_token().await
    }

    async fn clear_cached_token(&self) {
        let mut cached = self.shared_state.access_token.lock().await;
        *cached = None;
    }

    async fn should_retry_unauthorized_response(
        &self,
        response: &TossInvestResponse,
        retry_allowed: bool,
    ) -> Result<bool> {
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(false);
        }

        if !retry_allowed {
            return Ok(false);
        }

        if retryable_token_error_code(response)? {
            self.clear_cached_token().await;
            return Ok(true);
        }

        Ok(false)
    }

    async fn refresh_access_token(&self) -> Result<CachedAccessToken> {
        let mut cached = self.shared_state.access_token.lock().await;
        let now = Utc::now();
        if let Some(token) = cached.as_ref()
            && !token.needs_refresh(now)
        {
            return Ok(token.clone());
        }

        self.shared_state
            .rate_limiter
            .acquire(TossRateLimitGroup::Auth)
            .await;
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
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("Toss API path must not be empty"));
        }
        if trimmed.starts_with("//") {
            return Err(anyhow!(
                "Toss API path must be relative to the configured base URL"
            ));
        }
        if Url::parse(trimmed).is_ok() {
            return Err(anyhow!(
                "Toss API path must be relative to the configured base URL"
            ));
        }

        let base_url = format!("{}/", self.config.base_url().trim_end_matches('/'));
        Url::parse(&base_url)
            .and_then(|url| url.join(trimmed.trim_start_matches('/')))
            .map_err(|error| anyhow!("failed to build Toss API URL: {error}"))
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ClientIdentity {
    base_url: String,
    client_id: String,
}

impl ClientIdentity {
    fn from_config(config: &TossInvestConfig) -> Self {
        Self {
            base_url: config.base_url().to_string(),
            client_id: config.client_id().to_string(),
        }
    }
}

struct SharedClientState {
    access_token: Mutex<Option<CachedAccessToken>>,
    rate_limiter: TossRateLimiter,
}

impl Default for SharedClientState {
    fn default() -> Self {
        Self {
            access_token: Mutex::new(None),
            rate_limiter: TossRateLimiter::new(),
        }
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
    if let Ok(error) = serde_json::from_str::<OAuth2ErrorResponse>(body) {
        if let Some(description) = error.error_description.as_deref() {
            return anyhow!(
                "Toss OAuth token request failed with status {status} (error: {}, description: {})",
                error.error,
                description,
            );
        }

        return anyhow!(
            "Toss OAuth token request failed with status {status} (error: {})",
            error.error,
        );
    }

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

fn reject_authenticated_group(group: TossRateLimitGroup) -> Result<()> {
    if group == TossRateLimitGroup::Auth {
        return Err(anyhow!(
            "Toss authenticated endpoint calls must use a market/info rate-limit group, not Auth"
        ));
    }

    Ok(())
}

fn retryable_token_error_code(response: &TossInvestResponse) -> Result<bool> {
    if response.status() != StatusCode::UNAUTHORIZED {
        return Ok(false);
    }

    match serde_json::from_slice::<TossErrorEnvelope>(response.body()) {
        Ok(parsed) => Ok(matches!(
            parsed.error.code.as_str(),
            "invalid-token" | "expired-token"
        )),
        Err(_) => Ok(false),
    }
}

fn shared_state_for(config: &TossInvestConfig) -> Arc<SharedClientState> {
    static SHARED_CLIENT_STATES: OnceLock<StdMutex<BTreeMap<ClientIdentity, Weak<SharedClientState>>>> =
        OnceLock::new();

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
impl TossInvestClient {
    async fn test_set_cached_token(
        &self,
        access_token: &str,
        token_type: &str,
        expires_at: DateTime<Utc>,
    ) {
        let mut cached = self.shared_state.access_token.lock().await;
        *cached = Some(CachedAccessToken::new(access_token, token_type, expires_at));
    }

    async fn test_has_cached_token(&self) -> bool {
        self.shared_state.access_token.lock().await.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use chrono::{TimeDelta, Utc};
    use reqwest::{
        Method, StatusCode,
        header::{AUTHORIZATION, HeaderValue},
    };

    use crate::TossRateLimitGroup;

    use super::{CachedAccessToken, TossInvestClient, TossInvestResponse, build_oauth_error};

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
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "client-id-retryable".to_string(),
                ),
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

        client
            .test_set_cached_token(
                "cached-token",
                "Bearer",
                Utc::now() + TimeDelta::seconds(3600),
            )
            .await;

        let request = client
            .authenticated_request(TossRateLimitGroup::MarketData, Method::GET, "/api/v1/prices")
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
        assert!(
            client
                .rate_limiter()
                .test_has_scheduled_slot(TossRateLimitGroup::MarketData)
                .await
        );
    }

    #[tokio::test]
    async fn oauth_authenticated_request_rejects_absolute_urls() {
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "client-id-expired".to_string(),
                ),
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

        let error = client
            .authenticated_request(
                TossRateLimitGroup::MarketData,
                Method::GET,
                "https://evil.example/api/v1/prices",
            )
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("relative"));
        assert!(error.contains("base URL"));
    }

    #[tokio::test]
    async fn oauth_authenticated_request_rejects_scheme_relative_urls() {
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "client-id-nonretryable".to_string(),
                ),
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

        let error = client
            .authenticated_request(TossRateLimitGroup::MarketData, Method::GET, "//evil.example/api")
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("relative"));
        assert!(error.contains("base URL"));
    }

    #[tokio::test]
    async fn oauth_authenticated_request_rejects_auth_group_without_scheduling_auth_bucket() {
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "client-id-expired".to_string(),
                ),
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

        let error = client
            .authenticated_request(TossRateLimitGroup::Auth, Method::GET, "/api/v1/prices")
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("market/info"));
        assert!(
            !client
                .rate_limiter()
                .test_has_scheduled_slot(TossRateLimitGroup::Auth)
                .await
        );
    }

    #[tokio::test]
    async fn oauth_client_debug_redacts_token_and_secret_values() {
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "client-id-nonretryable".to_string(),
                ),
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

    #[test]
    fn oauth_standard_error_is_sanitized_and_includes_error_description() {
        let error = build_oauth_error(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{
                "error": "invalid_client",
                "error_description": "Client authentication failed."
            }"#,
        )
        .to_string();

        assert!(error.contains("invalid_client"));
        assert!(error.contains("Client authentication failed."));
        assert!(!error.contains("client-secret"));
        assert!(!error.contains("access_token"));
    }

    #[test]
    fn oauth_standard_error_without_description_avoids_printing_none() {
        let error = build_oauth_error(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{
                "error": "invalid_client"
            }"#,
        )
        .to_string();

        assert!(error.contains("invalid_client"));
        assert!(!error.contains("description"));
        assert!(!error.contains("None"));
    }

    #[tokio::test]
    async fn oauth_retryable_unauthorized_response_clears_cached_token() {
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "client-id-retryable".to_string(),
                ),
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

        let should_retry = client
            .should_retry_unauthorized_response(
                &TossInvestResponse::test_json(
                    StatusCode::UNAUTHORIZED,
                    r#"{"error":{"code":"invalid-token","message":"expired","requestId":"req-1"}}"#,
                ),
                true,
            )
            .await
            .unwrap();

        assert!(should_retry);
        assert!(!client.test_has_cached_token().await);
    }

    #[tokio::test]
    async fn oauth_expired_token_response_is_retryable() {
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "client-id-expired".to_string(),
                ),
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

        let should_retry = client
            .should_retry_unauthorized_response(
                &TossInvestResponse::test_json(
                    StatusCode::UNAUTHORIZED,
                    r#"{"error":{"code":"expired-token","message":"expired","requestId":"req-1"}}"#,
                ),
                true,
            )
            .await
            .unwrap();

        assert!(should_retry);
        assert!(!client.test_has_cached_token().await);
    }

    #[tokio::test]
    async fn oauth_non_retryable_unauthorized_response_keeps_cached_token() {
        let client = TossInvestClient::new(
            crate::TossInvestConfig::from_map(&BTreeMap::from([
                (
                    "TOSSINVEST_CLIENT_ID".to_string(),
                    "client-id-nonretryable".to_string(),
                ),
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

        let should_retry = client
            .should_retry_unauthorized_response(
                &TossInvestResponse::test_json(
                    StatusCode::UNAUTHORIZED,
                    r#"{"error":{"code":"permission-denied","message":"denied","requestId":"req-1"}}"#,
                ),
                true,
            )
            .await
            .unwrap();

        assert!(!should_retry);
        assert!(client.test_has_cached_token().await);
    }

    #[test]
    fn oauth_clients_with_same_identity_share_in_process_state() {
        let config = crate::TossInvestConfig::from_map(&BTreeMap::from([
            ("TOSSINVEST_CLIENT_ID".to_string(), "client-id".to_string()),
            (
                "TOSSINVEST_CLIENT_SECRET".to_string(),
                "client-secret".to_string(),
            ),
        ]))
        .unwrap();

        let first = TossInvestClient::new(config.clone());
        let second = TossInvestClient::new(config);

        assert!(Arc::ptr_eq(&first.shared_state, &second.shared_state));
    }
}
