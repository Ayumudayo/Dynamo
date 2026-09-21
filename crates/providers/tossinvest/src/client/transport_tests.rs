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
#[tokio::test]
async fn oauth_authenticated_request_uses_base_url_and_cached_token() {
    let client = TossInvestClient::new(test_config("client-id-request-url"));

    client
        .test_set_cached_token(
            "cached-token",
            "Bearer",
            Utc::now() + TimeDelta::seconds(3600),
        )
        .await;

    let request = client
        .authenticated_request(
            TossRateLimitGroup::MarketData,
            Method::GET,
            "/api/v1/prices",
        )
        .await
        .unwrap()
        .build()
        .unwrap();

    assert_eq!(
        request.url().as_str(),
        "https://openapi.tossinvest.com/api/v1/prices"
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
    let client = TossInvestClient::new(test_config("client-id-reject-absolute"));

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
    let client = TossInvestClient::new(test_config("client-id-reject-scheme-relative"));

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
            "//evil.example/api",
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("relative"));
    assert!(error.contains("base URL"));
}

#[tokio::test]
async fn oauth_authenticated_request_rejects_auth_group_without_scheduling_auth_bucket() {
    let client = TossInvestClient::new(test_config("client-id-reject-auth-group"));

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
async fn oauth_send_authenticated_retries_once_with_refreshed_token_for_invalid_token() {
    let client = TossInvestClient::new(test_config("client-id-send-retry-invalid-token"));
    client
        .test_set_cached_token("old-token", "Bearer", Utc::now() + TimeDelta::seconds(3600))
        .await;
    client
        .test_set_next_refresh_token("new-token", "Bearer", Utc::now() + TimeDelta::seconds(3600))
        .await;

    let seen_headers = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_headers_for_execute = seen_headers.clone();
    let response = client
            .send_authenticated_with(
                TossRateLimitGroup::MarketData,
                Method::GET,
                "/api/v1/prices",
                move |request| {
                    let seen_headers_for_execute = seen_headers_for_execute.clone();
                    async move {
                        let request = request.build().unwrap();
                        let authorization = request
                            .headers()
                            .get(AUTHORIZATION)
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .to_string();
                        seen_headers_for_execute.lock().unwrap().push(authorization);

                        let attempt = seen_headers_for_execute.lock().unwrap().len();
                        Ok(if attempt == 1 {
                            TossInvestResponse::test_json(
                                StatusCode::UNAUTHORIZED,
                                r#"{"error":{"code":"invalid-token","message":"expired","requestId":"req-1"}}"#,
                            )
                        } else {
                            TossInvestResponse::test_json(
                                StatusCode::OK,
                                r#"{"result":{"ok":true}}"#,
                            )
                        })
                    }
                },
            )
            .await
            .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        seen_headers.lock().unwrap().as_slice(),
        ["Bearer old-token", "Bearer new-token"]
    );
}

#[tokio::test]
async fn oauth_total_deadline_includes_waiting_for_token_lock() {
    let client = TossInvestClient::new(test_config("client-id-deadline-token-lock"));
    let _token_lock = client.shared_state.access_token.lock().await;

    let error = client
        .send_authenticated_with_deadline(
            TossRateLimitGroup::MarketData,
            Method::GET,
            "/api/v1/prices",
            Duration::from_millis(20),
            |_| async { unreachable!("request must not execute while token lock is held") },
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("total deadline"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn oauth_total_deadline_includes_endpoint_limiter_wait() {
    let client = TossInvestClient::new(test_config("client-id-deadline-endpoint-limiter"));
    client
        .test_set_cached_token(
            "cached-token",
            "Bearer",
            Utc::now() + TimeDelta::seconds(3600),
        )
        .await;
    client
        .rate_limiter()
        .acquire(TossRateLimitGroup::MarketData)
        .await;

    let error = client
        .send_authenticated_with_deadline(
            TossRateLimitGroup::MarketData,
            Method::GET,
            "/api/v1/prices",
            Duration::from_millis(20),
            |_| async { unreachable!("request must not execute before limiter admission") },
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("total deadline"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn oauth_total_deadline_is_not_reset_for_second_attempt_body() {
    let client = TossInvestClient::new(test_config("client-id-deadline-second-body"));
    client
        .test_set_cached_token("old-token", "Bearer", Utc::now() + TimeDelta::seconds(3600))
        .await;
    client
        .test_set_next_refresh_token("new-token", "Bearer", Utc::now() + TimeDelta::seconds(3600))
        .await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_execute = attempts.clone();

    let error = client
            .send_authenticated_with_deadline(
                TossRateLimitGroup::MarketData,
                Method::GET,
                "/api/v1/prices",
                Duration::from_millis(250),
                move |_| {
                    let attempt = attempts_for_execute.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt == 0 {
                            Ok(TossInvestResponse::test_json(
                                StatusCode::UNAUTHORIZED,
                                r#"{"error":{"code":"invalid-token","message":"expired","requestId":"req-1"}}"#,
                            ))
                        } else {
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            Ok(TossInvestResponse::test_json(
                                StatusCode::OK,
                                r#"{"result":{"ok":true}}"#,
                            ))
                        }
                    }
                },
            )
            .await
            .unwrap_err()
            .to_string();

    assert!(
        error.contains("total deadline"),
        "unexpected error: {error}"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn oauth_send_authenticated_returns_429_and_propagates_shared_cooldown() {
    let client = TossInvestClient::new(test_config("client-id-send-429"));
    client
        .test_set_cached_token(
            "cached-token",
            "Bearer",
            Utc::now() + TimeDelta::seconds(3600),
        )
        .await;

    let response = client
            .send_authenticated_with(
                TossRateLimitGroup::MarketData,
                Method::GET,
                "/api/v1/prices",
                |_| async {
                    Ok(TossInvestResponse::test_with_headers(
                        StatusCode::TOO_MANY_REQUESTS,
                        r#"{"error":{"code":"too-many-requests","message":"slow down","requestId":"req-1"}}"#,
                        &[("x-ratelimit-reset", "5")],
                    ))
                },
            )
            .await
            .unwrap();

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let remaining = client
        .rate_limiter()
        .test_remaining_delay_from(TossRateLimitGroup::MarketData, std::time::Instant::now())
        .await
        .unwrap();
    assert!(remaining >= std::time::Duration::from_secs(5));
}
