use axum::http::StatusCode;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use rand::{Rng, distributions::Alphanumeric};
use tracing::warn;

use crate::discord::{build_oauth_session, oauth_token_request, send_dashboard_http};
use crate::{
    DashboardGuild, DashboardSession, DashboardState, DiscordOAuthUser, DiscordTokenResponse,
    OAUTH_STATE_TTL_MINUTES, PendingOauthState, SESSION_COOKIE_NAME,
};

pub(crate) async fn load_session(
    state: &DashboardState,
    jar: &CookieJar,
) -> Option<DashboardSession> {
    let session_id = session_cookie_value(jar)?;
    let session = state.sessions.read().await.get(&session_id).cloned()?;
    if !is_session_expired(&session) {
        return Some(session);
    }

    // Only remove the expired entry observed by this request. Re-check its identity
    // after acquiring the exclusive lock so an OAuth replacement is never removed.
    let mut sessions = state.sessions.write().await;
    if sessions.get(&session_id).is_some_and(|current| {
        current.access_token == session.access_token && is_session_expired(current)
    }) {
        sessions.remove(&session_id);
    }
    None
}

pub(crate) fn session_cookie_value(jar: &CookieJar) -> Option<String> {
    jar.get(SESSION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
}

pub(crate) fn is_session_expired(session: &DashboardSession) -> bool {
    session.expires_at <= chrono::Utc::now()
}

pub(crate) fn is_oauth_state_expired(pending: &PendingOauthState) -> bool {
    pending.created_at + chrono::Duration::minutes(OAUTH_STATE_TTL_MINUTES) <= chrono::Utc::now()
}

pub(crate) fn sanitize_redirect_target(target: Option<&str>) -> String {
    let candidate = target.unwrap_or("/selector").trim();
    if candidate.starts_with('/') && !candidate.starts_with("//") {
        candidate.to_string()
    } else {
        "/selector".to_string()
    }
}

pub(crate) fn random_token(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

pub(crate) fn session_cookie(session_id: &str) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE_NAME, session_id.to_string());
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie
}

pub(crate) async fn exchange_oauth_code(
    state: &DashboardState,
    code: &str,
) -> Result<DashboardSession, anyhow::Error> {
    let token_request = oauth_token_request(state, code);
    let token_response = send_dashboard_http(state, token_request)
        .await?
        .error_for_status()?;

    let token_payload: DiscordTokenResponse = token_response.json().await?;
    let bearer = format!("Bearer {}", token_payload.access_token);

    let user_request = state
        .http
        .get(format!("{}/users/@me", state.discord_api_base))
        .header(reqwest::header::AUTHORIZATION, &bearer);
    let user_response = send_dashboard_http(state, user_request)
        .await?
        .error_for_status()?;
    let user: DiscordOAuthUser = user_response.json().await?;

    let guilds_request = state
        .http
        .get(format!("{}/users/@me/guilds", state.discord_api_base))
        .header(reqwest::header::AUTHORIZATION, &bearer);
    let guilds_response = send_dashboard_http(state, guilds_request)
        .await?
        .error_for_status()?;
    let guilds: Vec<DashboardGuild> = guilds_response.json().await?;

    build_oauth_session(token_payload, user, guilds)
}

