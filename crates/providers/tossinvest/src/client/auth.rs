use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use reqwest::StatusCode;

use crate::{
    models::{OAuth2ErrorResponse, OAuth2TokenResponse, TossErrorEnvelope, TossInvestApiError},
    rate_limit::TossRateLimitGroup,
};

use super::{
    OAUTH_TOKEN_PATH, TossInvestClient, TossInvestRequestError, TossInvestResponse,
    state::CachedAccessToken,
};

impl TossInvestClient {
    pub(super) async fn ensure_access_token(&self) -> Result<CachedAccessToken> {
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

    pub(super) async fn should_retry_unauthorized_response(
        &self,
        response: &TossInvestResponse,
        retry_allowed: bool,
    ) -> Result<bool> {
        if response.status() != StatusCode::UNAUTHORIZED || !retry_allowed {
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
        #[cfg(test)]
        {
            let mut next_refresh_token = self.shared_state.next_refresh_token.lock().await;
            if let Some(token) = next_refresh_token.take() {
                *cached = Some(token.clone());
                return Ok(token);
            }
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
}

pub(super) fn build_oauth_error(status: StatusCode, body: &str) -> anyhow::Error {
    if let Ok(error) = serde_json::from_str::<OAuth2ErrorResponse>(body) {
        if let Some(description) = error.error_description.as_deref() {
            return anyhow!(
                "Toss OAuth token request failed with status {status} (error: {}, description: {})",
                error.error,
                description
            );
        }
        return anyhow!(
            "Toss OAuth token request failed with status {status} (error: {})",
            error.error
        );
    }
    if let Ok(envelope) = serde_json::from_str::<TossErrorEnvelope>(body) {
        return TossInvestRequestError::from_api_error(
            "OAuth token",
            status,
            TossInvestApiError::from(envelope.error),
        )
        .into();
    }
    anyhow!("Toss OAuth token request failed with status {status}")
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
