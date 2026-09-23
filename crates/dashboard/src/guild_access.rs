use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use futures_util::{StreamExt, stream};
use tracing::warn;

use crate::{
    DashboardGuild, DashboardSession, DashboardState,
    auth::{
        refresh_session_guilds, session_can_manage_guild, session_cookie_value,
        user_can_manage_guild,
    },
    discord::{build_bot_invite_url, guild_icon_url, send_dashboard_http},
    error_payload,
    render::document::{BotGuildPresence, GuildCard},
    require_api_session,
};

/// Loads every guild the user can manage, keeping Discord bot-presence requests
/// concurrently bounded and the selector ordering stable.
pub(crate) async fn load_guild_cards(
    state: &DashboardState,
    session: &DashboardSession,
) -> Vec<GuildCard> {
    let manageable = session
        .guilds
        .iter()
        .filter(|guild| user_can_manage_guild(guild))
        .cloned()
        .collect::<Vec<_>>();
    let manageable_count = manageable.len();
    let lookup_started_at = std::time::Instant::now();

    let mut cards = stream::iter(manageable.into_iter().map(|guild| async move {
        let started_at = std::time::Instant::now();
        let guild_id = guild.id;
        let bot_presence = bot_is_in_guild(state, guild.id).await;
        tracing::info!(guild_id, elapsed_ms = started_at.elapsed().as_millis() as u64, presence = ?bot_presence, "dashboard selector guild bot-presence lookup completed");
        GuildCard {
            id: guild.id,
            name: guild.name.clone(),
            icon_url: guild_icon_url(&guild),
            bot_presence,
            manage_url: format!("/guild/{}", guild.id),
            invite_url: build_bot_invite_url(state, guild.id),
        }
    }))
    .buffer_unordered(8)
    .collect::<Vec<_>>()
    .await;
    let lookup_elapsed_ms = lookup_started_at.elapsed().as_millis() as u64;
    let sort_started_at = std::time::Instant::now();
    sort_guild_cards(&mut cards);
    tracing::info!(
        guilds = manageable_count,
        concurrency_limit = 8,
        lookup_elapsed_ms,
        sort_elapsed_ms = sort_started_at.elapsed().as_millis() as u64,
        "dashboard selector guild cards assembled"
    );
    cards
}

/// Builds the one card needed by the guild-detail route without rechecking every
/// manageable guild merely to locate the requested one. The selector deliberately
/// continues to use `load_guild_cards`, which keeps its bounded concurrent lookup
/// and stable ordering contract.
pub(crate) async fn load_guild_card(
    state: &DashboardState,
    session: &DashboardSession,
    guild_id: u64,
) -> Option<GuildCard> {
    let guild = session
        .guilds
        .iter()
        .find(|guild| guild.id == guild_id && user_can_manage_guild(guild))?;

    Some(GuildCard {
        id: guild.id,
        name: guild.name.clone(),
        icon_url: guild_icon_url(guild),
        bot_presence: bot_is_in_guild(state, guild.id).await,
        manage_url: format!("/guild/{}", guild.id),
        invite_url: build_bot_invite_url(state, guild.id),
    })
}

pub(crate) fn sort_guild_cards(cards: &mut [GuildCard]) {
    cards.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
}

pub(crate) async fn bot_is_in_guild(state: &DashboardState, guild_id: u64) -> BotGuildPresence {
    #[cfg(feature = "perf-harness")]
    if let Some(runtime) = state.perf_runtime.as_ref() {
        return if runtime.fixture_bot_present().await {
            BotGuildPresence::Present
        } else {
            BotGuildPresence::Missing
        };
    }

    let request = state
        .http
        .get(format!("{}/guilds/{guild_id}", state.discord_api_base))
        .header("Authorization", format!("Bot {}", state.config.bot_token));
    match send_dashboard_http(state, request).await {
        Ok(response) => {
            let presence = classify_bot_guild_status(response.status());
            if presence != BotGuildPresence::Unavailable {
                return presence;
            }
            warn!(
                guild_id,
                status = %response.status(),
                "Discord guild presence lookup returned an unavailable status"
            );
            BotGuildPresence::Unavailable
        }
        Err(error) => {
            warn!(
                ?error,
                guild_id, "Discord guild presence lookup was unavailable"
            );
            BotGuildPresence::Unavailable
        }
    }
}

