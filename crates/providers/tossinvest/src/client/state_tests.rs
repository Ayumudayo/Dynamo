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
fn oauth_mutating_test_identities_are_unique() {
    let identities = [
        ("client-id-request-url", DEFAULT_TEST_BASE_URL),
        ("client-id-reject-absolute", DEFAULT_TEST_BASE_URL),
        ("client-id-reject-scheme-relative", DEFAULT_TEST_BASE_URL),
        ("client-id-reject-auth-group", DEFAULT_TEST_BASE_URL),
        ("client-id-debug-redaction", DEFAULT_TEST_BASE_URL),
        ("client-id-invalid-token", DEFAULT_TEST_BASE_URL),
        ("client-id-expired-token", DEFAULT_TEST_BASE_URL),
        ("client-id-nonretryable-401", DEFAULT_TEST_BASE_URL),
    ];

    let unique = identities.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), identities.len());
}

#[test]
fn oauth_cached_token_refreshes_when_expiring_within_60_seconds() {
    let now = Utc::now();
    let fresh_token = CachedAccessToken::new("fresh-token", "Bearer", now + TimeDelta::seconds(61));
    let expiring_token =
        CachedAccessToken::new("expiring-token", "Bearer", now + TimeDelta::seconds(60));

    assert!(!fresh_token.needs_refresh(now));
    assert!(expiring_token.needs_refresh(now));
}

#[test]
fn oauth_clients_with_same_identity_share_in_process_state() {
    let config = test_config("client-id-shared-state");

    let first = TossInvestClient::new(config.clone());
    let second = TossInvestClient::new(config);

    assert!(Arc::ptr_eq(&first.shared_state, &second.shared_state));
}

#[test]
fn oauth_clients_share_state_across_trailing_slash_base_url_spellings() {
    let first = TossInvestClient::new(test_config_with_base_url(
        "client-id-shared-trailing-slash",
        "https://openapi.tossinvest.com",
    ));
    let second = TossInvestClient::new(test_config_with_base_url(
        "client-id-shared-trailing-slash",
        "https://openapi.tossinvest.com/",
    ));

    assert!(Arc::ptr_eq(&first.shared_state, &second.shared_state));
}
