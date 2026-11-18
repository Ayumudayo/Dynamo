use std::collections::HashMap;

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{DateTime, Duration, Utc};
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

use crate::{
    DashboardSession, DashboardState, DiscordOAuthUser, auth::random_token,
    discord::send_dashboard_http, handlers::api::error_payload, require_api_session,
};

type HmacSha1 = Hmac<Sha1>;

pub(crate) const OWNER_ELEVATION_COOKIE_NAME: &str = "dynamo_owner_elevation";
pub(crate) const OWNER_ELEVATION_IDLE_MINUTES: i64 = 20;
pub(crate) const OWNER_ELEVATION_MAX_HOURS: i64 = 2;
pub(crate) const OWNER_IDENTITY_RECHECK_MINUTES: i64 = 5;
pub(crate) const TOTP_MAX_FAILURES: usize = 5;
pub(crate) const TOTP_LOCKOUT_MINUTES: i64 = 5;
#[cfg(any(test, feature = "local-preview"))]
pub(crate) const DEMO_TOTP_SECRET_BASE32: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

#[derive(Debug, Clone)]
pub(crate) struct OwnerElevation {
    pub(crate) user_id: u64,
    pub(crate) session_csrf_token: String,
    pub(crate) elevated_at: DateTime<Utc>,
    pub(crate) last_seen_at: DateTime<Utc>,
    pub(crate) last_identity_verified_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub(crate) struct OwnerSecurityState {
    pub(crate) elevations: HashMap<String, OwnerElevation>,
    pub(crate) last_unlock_step: Option<u64>,
    pub(crate) last_action_step: Option<u64>,
    pub(crate) failed_attempts: Vec<DateTime<Utc>>,
    pub(crate) locked_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerUnlockError {
    NotConfigured,
    RateLimited,
    InvalidCode,
    ReplayedCode,
}

impl OwnerSecurityState {
    fn is_locked(&mut self, now: DateTime<Utc>) -> bool {
        if let Some(until) = self.locked_until {
            if now < until {
                return true;
            }
            self.locked_until = None;
            self.failed_attempts.clear();
        }
        false
    }

    fn record_failure(&mut self, now: DateTime<Utc>) -> bool {
        let cutoff = now - Duration::minutes(TOTP_LOCKOUT_MINUTES);
        self.failed_attempts.retain(|ts| *ts >= cutoff);
        self.failed_attempts.push(now);
        if self.failed_attempts.len() >= TOTP_MAX_FAILURES {
            self.locked_until = Some(now + Duration::minutes(TOTP_LOCKOUT_MINUTES));
            return true;
        }
        false
    }

    fn clear_failures(&mut self) {
        self.failed_attempts.clear();
        self.locked_until = None;
    }
}

pub(crate) fn parse_totp_secret(raw: &str) -> anyhow::Result<Vec<u8>> {
    let cleaned: String = raw
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-' && *ch != '=')
        .map(|ch| ch.to_ascii_uppercase())
        .collect();
    if cleaned.is_empty() {
        anyhow::bail!("DASHBOARD_OWNER_TOTP_SECRET must not be empty");
    }
    let decoded = BASE32_NOPAD.decode(cleaned.as_bytes()).map_err(|error| {
        anyhow::anyhow!("DASHBOARD_OWNER_TOTP_SECRET must be valid Base32: {error}")
    })?;
    if decoded.len() < 10 {
        anyhow::bail!("DASHBOARD_OWNER_TOTP_SECRET must decode to at least 10 bytes (80 bits)");
    }
    Ok(decoded)
}

#[cfg(any(test, feature = "local-preview"))]
pub(crate) fn current_totp_step(now: DateTime<Utc>) -> u64 {
    now.timestamp().max(0) as u64 / 30
}

pub(crate) fn totp_code_for_step(secret: &[u8], step: u64) -> String {
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts keys of any size");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = ((u32::from(digest[offset]) & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    let otp = binary % 1_000_000;
    format!("{otp:06}")
}

pub(crate) fn verify_totp_code(secret: &[u8], code: &str, now_unix: u64) -> Option<u64> {
    let normalized = code.trim();
    if normalized.len() != 6 || !normalized.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let current_step = now_unix / 30;
    let candidate_steps = [
        current_step.saturating_sub(1),
        current_step,
        current_step.saturating_add(1),
    ];
    let mut matched = None;
    for step in candidate_steps {
        let expected = totp_code_for_step(secret, step);
        if bool::from(normalized.as_bytes().ct_eq(expected.as_bytes())) && matched.is_none() {
            matched = Some(step);
        }
    }
    matched
}

pub(crate) fn is_application_owner(state: &DashboardState, session: &DashboardSession) -> bool {
    state.app_info.owner_user_id == Some(session.user.id)
}

pub(crate) fn owner_elevation_cookie(token: &str, secure: bool) -> Cookie<'static> {
    let max_age = OWNER_ELEVATION_MAX_HOURS * 3600;
    let secure_attr = if secure { "; Secure" } else { "" };
    let mut cookie = Cookie::parse(format!(
        "{OWNER_ELEVATION_COOKIE_NAME}=placeholder; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Strict{secure_attr}"
    ))
    .expect("valid elevation cookie template")
    .into_owned();
    cookie.set_value(token.to_string());
    cookie
}

pub(crate) fn clear_owner_elevation_cookie(secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::from(OWNER_ELEVATION_COOKIE_NAME);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Strict);
    cookie.set_path("/");
    if secure {
        cookie.set_secure(true);
    }
    cookie
}

