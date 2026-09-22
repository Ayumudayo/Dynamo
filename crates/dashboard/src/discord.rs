use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use url::Url;

use crate::state::{
    DASHBOARD_CONNECT_TIMEOUT, DASHBOARD_REQUEST_TIMEOUT, DISCORD_API_BASE,
    DiscordApplicationResponse, SESSION_TTL_HOURS,
};
use crate::{
    DashboardConfig, DashboardGuild, DashboardSession, DashboardState, DashboardUser,
    DiscordApplicationInfo, DiscordOAuthUser, DiscordTokenResponse,
};

/// Constructs the client used for every dashboard-originated Discord request.
///
/// The timeout applies to the whole exchange, including response body reads. Keep
/// this boundary shared so startup, OAuth, and guild presence lookups have the
/// same bounded failure behavior.
pub(crate) fn build_dashboard_http_client_with_timeouts(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("Dynamo Dashboard/0.1.0")
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()?)
}

pub(crate) fn build_dashboard_http_client() -> anyhow::Result<reqwest::Client> {
    build_dashboard_http_client_with_timeouts(DASHBOARD_CONNECT_TIMEOUT, DASHBOARD_REQUEST_TIMEOUT)
}

pub(crate) async fn fetch_application_info(
    http: &reqwest::Client,
    config: &DashboardConfig,
) -> anyhow::Result<DiscordApplicationInfo> {
    let request = http
        .get(format!("{DISCORD_API_BASE}/oauth2/applications/@me"))
        .header(AUTHORIZATION, format!("Bot {}", config.bot_token));
    let response = execute_dashboard_http(request).await?.error_for_status()?;
    let payload: DiscordApplicationResponse = response.json().await?;

    Ok(application_info_from_response(payload))
}

fn application_info_from_response(payload: DiscordApplicationResponse) -> DiscordApplicationInfo {
    let owner_user_id = payload
        .owner
        .as_ref()
        .and_then(|owner| owner.id.parse::<u64>().ok())
        .or_else(|| {
            payload
                .team
                .as_ref()
                .and_then(|team| team.owner_user_id.parse::<u64>().ok())
        });

    DiscordApplicationInfo {
        id: payload.id,
        name: payload.name,
        icon: payload.icon,
        owner_user_id,
    }
}

pub(crate) fn build_discord_authorize_url(state: &DashboardState, oauth_state: &str) -> String {
    let mut url = Url::parse("https://discord.com/oauth2/authorize").expect("valid url");
    url.query_pairs_mut()
        .append_pair("client_id", &state.app_info.id)
        .append_pair("response_type", "code")
        .append_pair("scope", "identify guilds")
        .append_pair(
            "redirect_uri",
            &format!("{}/auth/discord/callback", state.config.public_base_url),
        )
        .append_pair("state", oauth_state);
    url.to_string()
}

/// Maps only the Discord OAuth wire payloads into the dashboard session domain.
/// Session storage and callback error handling remain in the authentication
/// route, outside this transport boundary.
pub(crate) fn build_oauth_session(
    token: DiscordTokenResponse,
    user: DiscordOAuthUser,
    guilds: Vec<DashboardGuild>,
) -> anyhow::Result<DashboardSession> {
    Ok(DashboardSession {
        user: DashboardUser {
            id: user.id.parse::<u64>()?,
            username: user.username,
            global_name: user.global_name,
            avatar: user.avatar,
        },
        guilds,
        access_token: token.access_token,
        expires_at: chrono::Utc::now() + chrono::Duration::hours(SESSION_TTL_HOURS),
    })
}

/// Sends a dashboard-owned request unless the performance harness explicitly
/// forbids outbound network activity.
pub(crate) async fn send_dashboard_http(
    state: &DashboardState,
    request: reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    #[cfg(not(feature = "perf-harness"))]
    let _ = state;

    #[cfg(feature = "perf-harness")]
    if let Some(runtime) = state.perf_runtime.as_ref() {
        runtime.deny_outbound();
        anyhow::bail!("external HTTP is disabled by the dashboard performance harness");
    }

    execute_dashboard_http(request).await
}

pub(crate) async fn execute_dashboard_http(
    request: reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    Ok(request.send().await?)
}

pub(crate) fn build_bot_invite_url(state: &DashboardState, guild_id: u64) -> String {
    let mut url = Url::parse("https://discord.com/oauth2/authorize").expect("valid invite url");
    url.query_pairs_mut()
        .append_pair("client_id", &state.app_info.id)
        .append_pair("scope", "bot applications.commands")
        .append_pair("permissions", &state.config.invite_permissions.to_string())
        .append_pair("guild_id", &guild_id.to_string())
        .append_pair("disable_guild_select", "true");
    url.to_string()
}

pub(crate) fn guild_icon_url(guild: &DashboardGuild) -> Option<String> {
    guild.icon.as_ref().map(|icon| {
        format!(
            "https://cdn.discordapp.com/icons/{}/{}.png?size=128",
            guild.id, icon
        )
    })
}

pub(crate) fn oauth_token_request(state: &DashboardState, code: &str) -> reqwest::RequestBuilder {
    let redirect_uri = format!("{}/auth/discord/callback", state.config.public_base_url);
    state
        .http
        .post(format!("{}/oauth2/token", state.discord_api_base))
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .form(&[
            ("client_id", state.app_info.id.as_str()),
            ("client_secret", state.config.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ])
}