pub(crate) fn classify_bot_guild_status(status: StatusCode) -> BotGuildPresence {
    if status.is_success() {
        BotGuildPresence::Present
    } else if status == StatusCode::NOT_FOUND {
        BotGuildPresence::Missing
    } else {
        BotGuildPresence::Unavailable
    }
}

/// Checks cached guild authorization first, then performs one refresh only when
/// that cached authorization denies the requested guild.
#[allow(clippy::result_large_err)]
pub(crate) async fn require_api_guild_access(
    state: &DashboardState,
    jar: &CookieJar,
    guild_id: u64,
) -> Result<DashboardSession, Response> {
    let session = require_api_session(state, jar).await?;
    if session_can_manage_guild(&session, guild_id) {
        Ok(session)
    } else {
        if let Some(session_id) = session_cookie_value(jar) {
            match refresh_session_guilds(state, &session_id).await {
                Ok(Some(refreshed)) if session_can_manage_guild(&refreshed, guild_id) => {
                    return Ok(refreshed);
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        user_id = session.user.id,
                        guild_id,
                        ?error,
                        "failed to refresh dashboard guild access state"
                    );
                }
            }
        }

        warn!(
            user_id = session.user.id,
            guild_id,
            shared_guild_ids = ?session.guilds.iter().map(|guild| guild.id).collect::<Vec<_>>(),
            "dashboard denied guild access"
        );

        Err((
            StatusCode::FORBIDDEN,
            Json(error_payload(
                "you do not have access to that guild in the dashboard".to_string(),
            )),
        )
            .into_response())
    }
}

/// Performs a fresh Discord authorization lookup before a guild-scoped write.
/// I/O completes before the session write lock is taken; the token identity guard
/// prevents an in-flight request from changing a replacement session.
#[allow(clippy::result_large_err)]
pub(crate) async fn require_current_api_guild_access(
    state: &DashboardState,
    jar: &CookieJar,
    guild_id: u64,
) -> Result<DashboardSession, Response> {
    let session_id = session_cookie_value(jar).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(error_payload("dashboard login required".to_string())),
        )
            .into_response()
    })?;
    let session = require_api_session(state, jar).await?;
    let access_token = session.access_token.clone();
    let request = state
        .http
        .get(format!("{}/users/@me/guilds", state.discord_api_base))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {access_token}"),
        );
    let response = match send_dashboard_http(state, request).await {
        Ok(response) => response,
        Err(error) => {
            warn!(
                user_id = session.user.id,
                guild_id,
                ?error,
                "current Discord guild authorization lookup failed"
            );
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(error_payload(
                    "current guild authorization is unavailable".to_string(),
                )),
            )
                .into_response());
        }
    };

    if response.status() == StatusCode::UNAUTHORIZED {
        let mut sessions = state.sessions.write().await;
        if sessions
            .get(&session_id)
            .is_some_and(|current| current.access_token == access_token)
        {
            sessions.remove(&session_id);
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(error_payload(
                "Discord authorization expired; log in again".to_string(),
            )),
        )
            .into_response());
    }

    if !response.status().is_success() {
        warn!(
            user_id = session.user.id,
            guild_id,
            status = %response.status(),
            "current Discord guild authorization lookup was unavailable"
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "current guild authorization is unavailable".to_string(),
            )),
        )
            .into_response());
    }

    let guilds = match response.json::<Vec<DashboardGuild>>().await {
        Ok(guilds) => guilds,
        Err(error) => {
            warn!(
                user_id = session.user.id,
                guild_id,
                ?error,
                "current Discord guild authorization response was invalid"
            );
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(error_payload(
                    "current guild authorization is unavailable".to_string(),
                )),
            )
                .into_response());
        }
    };

    let refreshed = {
        let mut sessions = state.sessions.write().await;
        let Some(current) = sessions.get_mut(&session_id) else {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(error_payload("dashboard login required".to_string())),
            )
                .into_response());
        };
        if current.access_token != access_token {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(error_payload("dashboard login required".to_string())),
            )
                .into_response());
        }
        current.guilds = guilds;
        current.clone()
    };

    if session_can_manage_guild(&refreshed, guild_id) {
        Ok(refreshed)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(error_payload(
                "you no longer have permission to manage that guild".to_string(),
            )),
        )
            .into_response())
    }
}
