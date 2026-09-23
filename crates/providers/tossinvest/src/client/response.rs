use std::fmt;

use anyhow::{Context, Result};
#[cfg(test)]
use reqwest::header::HeaderValue;
use reqwest::{StatusCode, header::HeaderMap};
use serde::de::DeserializeOwned;

use crate::models::{TossErrorEnvelope, TossInvestApiError};

#[derive(Debug, Clone)]
pub struct TossInvestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl TossInvestResponse {
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Returns a structured error for a non-successful Toss API response.
    ///
    /// Callers should keep this error intact when propagating it so consumers can
    /// distinguish a provider-wide maintenance response from an endpoint-specific
    /// failure without inspecting localized response text.
    pub fn request_error(&self, endpoint: impl Into<String>) -> TossInvestRequestError {
        let api_error = self
            .json::<TossErrorEnvelope>()
            .ok()
            .map(|envelope| TossInvestApiError::from(envelope.error));
        TossInvestRequestError {
            endpoint: endpoint.into(),
            status: self.status,
            api_error,
        }
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

    pub(super) async fn from_reqwest(response: reqwest::Response) -> Result<Self> {
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
    pub(super) fn test_json(status: StatusCode, body: &str) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[cfg(test)]
    pub(super) fn test_with_headers(
        status: StatusCode,
        body: &str,
        headers: &[(&str, &str)],
    ) -> Self {
        let mut header_map = HeaderMap::new();
        for (name, value) in headers {
            header_map.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        Self {
            status,
            headers: header_map,
            body: body.as_bytes().to_vec(),
        }
    }
}

/// A non-success response from the Toss Invest API.
///
/// `maintenance` is an API error code, rather than a localized message. It is
/// therefore safe for downstream commands and jobs to use for stable recovery
/// behavior and user-facing grouping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TossInvestRequestError {
    endpoint: String,
    status: StatusCode,
    api_error: Option<TossInvestApiError>,
}

impl TossInvestRequestError {
    pub(super) fn from_api_error(
        endpoint: impl Into<String>,
        status: StatusCode,
        api_error: TossInvestApiError,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            status,
            api_error: Some(api_error),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn api_error(&self) -> Option<&TossInvestApiError> {
        self.api_error.as_ref()
    }
    pub fn code(&self) -> Option<&str> {
        self.api_error
            .as_ref()
            .and_then(|error| error.code.as_deref())
    }
    pub fn is_maintenance(&self) -> bool {
        self.code()
            .is_some_and(|code| code.trim().eq_ignore_ascii_case("maintenance"))
    }
}

impl fmt::Display for TossInvestRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(error) = &self.api_error {
            return write!(
                f,
                "Toss Invest {} request failed with status {} (request_id: {}, code: {}, message: {})",
                self.endpoint,
                self.status,
                error.request_id.as_deref().unwrap_or("unknown"),
                error.code.as_deref().unwrap_or("unknown"),
                error.message.as_deref().unwrap_or("unknown")
            );
        }
        write!(
            f,
            "Toss Invest {} request failed with status {}",
            self.endpoint, self.status
        )
    }
}

impl std::error::Error for TossInvestRequestError {}