pub(crate) fn session_can_manage_guild(session: &DashboardSession, guild_id: u64) -> bool {
    session
        .guilds
        .iter()
        .any(|guild| guild.id == guild_id && user_can_manage_guild(guild))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshSessionGuildsError {
    Unauthorized,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadGuildAuthorizationError {
    LoginRequired,
    Unavailable,
}

/// Refreshes the Discord guild grant without holding the session lock during I/O.
/// The token identity guard prevents an in-flight refresh from writing into a
/// replacement session that reused the same cookie key.
pub(crate) async fn refresh_session_guilds(
    state: &DashboardState,
    session_id: &str,
) -> Result<Option<DashboardSession>, RefreshSessionGuildsError> {
    #[cfg(feature = "perf-harness")]
    if state.perf_runtime.is_some() {
        return Ok(state.sessions.read().await.get(session_id).cloned());
    }

    let access_token = {
        let sessions = state.sessions.read().await;
        sessions
            .get(session_id)
            .map(|session| session.access_token.clone())
    };
    let Some(access_token) = access_token else {
        return Ok(None);
    };

    let bearer = format!("Bearer {}", access_token);
    let guilds_request = state
        .http
        .get(format!("{}/users/@me/guilds", state.discord_api_base))
        .header(reqwest::header::AUTHORIZATION, &bearer);
    let refresh_started_at = std::time::Instant::now();
    let guilds_response = send_dashboard_http(state, guilds_request)
        .await
        .map_err(|error| {
            warn!(
                ?error,
                elapsed_ms = refresh_started_at.elapsed().as_millis() as u64,
                "Discord guild authorization refresh request failed"
            );
            RefreshSessionGuildsError::Unavailable
        })?;
    if guilds_response.status() == StatusCode::UNAUTHORIZED {
        warn!(status = %guilds_response.status(), elapsed_ms = refresh_started_at.elapsed().as_millis() as u64, "Discord guild authorization refresh requires login");
        let mut sessions = state.sessions.write().await;
        if sessions
            .get(session_id)
            .is_some_and(|current| current.access_token == access_token)
        {
            sessions.remove(session_id);
        }
        return Err(RefreshSessionGuildsError::Unauthorized);
    }
    if !guilds_response.status().is_success() {
        warn!(status = %guilds_response.status(), elapsed_ms = refresh_started_at.elapsed().as_millis() as u64, "Discord guild authorization refresh was unavailable");
        return Err(RefreshSessionGuildsError::Unavailable);
    }
    let guilds: Vec<DashboardGuild> = guilds_response.json().await.map_err(|error| {
        warn!(
            ?error,
            elapsed_ms = refresh_started_at.elapsed().as_millis() as u64,
            "Discord guild authorization refresh response was invalid"
        );
        RefreshSessionGuildsError::Unavailable
    })?;
    let response_elapsed_ms = refresh_started_at.elapsed().as_millis() as u64;
    let guild_count = guilds.len();

    let update_started_at = std::time::Instant::now();
    let mut sessions = state.sessions.write().await;
    let Some(session) = sessions.get_mut(session_id) else {
        warn!(
            response_elapsed_ms,
            session_update_elapsed_ms = update_started_at.elapsed().as_millis() as u64,
            "Discord guild authorization refresh discarded because the session expired"
        );
        return Ok(None);
    };
    if session.access_token != access_token {
        warn!(
            response_elapsed_ms,
            session_update_elapsed_ms = update_started_at.elapsed().as_millis() as u64,
            "Discord guild authorization refresh discarded because the session changed"
        );
        return Ok(None);
    }
    session.guilds = guilds;
    tracing::info!(
        response_elapsed_ms,
        guilds = guild_count,
        session_update_elapsed_ms = update_started_at.elapsed().as_millis() as u64,
        "Discord guild authorization refresh completed"
    );
    Ok(Some(session.clone()))
}

pub(crate) async fn refresh_read_session(
    state: &DashboardState,
    jar: &CookieJar,
) -> Result<DashboardSession, ReadGuildAuthorizationError> {
    let session_id = session_cookie_value(jar).ok_or(ReadGuildAuthorizationError::LoginRequired)?;
    match refresh_session_guilds(state, &session_id).await {
        Ok(Some(session)) => Ok(session),
        Ok(None) | Err(RefreshSessionGuildsError::Unauthorized) => {
            Err(ReadGuildAuthorizationError::LoginRequired)
        }
        Err(RefreshSessionGuildsError::Unavailable) => {
            Err(ReadGuildAuthorizationError::Unavailable)
        }
    }
}

pub(crate) fn user_can_manage_guild(guild: &DashboardGuild) -> bool {
    let Ok(bits) = guild.permissions.parse::<u64>() else {
        return false;
    };
    let administrator = 1 << 3;
    let manage_guild = 1 << 5;
    bits & administrator == administrator || bits & manage_guild == manage_guild
}