pub(crate) async fn verify_live_owner_identity(
    state: &DashboardState,
    session: &DashboardSession,
) -> Result<(), StatusCode> {
    if !is_application_owner(state, session) {
        return Err(StatusCode::FORBIDDEN);
    }

    #[cfg(feature = "perf-harness")]
    if state.perf_runtime.is_some() {
        return Ok(());
    }

    #[cfg(feature = "local-preview")]
    if state.local_preview.is_some() {
        return Ok(());
    }

    let request = state
        .http
        .get(format!("{}/users/@me", state.discord_api_base))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", session.access_token),
        );
    let response = send_dashboard_http(state, request)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !response.status().is_success() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let user: DiscordOAuthUser = response
        .json()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let Ok(parsed_id) = user.id.parse::<u64>() else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    if parsed_id != session.user.id || state.app_info.owner_user_id != Some(parsed_id) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

pub(crate) async fn load_valid_elevation(
    state: &DashboardState,
    jar: &CookieJar,
    session: &DashboardSession,
) -> Result<Option<OwnerElevation>, StatusCode> {
    if !is_application_owner(state, session) || state.config.owner_totp_secret.is_none() {
        return Ok(None);
    }
    let Some(token) = jar
        .get(OWNER_ELEVATION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
    else {
        return Ok(None);
    };

    let now = Utc::now();
    let candidate = {
        let mut security = state.owner_security.write().await;
        let Some(existing) = security.elevations.get(&token) else {
            return Ok(None);
        };
        let idle_expired =
            now - existing.last_seen_at >= Duration::minutes(OWNER_ELEVATION_IDLE_MINUTES);
        let max_expired = now - existing.elevated_at >= Duration::hours(OWNER_ELEVATION_MAX_HOURS);
        let identity_mismatch = existing.user_id != session.user.id
            || !bool::from(
                existing
                    .session_csrf_token
                    .as_bytes()
                    .ct_eq(session.csrf_token.as_bytes()),
            );
        if idle_expired || max_expired || identity_mismatch {
            security.elevations.remove(&token);
            return Ok(None);
        }
        existing.clone()
    };

    let needs_recheck = now - candidate.last_identity_verified_at
        >= Duration::minutes(OWNER_IDENTITY_RECHECK_MINUTES);
    if needs_recheck && let Err(status) = verify_live_owner_identity(state, session).await {
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            state.owner_security.write().await.elevations.remove(&token);
        }
        return Err(status);
    }

    let mut security = state.owner_security.write().await;
    let Some(existing) = security.elevations.get_mut(&token) else {
        return Ok(None);
    };
    existing.last_seen_at = now;
    if needs_recheck {
        existing.last_identity_verified_at = now;
    }
    Ok(Some(existing.clone()))
}

pub(crate) async fn verify_unlock_totp(
    state: &DashboardState,
    session: &DashboardSession,
    code: &str,
) -> Result<String, OwnerUnlockError> {
    let Some(secret) = state.config.owner_totp_secret.as_deref() else {
        return Err(OwnerUnlockError::NotConfigured);
    };
    let now = Utc::now();
    let now_unix = now.timestamp().max(0) as u64;
    let mut security = state.owner_security.write().await;
    if security.is_locked(now) {
        return Err(OwnerUnlockError::RateLimited);
    }

    let Some(step) = verify_totp_code(secret, code, now_unix) else {
        let locked = security.record_failure(now);
        return Err(if locked {
            OwnerUnlockError::RateLimited
        } else {
            OwnerUnlockError::InvalidCode
        });
    };

    if security
        .last_unlock_step
        .is_some_and(|previous| step <= previous)
    {
        return Err(OwnerUnlockError::ReplayedCode);
    }

    security.last_unlock_step = Some(step);
    security.clear_failures();
    let token = random_token(32);
    security.elevations.insert(
        token.clone(),
        OwnerElevation {
            user_id: session.user.id,
            session_csrf_token: session.csrf_token.clone(),
            elevated_at: now,
            last_seen_at: now,
            last_identity_verified_at: now,
        },
    );
    Ok(token)
}

pub(crate) async fn revoke_elevation(
    state: &DashboardState,
    jar: &CookieJar,
    session: Option<&DashboardSession>,
) {
    let cookie_token = jar
        .get(OWNER_ELEVATION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string());
    let mut security = state.owner_security.write().await;
    if let Some(token) = cookie_token {
        security.elevations.remove(&token);
    }
    if let Some(session) = session {
        security
            .elevations
            .retain(|_, elevation| elevation.session_csrf_token != session.csrf_token);
    }
}

