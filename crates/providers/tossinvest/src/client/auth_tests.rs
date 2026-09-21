#![allow(unused_imports)]

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use chrono::{TimeDelta, Utc};
use reqwest::{
    Method, StatusCode,
    header::{AUTHORIZATION, HeaderValue},
};

use crate::TossRateLimitGroup;

use super::{
    TossInvestClient, TossInvestRequestError, TossInvestResponse, auth::build_oauth_error,
    state::CachedAccessToken,
};

const DEFAULT_TEST_BASE_URL: &str = "https://openapi.tossinvest.com";

fn test_config(client_id: &str) -> crate::TossInvestConfig {
    test_config_with_base_url(client_id, DEFAULT_TEST_BASE_URL)
}

fn test_config_with_base_url(client_id: &str, base_url: &str) -> crate::TossInvestConfig {
    crate::TossInvestConfig::from_map(&BTreeMap::from([
        ("TOSSINVEST_CLIENT_ID".to_string(), client_id.to_string()),
        (
            "TOSSINVEST_CLIENT_SECRET".to_string(),
            "client-secret".to_string(),
        ),
        ("TOSSINVEST_BASE_URL".to_string(), base_url.to_string()),
    ]))
    .unwrap()
}
#[test]
fn oauth_toss_maintenance_error_preserves_structured_classification() {
    let error = build_oauth_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        r#"{
                "error": {
                    "requestId": "request-789",
                    "code": "maintenance",
                    "message": "any locale is fine"
                }
            }"#,
    );

    let structured = error.downcast_ref::<TossInvestRequestError>().unwrap();
    assert!(structured.is_maintenance());
    assert_eq!(structured.endpoint(), "OAuth token");
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
    let client = TossInvestClient::new(test_config("client-id-invalid-token"));
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
    let client = TossInvestClient::new(test_config("client-id-expired-token"));
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
    let client = TossInvestClient::new(test_config("client-id-nonretryable-401"));
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
