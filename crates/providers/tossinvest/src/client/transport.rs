use anyhow::{Context, Result, anyhow};
use reqwest::{
    Method, RequestBuilder, StatusCode, Url,
    header::{ACCEPT, AUTHORIZATION},
};

use crate::rate_limit::TossRateLimitGroup;

use super::{TOSS_TOTAL_DEADLINE, TossInvestClient, TossInvestResponse};

impl TossInvestClient {
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

    pub(super) async fn authenticated_request(
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

    pub(super) async fn send_authenticated_with<E, Fut>(
        &self,
        group: TossRateLimitGroup,
        method: Method,
        path: &str,
        execute: E,
    ) -> Result<TossInvestResponse>
    where
        E: FnMut(RequestBuilder) -> Fut,
        Fut: std::future::Future<Output = Result<TossInvestResponse>>,
    {
        self.send_authenticated_with_deadline(group, method, path, TOSS_TOTAL_DEADLINE, execute)
            .await
    }

    pub(super) async fn send_authenticated_with_deadline<E, Fut>(
        &self,
        group: TossRateLimitGroup,
        method: Method,
        path: &str,
        total_deadline: std::time::Duration,
        execute: E,
    ) -> Result<TossInvestResponse>
    where
        E: FnMut(RequestBuilder) -> Fut,
        Fut: std::future::Future<Output = Result<TossInvestResponse>>,
    {
        tokio::time::timeout(
            total_deadline,
            self.send_authenticated_attempts(group, method, path, execute),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "Toss authenticated operation exceeded its {} ms total deadline",
                total_deadline.as_millis()
            )
        })?
    }

    async fn send_authenticated_attempts<E, Fut>(
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

    pub(super) fn api_url(&self, path: &str) -> Result<Url> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("Toss API path must not be empty"));
        }
        if trimmed.starts_with("//") || Url::parse(trimmed).is_ok() {
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

fn reject_authenticated_group(group: TossRateLimitGroup) -> Result<()> {
    if group == TossRateLimitGroup::Auth {
        return Err(anyhow!(
            "Toss authenticated endpoint calls must use a market/info rate-limit group, not Auth"
        ));
    }
    Ok(())
}