#[allow(clippy::result_large_err)]
pub(crate) fn verify_mutation_request_security(
    headers: &HeaderMap,
    session: &DashboardSession,
    form_csrf: Option<&str>,
) -> Result<(), Response> {
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
    {
        return Err((
            StatusCode::FORBIDDEN,
            Json(error_payload("cross-site mutation request rejected")),
        )
            .into_response());
    }

    let header_token = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok());
    let provided = header_token.or(form_csrf).map(str::trim);
    let matches = provided.is_some_and(|candidate| {
        !candidate.is_empty()
            && bool::from(candidate.as_bytes().ct_eq(session.csrf_token.as_bytes()))
    });
    if !matches {
        return Err((
            StatusCode::FORBIDDEN,
            Json(error_payload("invalid or missing CSRF token")),
        )
            .into_response());
    }

    Ok(())
}

#[allow(clippy::result_large_err)]
pub(crate) async fn require_elevated_api_owner(
    state: &DashboardState,
    jar: &CookieJar,
    headers: &HeaderMap,
    require_csrf: bool,
) -> Result<DashboardSession, Response> {
    let session = require_api_session(state, jar).await?;
    if !is_application_owner(state, &session) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(error_payload("application owner access required")),
        )
            .into_response());
    }
    if require_csrf {
        verify_mutation_request_security(headers, &session, None)?;
    }
    if state.config.owner_totp_secret.is_none() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(error_payload(
                "owner TOTP authenticator secret is not configured",
            )),
        )
            .into_response());
    }
    match load_valid_elevation(state, jar, &session).await {
        Ok(Some(_)) => Ok(session),
        Ok(None) => Err((
            StatusCode::FORBIDDEN,
            Json(error_payload("owner console elevation required")),
        )
            .into_response()),
        Err(StatusCode::UNAUTHORIZED) => Err((
            StatusCode::UNAUTHORIZED,
            Json(error_payload("Discord authorization expired; log in again")),
        )
            .into_response()),
        Err(StatusCode::FORBIDDEN) => Err((
            StatusCode::FORBIDDEN,
            Json(error_payload("application owner access required")),
        )
            .into_response()),
        Err(_) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "Discord owner identity verification is temporarily unavailable",
            )),
        )
            .into_response()),
    }
}

#[allow(clippy::result_large_err)]
pub(crate) async fn require_fresh_action_totp(
    state: &DashboardState,
    code: &str,
) -> Result<(), Response> {
    let Some(secret) = state.config.owner_totp_secret.as_deref() else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(error_payload(
                "owner TOTP authenticator secret is not configured",
            )),
        )
            .into_response());
    };
    let now = Utc::now();
    let now_unix = now.timestamp().max(0) as u64;
    let mut security = state.owner_security.write().await;
    if security.is_locked(now) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(error_payload(
                "too many failed verification attempts; wait 5 minutes before retrying",
            )),
        )
            .into_response());
    }
    let Some(step) = verify_totp_code(secret, code, now_unix) else {
        let locked = security.record_failure(now);
        return Err(if locked {
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(error_payload(
                    "too many failed verification attempts; wait 5 minutes before retrying",
                )),
            )
                .into_response()
        } else {
            (
                StatusCode::BAD_REQUEST,
                Json(error_payload("invalid 6-digit verification code")),
            )
                .into_response()
        });
    };

    if security
        .last_action_step
        .is_some_and(|previous| step <= previous)
    {
        return Err((
            StatusCode::CONFLICT,
            Json(error_payload(
                "verification code already used; wait for the next 30-second code",
            )),
        )
            .into_response());
    }

    security.last_action_step = Some(step);
    security.clear_failures();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_totp_secret, totp_code_for_step, verify_totp_code};

    #[test]
    fn rfc6238_sha1_test_vectors_match() {
        let secret = b"12345678901234567890";
        assert_eq!(totp_code_for_step(secret, 59 / 30), "287082");
        assert_eq!(totp_code_for_step(secret, 1_111_111_109 / 30), "081804");
        assert_eq!(totp_code_for_step(secret, 1_111_111_111 / 30), "050471");
        assert_eq!(totp_code_for_step(secret, 1_234_567_890 / 30), "005924");
        assert_eq!(totp_code_for_step(secret, 2_000_000_000 / 30), "279037");
    }

    #[test]
    fn verify_totp_accepts_adjacent_window_and_rejects_invalid() {
        let secret = parse_totp_secret("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").expect("valid base32");
        let now = 1_700_000_000;
        let current = totp_code_for_step(&secret, now / 30);
        let prev = totp_code_for_step(&secret, (now / 30) - 1);
        assert_eq!(verify_totp_code(&secret, &current, now), Some(now / 30));
        assert_eq!(verify_totp_code(&secret, &prev, now), Some((now / 30) - 1));
        assert_eq!(verify_totp_code(&secret, "000000", now), None);
        assert_eq!(verify_totp_code(&secret, "abc123", now), None);
    }

    #[test]
    fn parse_totp_secret_rejects_short_or_invalid_values() {
        assert!(parse_totp_secret("").is_err());
        assert!(parse_totp_secret("JBSWY3DP").is_err());
        assert!(parse_totp_secret("not-valid-base32!").is_err());
    }
}
