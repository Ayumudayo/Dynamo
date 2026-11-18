use std::{net::SocketAddr, sync::Arc, time::Instant};

use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, patch, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use dynamo_enablement::{resolve_command_states, resolve_module_states};
use dynamo_module_kit::{CommandCatalog, ModuleCatalog};
use dynamo_observability::{
    CatalogStartupSummary, StartupPhase, StartupReport, StartupStatus, catalog_startup_summary,
    format_preview_kv_list, format_preview_list, init_tracing,
};
use dynamo_ops::{
    COMMAND_SYNC_PROVIDER_ID, CommandSyncResult, CommandSyncScopeState, CommandSyncStateStore,
    DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry, DashboardAuditLogPage,
    DashboardAuditLogQuery, DashboardAuditScope,
};
use dynamo_persistence_api::Persistence;
use dynamo_runtime_api::Error;
use dynamo_settings::{
    DeploymentModuleSettings, DeploymentSettings, GuildModuleSettings, GuildSettings,
};
use serde::Deserialize;
use tracing::{info, warn};

mod assets;
mod auth;
mod discord;
mod font_assets;
mod guild_access;
mod handlers;
mod i18n;
mod owner_auth;
mod render;
mod state;
mod status;

use auth::{
    ReadGuildAuthorizationError, exchange_oauth_code, is_oauth_state_expired, is_session_expired,
    load_session, random_token, refresh_read_session, sanitize_redirect_target,
    session_can_manage_guild, session_cookie,
};
#[cfg(test)]
use auth::{RefreshSessionGuildsError, refresh_session_guilds, user_can_manage_guild};
#[cfg(test)]
use discord::build_dashboard_http_client_with_timeouts;
use discord::{build_dashboard_http_client, build_discord_authorize_url, fetch_application_info};
pub(crate) use font_assets::*;
#[cfg(test)]
use guild_access::{classify_bot_guild_status, sort_guild_cards};
use guild_access::{
    load_guild_card, load_guild_cards, require_api_guild_access, require_current_api_guild_access,
};
pub(crate) use handlers::api::{error_payload, require_api_session};
use handlers::api::{
    get_deployment_settings, get_deployment_state, get_guild_resources, get_guild_settings,
    get_guild_state, list_default_module_states, list_live_module_states, list_modules,
    patch_deployment_command_settings, patch_deployment_module_settings,
    patch_guild_announcement_channel, patch_guild_command_settings, patch_guild_module_settings,
    patch_owner_notices, post_deployment_command_sync, post_guild_command_sync,
    post_owner_announcement, post_owner_guild_block, post_owner_guild_leave,
    post_owner_guild_reset, post_owner_guild_sync, post_owner_sync_stale_guilds,
};
use handlers::pages::{
    deployment_page, deployment_settings_fragment, discord_callback, guild_page,
    guild_settings_fragment, index, lock_owner_console, login, logout, selector, set_language,
    verify_owner_totp,
};
pub(crate) use handlers::pages::{
    page_query_for_logs, page_query_for_owner_logs, page_query_for_tab,
};
use i18n::Locale;
#[cfg(test)]
use render::document::GuildCard;
use render::document::{
    BotGuildPresence, render_error_page, render_install_required_page, render_landing_page,
    render_selector_page,
};
pub(crate) use state::{
    DISCORD_API_BASE, DashboardConfig, DashboardGuild, DashboardPageQuery, DashboardSession,
    DashboardState, DashboardUser, DiscordApplicationInfo, DiscordCallbackQuery, DiscordOAuthUser,
    DiscordTokenResponse, LoginQuery, OAUTH_STATE_TTL_MINUTES, PendingOauthState,
    SESSION_COOKIE_NAME,
};

#[cfg(feature = "perf-harness")]
mod perf_harness;

#[cfg(feature = "perf-harness")]
pub use perf_harness::run_perf_harness;

#[cfg(feature = "perf-harness")]
pub(crate) type DashboardPerfRuntime = perf_harness::PerfRuntime;

#[cfg(feature = "local-preview")]
mod local_preview;

#[cfg(feature = "local-preview")]
pub use local_preview::run_local_preview;

#[cfg(feature = "local-preview")]
pub(crate) type DashboardLocalPreview = local_preview::LocalPreviewRuntime;

pub async fn run_production() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = DashboardConfig::from_env()?;
    info!(
        host = %config.host,
        port = config.port,
        public_base_url = %config.public_base_url,
        "Dashboard startup preflight: loading registry, persistence, and Discord application metadata"
    );
    let registry = dynamo_app::module_registry();
    let module_catalog = registry.catalog().clone();
    let command_catalog = registry.command_catalog().clone();
    let catalog_summary = catalog_startup_summary(&module_catalog, &command_catalog);
    let http = build_dashboard_http_client()?;
    let persistence = dynamo_app::persistence_from_env().await?;
    validate_dashboard_persistence(&config, &module_catalog, &command_catalog, &persistence)?;
    let app_info = fetch_application_info(&http, &config).await?;
    let state = Arc::new(DashboardState {
        config,
        http,
        discord_api_base: DISCORD_API_BASE.to_string(),
        app_info,
        module_catalog,
        command_catalog,
        persistence,
        sessions: Default::default(),
        oauth_states: Default::default(),
        owner_security: Default::default(),
        #[cfg(feature = "perf-harness")]
        perf_runtime: None,
        #[cfg(feature = "local-preview")]
        local_preview: None,
    });

    let app = build_dashboard_router(state.clone());

    let address = SocketAddr::new(state.config.host, state.config.port);
    let listener = tokio::net::TcpListener::bind(address).await?;

    build_dashboard_startup_report(
        &state,
        &catalog_summary,
        address,
        &format!("{}/healthz", state.config.public_base_url),
    )
    .log();
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_dashboard_router(state: Arc<DashboardState>) -> Router {
    build_dashboard_routes()
        .with_state(state)
        .layer(middleware::from_fn(assets::security_headers))
        .layer(middleware::from_fn(log_request))
}

fn build_dashboard_routes() -> Router<Arc<DashboardState>> {
    Router::new()
        .route("/", get(index))
        .route("/login", get(login))
        .route("/auth/discord/callback", get(discord_callback))
        .route("/logout", get(logout))
        .route("/lang/{code}", get(set_language))
        .route("/selector", get(selector))
        .route("/deployment", get(deployment_page))
        .route("/deployment/verify", post(verify_owner_totp))
        .route("/deployment/lock", post(lock_owner_console))
        .route(
            "/deployment/settings/{kind}/{item_id}",
            get(deployment_settings_fragment),
        )
        .route("/guild/{guild_id}", get(guild_page))
        .route(
            "/guild/{guild_id}/settings/{kind}/{item_id}",
            get(guild_settings_fragment),
        )
        .route("/healthz", get(healthz))
        .route("/api/modules", get(list_modules))
        .route(
            "/api/module-states/default",
            get(list_default_module_states),
        )
        .route("/api/module-states/live", get(list_live_module_states))
        .route("/api/deployment-settings", get(get_deployment_settings))
        .route("/api/deployment-state", get(get_deployment_state))
        .route(
            "/api/deployment-settings/{module_id}",
            patch(patch_deployment_module_settings),
        )
        .route(
            "/api/deployment-command-settings/{command_id}",
            patch(patch_deployment_command_settings),
        )
        .route(
            "/api/deployment-command-sync",
            post(post_deployment_command_sync),
        )
        .route(
            "/api/owner/sync-stale-guilds",
            post(post_owner_sync_stale_guilds),
        )
        .route(
            "/api/owner/guilds/{guild_id}/sync",
            post(post_owner_guild_sync),
        )
        .route(
            "/api/owner/guilds/{guild_id}/reset",
            post(post_owner_guild_reset),
        )
        .route(
            "/api/owner/guilds/{guild_id}/leave",
            post(post_owner_guild_leave),
        )
        .route(
            "/api/owner/guilds/{guild_id}/block",
            post(post_owner_guild_block),
        )
        .route("/api/owner/notices", patch(patch_owner_notices))
        .route("/api/owner/announcements", post(post_owner_announcement))
        .route("/api/guild-settings/{guild_id}", get(get_guild_settings))
        .route("/api/guild-state/{guild_id}", get(get_guild_state))
        .route("/api/guild-resources/{guild_id}", get(get_guild_resources))
        .route(
            "/api/guild-announcement-channel/{guild_id}",
            patch(patch_guild_announcement_channel),
        )
        .route(
            "/api/guild-settings/{guild_id}/{module_id}",
            patch(patch_guild_module_settings),
        )
        .route(
            "/api/guild-command-settings/{guild_id}/{command_id}",
            patch(patch_guild_command_settings),
        )
        .route(
            "/api/guild-command-sync/{guild_id}",
            post(post_guild_command_sync),
        )
        .merge(font_asset_router())
        .merge(assets::browser_asset_router())
}

fn validate_dashboard_persistence(
    config: &DashboardConfig,
    module_catalog: &ModuleCatalog,
    command_catalog: &CommandCatalog,
    persistence: &Persistence,
) -> anyhow::Result<()> {
    let persistence_ready = persistence.database_name.is_some()
        && persistence.guild_settings.is_some()
        && persistence.deployment_settings.is_some();
    if persistence_ready {
        return Ok(());
    }

    let catalog_summary = catalog_startup_summary(module_catalog, command_catalog);
    let mut report = StartupReport::new("dashboard");
    report.add_phase(
        StartupPhase::new(
            "config",
            StartupStatus::Ok,
            "Dashboard config resolved but startup cannot continue".to_string(),
        )
        .detail("host", config.host.to_string())
        .detail("port", config.port.to_string())
        .detail("public_base_url", config.public_base_url.clone())
        .detail("callback_url", oauth_callback_url(&config.public_base_url)),
    );
    report.add_phase(
        StartupPhase::new(
            "registry",
            StartupStatus::Ok,
            format!(
                "Discovered {} modules and {} leaf commands",
                catalog_summary.module_count, catalog_summary.discovered_leaf_command_count
            ),
        )
        .detail(
            "module_ids",
            format_preview_list(&catalog_summary.module_ids, 5),
        )
        .detail(
            "per_category_command_counts",
            format_preview_kv_list(&catalog_summary.per_category_command_counts, 5),
        ),
    );
    report.add_phase(
        StartupPhase::new(
            "readiness",
            StartupStatus::Error,
            "Dashboard requires MongoDB persistence and OAuth configuration".to_string(),
        )
        .detail(
            "database",
            persistence
                .database_name
                .clone()
                .unwrap_or_else(|| "none".to_string()),
        )
        .detail(
            "guild_settings_repo",
            persistence.guild_settings.is_some().to_string(),
        )
        .detail(
            "deployment_settings_repo",
            persistence.deployment_settings.is_some().to_string(),
        )
        .detail("session_store_mode", "in-memory"),
    );
    report.log();

    anyhow::bail!(
        "Dashboard requires MongoDB persistence (database + guild/deployment settings repositories) and cannot start in degraded mode"
    );
}

fn build_dashboard_startup_report(
    state: &DashboardState,
    catalog_summary: &CatalogStartupSummary,
    address: SocketAddr,
    health_endpoint: &str,
) -> StartupReport {
    let mut report = StartupReport::new("dashboard");
    report.add_phase(
        StartupPhase::new(
            "config",
            StartupStatus::Ok,
            format!(
                "app={} host={}:{}",
                state.app_info.name, state.config.host, state.config.port
            ),
        )
        .detail("application_id", state.app_info.id.clone())
        .detail("application_name", state.app_info.name.clone())
        .detail("host", state.config.host.to_string())
        .detail("port", state.config.port.to_string())
        .detail("public_base_url", state.config.public_base_url.clone())
        .detail(
            "callback_url",
            oauth_callback_url(&state.config.public_base_url),
        )
        .detail("admin_mode", dashboard_admin_mode_summary(state)),
    );
    report.add_phase(
        StartupPhase::new(
            "registry",
            StartupStatus::Ok,
            format!(
                "modules={} leaf_commands={}",
                catalog_summary.module_count, catalog_summary.discovered_leaf_command_count
            ),
        )
        .detail(
            "module_ids",
            format_preview_list(&catalog_summary.module_ids, 5),
        )
        .detail(
            "leaf_command_count",
            catalog_summary.discovered_leaf_command_count.to_string(),
        )
        .detail(
            "per_category_command_counts",
            format_preview_kv_list(&catalog_summary.per_category_command_counts, 5),
        ),
    );
    report.add_phase(
        StartupPhase::new(
            "readiness",
            StartupStatus::Ok,
            format!(
                "db={} oauth=ready session=in-memory",
                state
                    .persistence
                    .database_name
                    .as_deref()
                    .unwrap_or("unknown")
            ),
        )
        .detail(
            "database",
            state
                .persistence
                .database_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        )
        .detail(
            "oauth_client_secret",
            (!state.config.client_secret.is_empty()).to_string(),
        )
        .detail(
            "callback_url_resolved",
            (!oauth_callback_url(&state.config.public_base_url).is_empty()).to_string(),
        )
        .detail("session_store_mode", "in-memory"),
    );
    report.add_phase(
        StartupPhase::new(
            "listening",
            StartupStatus::Ok,
            format!("url={}", state.config.public_base_url),
        )
        .detail("listening_address", address.to_string())
        .detail(
            "listening_url",
            format!("{}/", state.config.public_base_url),
        )
        .detail("health_endpoint", health_endpoint.to_string()),
    );
    report
}

fn oauth_callback_url(public_base_url: &str) -> String {
    format!(
        "{}/auth/discord/callback",
        public_base_url.trim_end_matches('/')
    )
}

fn dashboard_admin_mode_summary(state: &DashboardState) -> String {
    let totp = if state.config.owner_totp_secret.is_some() {
        "totp-configured"
    } else {
        "totp-missing"
    };
    match state.app_info.owner_user_id {
        Some(owner_id) => format!("application owner ({owner_id}), {totp}"),
        None => format!("application owner unresolved, {totp}"),
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SyncScopeKind {
    Global,
    Guild(u64),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum CommandSyncDisplayState {
    InSync,
    Required,
    Pending,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone)]
pub(crate) enum CommandSyncMeta {
    Time(String, chrono::DateTime<chrono::Utc>),
    Text(String),
}

#[derive(Debug, Clone)]
pub(crate) struct CommandSyncAction {
    pub(crate) label: String,
    pub(crate) url: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CommandSyncPanel {
    state: CommandSyncDisplayState,
    title: String,
    message: String,
    raw_error: Option<String>,
    action: Option<CommandSyncAction>,
    meta: Vec<CommandSyncMeta>,
}

impl CommandSyncPanel {
    /// JSON used by the browser to refresh the panel after a change.
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tone": render::entities::sync_tone(self.state),
            "title": self.title,
            "body": self.message,
        })
    }
}

pub(crate) async fn load_command_sync_store(persistence: &Persistence) -> CommandSyncStateStore {
    persistence
        .load_provider_state(COMMAND_SYNC_PROVIDER_ID)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<CommandSyncStateStore>(value).ok())
        .unwrap_or_default()
}

async fn save_command_sync_store(
    persistence: &Persistence,
    store: &CommandSyncStateStore,
) -> Result<(), Error> {
    persistence
        .save_provider_state(COMMAND_SYNC_PROVIDER_ID, serde_json::to_value(store)?)
        .await
}

pub(crate) fn build_command_sync_panel(
    scope: SyncScopeKind,
    current_fingerprint: &str,
    scope_state: Option<&CommandSyncScopeState>,
    config: &DashboardConfig,
    locale: Locale,
) -> CommandSyncPanel {
    let t = locale.t();
    let scope_state = scope_state.cloned().unwrap_or_default();
    let action = Some(match scope {
        SyncScopeKind::Global => CommandSyncAction {
            label: t.sync_button_global.to_string(),
            url: "/api/deployment-command-sync".to_string(),
        },
        SyncScopeKind::Guild(guild_id) => CommandSyncAction {
            label: t.sync_button_guild.to_string(),
            url: format!("/api/guild-command-sync/{guild_id}"),
        },
    });
    let meta = sync_status_meta(&scope_state, locale);

    let (state, title, message, raw_error) = if scope_state.has_pending_request() {
        (
            CommandSyncDisplayState::Pending,
            t.sync_pending_title,
            i18n::fill(
                t.sync_pending_body,
                &[("seconds", &config.command_sync_interval_seconds.max(5))],
            ),
            None,
        )
    } else if scope_state.last_result == Some(CommandSyncResult::Failed) {
        (
            CommandSyncDisplayState::Failed,
            t.sync_failed_title,
            t.sync_failed_body.to_string(),
            scope_state
                .last_error
                .clone()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty()),
        )
    } else if scope_state.is_in_sync_with(current_fingerprint) {
        (
            CommandSyncDisplayState::InSync,
            t.sync_ok_title,
            t.sync_ok_body.to_string(),
            None,
        )
    } else {
        (
            CommandSyncDisplayState::Required,
            t.sync_required_title,
            t.sync_required_body.to_string(),
            None,
        )
    };

    CommandSyncPanel {
        state,
        title: title.to_string(),
        message,
        raw_error,
        action,
        meta,
    }
}

pub(crate) fn build_unsupported_sync_panel(message: &str, locale: Locale) -> CommandSyncPanel {
    CommandSyncPanel {
        state: CommandSyncDisplayState::Unsupported,
        title: locale.t().sync_unsupported_title.to_string(),
        message: message.to_string(),
        raw_error: None,
        action: None,
        meta: Vec::new(),
    }
}

fn sync_status_meta(scope_state: &CommandSyncScopeState, locale: Locale) -> Vec<CommandSyncMeta> {
    let t = locale.t();
    let mut parts = Vec::new();
    if let Some(synced_at) = scope_state.last_synced_at {
        parts.push(CommandSyncMeta::Time(
            t.sync_last_synced.to_string(),
            synced_at,
        ));
    }
    if let Some(command_count) = scope_state.last_submitted_top_level_commands {
        parts.push(CommandSyncMeta::Text(i18n::fill(
            t.sync_command_count,
            &[("count", &command_count)],
        )));
    }
    if let Some(requested_at) = scope_state.requested_at {
        parts.push(CommandSyncMeta::Time(
            t.sync_requested_at.to_string(),
            requested_at,
        ));
    }
    if let Some(requested_by) = &scope_state.requested_by_username {
        parts.push(CommandSyncMeta::Text(i18n::fill(
            t.sync_requested_by,
            &[("name", requested_by)],
        )));
    }
    parts
}

fn guild_settings_ui_state(settings: &GuildSettings, persisted: bool) -> &'static str {
    if !persisted {
        "absent"
    } else if settings.modules.is_empty() && settings.commands.is_empty() {
        "existing-empty"
    } else {
        "existing-configured"
    }
}

/// Returns the test id suffix and the localized notice for a settings state.
fn guild_settings_notice(state: &str, locale: Locale) -> (&'static str, &'static str) {
    let t = locale.t();
    match state {
        "absent" => ("absent", t.guild_settings_absent),
        "existing-empty" => ("empty", t.guild_settings_empty),
        _ => ("configured", t.guild_settings_configured),
    }
}

fn display_name(user: &DashboardUser) -> String {
    user.global_name
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| user.username.clone())
}

/// Global and per-module operator notices; returns `None` when nothing is active.
pub(crate) fn runtime_notice_text(module_id: &str) -> Option<&'static str> {
    let _ = module_id;
    None
}

fn render_runtime_notices(
    catalog: &ModuleCatalog,
    dynamic: &std::collections::BTreeMap<String, String>,
    locale: Locale,
) -> Option<maud::Markup> {
    let t = locale.t();
    let mut notices: Vec<(String, String)> = Vec::new();
    if let Some(global) = dynamic
        .get("__global")
        .map(String::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        notices.push((t.overview_runtime_global.to_string(), global.to_string()));
    }
    for entry in &catalog.entries {
        if let Some(note) = dynamic
            .get(entry.module.id)
            .map(String::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .or_else(|| runtime_notice_text(entry.module.id))
        {
            notices.push((
                locale.catalog(entry.module.display_name).to_string(),
                note.to_string(),
            ));
        }
    }
    (!notices.is_empty()).then(|| {
        maud::html! {
            section.notice.runtime-notice data-testid="runtime-notices" {
                h3 { (t.overview_runtime_title) }
                ul {
                    @for (name, note) in &notices {
                        li { strong { (name) } ": " (note) }
                    }
                }
            }
        }
    })
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request_path_for_logging(request.uri()).to_string();
    let request_id = request_id_for_logging(request.headers());
    let started = Instant::now();

    let response = next.run(request).await;
    let status = response.status();
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    if !request_path_should_be_logged(&path) {
        return response;
    }

    if let Some(request_id) = request_id {
        info!(
            method = %method,
            path = %path,
            status = status.as_u16(),
            latency_ms,
            request_id = %request_id,
            "dashboard request"
        );
    } else {
        info!(
            method = %method,
            path = %path,
            status = status.as_u16(),
            latency_ms,
            "dashboard request"
        );
    }

    response
}

fn request_path_for_logging(uri: &Uri) -> &str {
    uri.path()
}

fn request_path_should_be_logged(path: &str) -> bool {
    path != "/healthz"
}

fn request_id_for_logging(headers: &HeaderMap) -> Option<String> {
    const REQUEST_ID_HEADERS: [&str; 2] = ["x-request-id", "x-correlation-id"];

    REQUEST_ID_HEADERS.iter().find_map(|name| {
        let value = headers.get(*name)?.to_str().ok()?.trim();
        if value.is_empty() || value.len() > 128 || !value.chars().all(|ch| ch.is_ascii_graphic()) {
            return None;
        }

        Some(value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::{Duration as StdDuration, Instant as StdInstant};

    use super::i18n::Locale;
    use super::render::document::{render_guild_card, render_section_tabs};
    use super::render::entities::{
        audit_action_label, audit_entity_label, command_rows, module_rows,
        render_audit_logs_section, render_entity_section,
    };
    use super::status::{EntityKind, EntityState, Scope};
    use super::{
        BotGuildPresence, DISCORD_API_BASE, DashboardConfig, DashboardGuild, DashboardSession,
        DashboardState, DashboardUser, DiscordApplicationInfo, FIRA_CODE_VARIABLE_BYTES,
        FIRA_CODE_VARIABLE_ETAG, FIRA_CODE_VARIABLE_PATH, FIRA_CODE_VARIABLE_SHA256,
        FIRA_SANS_BOLD_BYTES, FIRA_SANS_BOLD_ETAG, FIRA_SANS_BOLD_PATH, FIRA_SANS_BOLD_SHA256,
        FIRA_SANS_LIGHT_BYTES, FIRA_SANS_LIGHT_ETAG, FIRA_SANS_LIGHT_PATH, FIRA_SANS_LIGHT_SHA256,
        FIRA_SANS_MEDIUM_BYTES, FIRA_SANS_MEDIUM_ETAG, FIRA_SANS_MEDIUM_PATH,
        FIRA_SANS_MEDIUM_SHA256, FIRA_SANS_REGULAR_BYTES, FIRA_SANS_REGULAR_ETAG,
        FIRA_SANS_REGULAR_PATH, FIRA_SANS_REGULAR_SHA256, FIRA_SANS_SEMIBOLD_BYTES,
        FIRA_SANS_SEMIBOLD_ETAG, FIRA_SANS_SEMIBOLD_PATH, FIRA_SANS_SEMIBOLD_SHA256,
        FONT_CACHE_CONTROL, GuildCard, GuildModuleSettings, GuildSettings,
        RefreshSessionGuildsError, SESSION_COOKIE_NAME, build_dashboard_http_client_with_timeouts,
        build_dashboard_router, classify_bot_guild_status, font_asset_router,
        guild_settings_notice, guild_settings_ui_state, refresh_session_guilds, render_error_page,
        render_landing_page, request_id_for_logging, request_path_for_logging,
        request_path_should_be_logged, sanitize_redirect_target, session_can_manage_guild,
        sort_guild_cards, user_can_manage_guild,
    };
    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{HeaderMap, HeaderValue, Request, StatusCode, Uri, header},
    };
    use chrono::{Duration, Utc};
    use dynamo_module_kit::{CommandCatalog, ModuleCatalog, SettingsField, SettingsFieldKind};
    use dynamo_ops::{
        DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry,
        DashboardAuditLogPage, DashboardAuditLogQuery, DashboardAuditLogRepository,
        DashboardAuditScope,
    };
    use dynamo_persistence_api::Persistence;
    use dynamo_repositories::{
        DeploymentSettingsRepository, GuildSettingsRepository, ProviderStateRepository,
    };
    use dynamo_settings::{
        DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings,
        GuildCommandSettings,
    };
    use tower::ServiceExt;

    async fn write_raw_response(stream: &tokio::net::TcpStream, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            stream.writable().await.expect("loopback socket writable");
            match stream.try_write(bytes) {
                Ok(0) => panic!("loopback socket closed before response completed"),
                Ok(written) => bytes = &bytes[written..],
                Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                Err(error) => panic!("failed to write loopback response: {error}"),
            }
        }
    }

    async fn read_raw_request(stream: &tokio::net::TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            stream.readable().await.expect("loopback socket readable");
            match stream.try_read(&mut buffer) {
                Ok(0) => panic!("loopback client closed before request completed"),
                Ok(read) => request.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                Err(error) => panic!("failed to read loopback request: {error}"),
            }
        }
    }

    async fn spawn_delayed_raw_server(delay_headers: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback raw server");
        let address = listener.local_addr().expect("loopback address");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept loopback request");
            read_raw_request(&stream).await;
            if delay_headers {
                tokio::time::sleep(StdDuration::from_millis(350)).await;
            }
            write_raw_response(&stream, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n").await;
            if !delay_headers {
                tokio::time::sleep(StdDuration::from_millis(350)).await;
            }
            write_raw_response(&stream, b"OK").await;
        });
        format!("http://{address}/delayed")
    }

    async fn spawn_stalled_tls_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback TLS stall server");
        let address = listener.local_addr().expect("loopback address");
        tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept loopback connection");
            tokio::time::sleep(StdDuration::from_millis(350)).await;
        });
        format!("https://{address}/stalled-connect")
    }

    async fn spawn_discord_guilds_server(
        status: StatusCode,
        body: &'static str,
        delay: StdDuration,
        request_count: usize,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback Discord server");
        let address = listener.local_addr().expect("loopback Discord address");
        let requests = Arc::new(AtomicUsize::new(0));
        let observed_requests = requests.clone();
        tokio::spawn(async move {
            for _ in 0..request_count {
                let (stream, _) = listener.accept().await.expect("accept Discord request");
                read_raw_request(&stream).await;
                observed_requests.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                let response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Response"),
                    body.len(),
                    body
                );
                write_raw_response(&stream, response.as_bytes()).await;
            }
        });
        (format!("http://{address}"), requests)
    }

    #[tokio::test]
    async fn dashboard_http_delayed_connect_is_bounded() {
        let url = spawn_stalled_tls_server().await;
        let client = build_dashboard_http_client_with_timeouts(
            StdDuration::from_millis(100),
            StdDuration::from_secs(1),
        )
        .expect("dashboard HTTP client");
        let started = StdInstant::now();

        let error = client
            .get(url)
            .send()
            .await
            .expect_err("stalled TLS connection must time out");

        let elapsed = started.elapsed();
        assert!(error.is_timeout(), "expected connect timeout, got {error}");
        assert!(elapsed >= StdDuration::from_millis(80));
        assert!(elapsed < StdDuration::from_secs(1));
    }

    #[tokio::test]
    async fn dashboard_http_delayed_headers_are_bounded() {
        let url = spawn_delayed_raw_server(true).await;
        let client = build_dashboard_http_client_with_timeouts(
            StdDuration::from_millis(100),
            StdDuration::from_millis(100),
        )
        .expect("dashboard HTTP client");
        let started = StdInstant::now();

        let error = client
            .get(url)
            .send()
            .await
            .expect_err("delayed headers must time out");

        let elapsed = started.elapsed();
        assert!(error.is_timeout(), "expected timeout, got {error}");
        assert!(elapsed >= StdDuration::from_millis(80));
        assert!(elapsed < StdDuration::from_secs(1));
    }

    #[tokio::test]
    async fn dashboard_http_delayed_body_is_bounded() {
        let url = spawn_delayed_raw_server(false).await;
        let client = build_dashboard_http_client_with_timeouts(
            StdDuration::from_millis(100),
            StdDuration::from_millis(100),
        )
        .expect("dashboard HTTP client");
        let started = StdInstant::now();
        let response = client.get(url).send().await.expect("response headers");

        let error = response
            .bytes()
            .await
            .expect_err("delayed body must time out");

        let elapsed = started.elapsed();
        assert!(error.is_timeout(), "expected timeout, got {error}");
        assert!(elapsed >= StdDuration::from_millis(80));
        assert!(elapsed < StdDuration::from_secs(1));
    }

    #[test]
    fn bot_guild_presence_only_treats_not_found_as_missing() {
        assert_eq!(
            classify_bot_guild_status(StatusCode::OK),
            BotGuildPresence::Present
        );
        assert_eq!(
            classify_bot_guild_status(StatusCode::NOT_FOUND),
            BotGuildPresence::Missing
        );
        assert_eq!(
            classify_bot_guild_status(StatusCode::TOO_MANY_REQUESTS),
            BotGuildPresence::Unavailable
        );
        assert_eq!(
            classify_bot_guild_status(StatusCode::INTERNAL_SERVER_ERROR),
            BotGuildPresence::Unavailable
        );
    }

    #[test]
    fn unavailable_bot_presence_does_not_render_install_action() {
        let rendered = render_guild_card(
            &GuildCard {
                id: 42,
                name: "Unavailable Guild".to_string(),
                icon_url: None,
                bot_presence: BotGuildPresence::Unavailable,
                manage_url: "/guild/42".to_string(),
                invite_url: "https://discord.com/invite".to_string(),
            },
            Locale::En,
        )
        .into_string();

        assert!(rendered.contains("Unavailable"));
        assert!(!rendered.contains("Not installed"));
        assert!(!rendered.contains("Invite bot"));
        assert!(!rendered.contains("https://discord.com/invite"));
        assert!(rendered.contains("Check again"));
    }

    #[test]
    fn guild_module_state_keeps_local_gate_visible_when_deployment_blocks_effective_state() {
        let registry = dynamo_app::module_registry();
        let module = registry
            .catalog()
            .entries
            .first()
            .expect("module")
            .module
            .clone();
        let mut deployment = DeploymentSettings::default();
        deployment.modules.insert(
            module.id.to_string(),
            DeploymentModuleSettings {
                installed: true,
                enabled: false,
            },
        );
        let mut guild = GuildSettings::for_guild(42);
        guild.modules.insert(
            module.id.to_string(),
            GuildModuleSettings {
                enabled: true,
                configuration: serde_json::json!({}),
            },
        );
        let resolved =
            dynamo_enablement::resolve_module_states(registry.catalog(), &deployment, Some(&guild));
        let rows = module_rows(registry.catalog(), &resolved, Scope::Guild(42), Locale::En);
        let row = rows.iter().find(|row| row.id == module.id).expect("row");
        assert_eq!(row.status.state, EntityState::Blocked);
        assert!(row.status.own_enabled);

        let rendered = render_entity_section(
            EntityKind::Module,
            Scope::Guild(42),
            &rows,
            None,
            Locale::En,
        )
        .into_string();
        let card_start = rendered
            .find(&format!("data-testid=\"module-card-{}\"", module.id))
            .expect("card");
        let card = &rendered[card_start..];
        let card = &card[..card.find("</article>").expect("card end")];
        assert!(card.contains("data-state=\"blocked\""));
        assert!(card.contains(" checked"));
        assert!(card.contains("Blocked: disabled globally"));
        assert!(card.contains("data-on=\"false\""));

        let deployment_rows = module_rows(
            registry.catalog(),
            &dynamo_enablement::resolve_module_states(registry.catalog(), &deployment, None),
            Scope::Deployment,
            Locale::En,
        );
        let deployment_row = deployment_rows
            .iter()
            .find(|row| row.id == module.id)
            .expect("deployment row");
        assert_eq!(deployment_row.status.state, EntityState::Off);
        assert!(!deployment_row.status.own_enabled);
    }

    #[test]
    fn guild_settings_ui_state_distinguishes_absent_empty_and_configured() {
        let empty = GuildSettings::for_guild(42);
        assert_eq!(guild_settings_ui_state(&empty, false), "absent");
        assert_eq!(guild_settings_ui_state(&empty, true), "existing-empty");
        assert_eq!(guild_settings_notice("absent", Locale::En).0, "absent");
        assert_eq!(
            guild_settings_notice("existing-empty", Locale::En).0,
            "empty"
        );

        let mut configured = empty;
        configured.modules.insert(
            "stock".to_string(),
            GuildModuleSettings {
                enabled: true,
                configuration: serde_json::json!({}),
            },
        );
        assert_eq!(
            guild_settings_ui_state(&configured, true),
            "existing-configured"
        );
        let (testid, text) = guild_settings_notice("existing-configured", Locale::Ko);
        assert_eq!(testid, "configured");
        assert_eq!(text, "서버별 재정의가 적용 중입니다.");
    }

    #[test]
    fn dashboard_error_page_offers_retry_without_echoing_internal_details() {
        let state = test_dashboard_state(Persistence::default());
        let rendered = render_error_page(
            &state,
            None,
            Locale::En,
            "Settings Unavailable",
            "Settings could not be loaded. <script>",
        );

        assert!(rendered.contains(">Try again</a>"));
        assert!(rendered.contains("role=\"alert\""));
        assert!(rendered.contains("&lt;script&gt;"));
        assert!(!rendered.contains("mongodb://"));
        assert!(!rendered.contains("test-secret"));
    }

    #[test]
    fn document_shell_is_localized_accessible_and_csp_ready() {
        let state = test_dashboard_state(Persistence::default());
        let english = render_landing_page(&state, None, Locale::En);
        assert!(english.starts_with("<!DOCTYPE html><html lang=\"en\" class=\"no-js\">"));
        assert!(english.contains("<meta name=\"color-scheme\" content=\"dark\">"));
        assert!(english.contains("<meta name=\"description\""));
        assert!(english.contains("class=\"skip-link\" href=\"#main-content\""));
        assert!(english.contains("<main ") && english.contains("id=\"main-content\""));
        assert!(english.contains(super::assets::DASHBOARD_CSS_PATH));
        assert!(english.contains(super::assets::DASHBOARD_JS_PATH));
        assert!(english.contains("id=\"dynamo-i18n\" type=\"application/json\""));
        assert!(english.contains("rel=\"icon\" href=\"data:image/svg+xml"));
        // No inline handlers or inline styles: the CSP forbids both.
        for forbidden in [
            " onclick=",
            " onchange=",
            " oninput=",
            " onsubmit=",
            " style=",
        ] {
            assert!(!english.contains(forbidden), "found {forbidden}");
        }
        // The only executable inline script is the hashed boot script.
        assert_eq!(english.matches("<script>").count(), 1);
        assert!(english.contains(&format!("<script>{}</script>", super::assets::BOOT_SCRIPT)));
        // Developer-only copy and the health check link are not shown to visitors.
        assert!(!english.contains("Dyno-like"));
        assert!(!english.contains("href=\"/healthz\""));

        let korean = render_landing_page(&state, None, Locale::Ko);
        assert!(korean.contains("<html lang=\"ko\""));
        assert!(korean.contains("본문으로 건너뛰기"));
        assert!(korean.contains("Discord로 로그인"));
    }

    #[test]
    fn browser_assets_cover_focus_dark_scheme_and_reduced_motion() {
        let css = super::assets::DASHBOARD_CSS;
        assert!(css.contains("color-scheme: dark;"));
        assert!(css.contains(":focus-visible"));
        assert!(css.contains(".switch input:focus-visible + .switch-track"));
        assert!(css.contains("prefers-reduced-motion: reduce"));
        assert!(css.contains("[hidden] { display: none !important; }"));
        assert!(!css.contains("var(--muted-soft)"));
        assert!(css.contains(".notice {"));
        // Generic element selectors must not be styled as panels.
        assert!(!css.contains("section, article, details {"));
        assert!(!css.contains(".summary-card:hover"));

        let js = super::assets::DASHBOARD_JS;
        assert!(js.contains("DynamoMutationTransport"));
        assert!(js.contains("document.addEventListener('click'"));
        assert!(!js.contains("eval("));
        assert!(!js.contains("innerHTML = message"));
    }

    #[test]
    fn filters_and_navigation_expose_current_semantic_state() {
        let tabs = render_section_tabs(Locale::En, "/guild/42", "modules").into_string();
        assert!(tabs.contains(
            "data-testid=\"page-tab-modules\" href=\"/guild/42?tab=modules\" aria-current=\"page\""
        ));
        assert_eq!(tabs.matches("aria-current").count(), 1);

        let registry = dynamo_app::module_registry();
        let deployment = DeploymentSettings::default();
        let rows = command_rows(
            registry.command_catalog(),
            &dynamo_enablement::resolve_command_states(
                registry.catalog(),
                registry.command_catalog(),
                &deployment,
                None,
            ),
            Scope::Deployment,
            Locale::En,
        );
        let rendered = render_entity_section(
            EntityKind::Command,
            Scope::Deployment,
            &rows,
            None,
            Locale::En,
        )
        .into_string();
        assert!(rendered.contains("data-testid=\"command-filter\""));
        assert!(rendered.contains("data-testid=\"command-tab-all\""));
        assert!(rendered.contains("data-category-filter=\"all\" aria-pressed=\"true\""));
        assert!(rendered.contains("data-state-filter=\"blocked\" aria-pressed=\"false\""));
        assert!(rendered.contains("role=\"status\" aria-live=\"polite\""));
        // Search covers ids and descriptions, not just display names.
        assert!(rendered.contains("data-search=\"etf /etf"));
    }

    #[test]
    fn sidebar_has_one_active_destination_and_no_duplicate_section_menu() {
        let mut state = test_dashboard_state(Persistence::default());
        Arc::get_mut(&mut state)
            .expect("test dashboard state is uniquely owned")
            .app_info
            .owner_user_id = Some(7);
        let session = DashboardSession {
            user: DashboardUser {
                id: 7,
                username: "tester".to_string(),
                global_name: Some("Tester".to_string()),
                avatar: None,
            },
            guilds: vec![DashboardGuild {
                id: 42,
                name: "Guild".to_string(),
                icon: None,
                permissions: "32".to_string(),
            }],
            access_token: "access-token".to_string(),
            expires_at: Utc::now() + Duration::minutes(5),
            csrf_token: "csrf-test".to_string(),
        };

        let selector = super::render_selector_page(&state, &session, Locale::En, &[]);
        assert_eq!(selector.matches("nav-link active").count(), 1);
        assert!(selector.contains(
            "class=\"nav-link active\" href=\"/selector\" aria-current=\"page\">Servers"
        ));
        assert!(selector.contains("href=\"/deployment\">App owner"));
        assert!(!selector.contains("nav-sub-link"));
        assert!(selector.contains("aria-label=\"Breadcrumb\""));
        // /healthz is no longer exposed in sidebar; the page title appears once as <h1>.
        assert!(!selector.contains("href=\"/healthz\""));
        assert_eq!(selector.matches("<h1").count(), 1);

        let dashboard = render_landing_page(&state, Some(&session), Locale::En);
        assert!(
            dashboard
                .contains("class=\"button button-primary\" href=\"/selector\">Choose a server")
        );
        assert!(!dashboard.contains("Sign in with Discord"));
        assert!(dashboard.contains("href=\"/logout\""));
    }

    #[test]
    fn guild_cards_use_stable_name_then_id_order() {
        let mut cards = vec![
            GuildCard {
                id: 20,
                name: "beta".to_string(),
                icon_url: None,
                bot_presence: BotGuildPresence::Missing,
                manage_url: "/guild/20".to_string(),
                invite_url: "https://discord.com/invite/20".to_string(),
            },
            GuildCard {
                id: 3,
                name: "Alpha".to_string(),
                icon_url: None,
                bot_presence: BotGuildPresence::Missing,
                manage_url: "/guild/3".to_string(),
                invite_url: "https://discord.com/invite/3".to_string(),
            },
            GuildCard {
                id: 4,
                name: "alpha".to_string(),
                icon_url: None,
                bot_presence: BotGuildPresence::Missing,
                manage_url: "/guild/4".to_string(),
                invite_url: "https://discord.com/invite/4".to_string(),
            },
        ];

        sort_guild_cards(&mut cards);

        assert_eq!(
            cards.iter().map(|card| card.id).collect::<Vec<_>>(),
            vec![3, 4, 20]
        );
    }

    struct UnavailableDeploymentSettingsRepository;

    #[async_trait]
    impl DeploymentSettingsRepository for UnavailableDeploymentSettingsRepository {
        async fn get(&self) -> anyhow::Result<DeploymentSettings> {
            anyhow::bail!("mongodb://secret@host/dynamo collection=deployment_settings")
        }

        async fn upsert_module_settings(
            &self,
            _module_id: &str,
            _settings: DeploymentModuleSettings,
        ) -> anyhow::Result<DeploymentSettings> {
            unreachable!("read-only fixture")
        }

        async fn upsert_command_settings(
            &self,
            _command_id: &str,
            _settings: DeploymentCommandSettings,
        ) -> anyhow::Result<DeploymentSettings> {
            unreachable!("read-only fixture")
        }

        async fn upsert_operational_settings(
            &self,
            _blocked_guild_ids: Vec<u64>,
            _module_notices: std::collections::BTreeMap<String, String>,
        ) -> anyhow::Result<DeploymentSettings> {
            unreachable!("read-only fixture")
        }
    }

    enum GuildSettingsReadResult {
        Absent,
        Existing(GuildSettings),
        Unavailable,
    }

    struct FakeGuildSettingsRepository {
        result: GuildSettingsReadResult,
        reads: AtomicUsize,
        writes: AtomicUsize,
    }

    impl FakeGuildSettingsRepository {
        fn new(result: GuildSettingsReadResult) -> Self {
            Self {
                result,
                reads: AtomicUsize::new(0),
                writes: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl GuildSettingsRepository for FakeGuildSettingsRepository {
        async fn get(&self, _guild_id: u64) -> anyhow::Result<Option<GuildSettings>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match &self.result {
                GuildSettingsReadResult::Absent => Ok(None),
                GuildSettingsReadResult::Existing(settings) => Ok(Some(settings.clone())),
                GuildSettingsReadResult::Unavailable => {
                    anyhow::bail!(
                        "mongodb://secret@host/dynamo collection=guild_settings unavailable"
                    )
                }
            }
        }

        async fn upsert_module_settings(
            &self,
            guild_id: u64,
            module_id: &str,
            settings: GuildModuleSettings,
        ) -> anyhow::Result<GuildSettings> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let mut stored = GuildSettings::for_guild(guild_id);
            stored.modules.insert(module_id.to_string(), settings);
            Ok(stored)
        }

        async fn upsert_command_settings(
            &self,
            guild_id: u64,
            command_id: &str,
            settings: GuildCommandSettings,
        ) -> anyhow::Result<GuildSettings> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let mut stored = GuildSettings::for_guild(guild_id);
            stored.commands.insert(command_id.to_string(), settings);
            Ok(stored)
        }

        async fn list_all(&self) -> anyhow::Result<Vec<GuildSettings>> {
            Ok(Vec::new())
        }

        async fn upsert_announcement_channel(
            &self,
            guild_id: u64,
            channel_id: Option<u64>,
        ) -> anyhow::Result<GuildSettings> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let mut stored = GuildSettings::for_guild(guild_id);
            stored.announcement_channel_id = channel_id;
            Ok(stored)
        }

        async fn reset_guild(&self, _guild_id: u64) -> anyhow::Result<()> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Default)]
    struct CountingProviderStateRepository {
        loads: AtomicUsize,
        saves: AtomicUsize,
    }

    #[async_trait]
    impl ProviderStateRepository for CountingProviderStateRepository {
        async fn load_json(&self, _provider_id: &str) -> anyhow::Result<Option<serde_json::Value>> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        async fn save_json(
            &self,
            _provider_id: &str,
            _value: serde_json::Value,
        ) -> anyhow::Result<()> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Default)]
    struct CountingAuditLogRepository {
        appends: AtomicUsize,
    }

    #[async_trait]
    impl DashboardAuditLogRepository for CountingAuditLogRepository {
        async fn append(
            &self,
            entry: DashboardAuditLogEntry,
        ) -> anyhow::Result<DashboardAuditLogEntry> {
            self.appends.fetch_add(1, Ordering::SeqCst);
            Ok(entry)
        }

        async fn list(
            &self,
            query: DashboardAuditLogQuery,
        ) -> anyhow::Result<DashboardAuditLogPage> {
            Ok(DashboardAuditLogPage::empty(query.page, query.page_size))
        }
    }

    fn test_dashboard_state(persistence: Persistence) -> Arc<DashboardState> {
        test_dashboard_state_with_discord(
            persistence,
            "http://127.0.0.1:9".to_string(),
            StdDuration::from_millis(100),
        )
    }

    fn test_dashboard_state_with_discord(
        persistence: Persistence,
        discord_api_base: String,
        request_timeout: StdDuration,
    ) -> Arc<DashboardState> {
        let registry = dynamo_app::module_registry();
        Arc::new(DashboardState {
            config: DashboardConfig {
                host: "127.0.0.1".parse().expect("loopback address"),
                port: 3000,
                public_base_url: "http://127.0.0.1:3000".to_string(),
                bot_token: "test-token".to_string(),
                client_secret: "test-secret".to_string(),
                invite_permissions: 0,
                owner_totp_secret: None,
                register_globally: false,
                command_sync_interval_seconds: 15,
            },
            http: build_dashboard_http_client_with_timeouts(
                StdDuration::from_millis(100),
                request_timeout,
            )
            .expect("test dashboard HTTP client"),
            discord_api_base,
            app_info: DiscordApplicationInfo {
                id: "test-app".to_string(),
                name: "Test App".to_string(),
                icon: None,
                owner_user_id: None,
            },
            module_catalog: registry.catalog().clone(),
            command_catalog: registry.command_catalog().clone(),
            persistence,
            sessions: Default::default(),
            oauth_states: Default::default(),
            owner_security: Default::default(),
            #[cfg(feature = "perf-harness")]
            perf_runtime: None,
            #[cfg(feature = "local-preview")]
            local_preview: None,
        })
    }

    async fn insert_session(state: &Arc<DashboardState>, session_id: &str, guild_id: u64) {
        state.sessions.write().await.insert(
            session_id.to_string(),
            DashboardSession {
                user: DashboardUser {
                    id: 7,
                    username: "tester".to_string(),
                    global_name: Some("Tester".to_string()),
                    avatar: None,
                },
                guilds: vec![DashboardGuild {
                    id: guild_id,
                    name: "Guild".to_string(),
                    icon: None,
                    permissions: (1u64 << 5).to_string(),
                }],
                access_token: "test-access-token".to_string(),
                expires_at: Utc::now() + Duration::minutes(5),
                csrf_token: format!("csrf-{session_id}"),
            },
        );
    }

    fn authenticated_request(method: &str, path: &str, session_id: &str) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(path).header(
            header::COOKIE,
            format!("{SESSION_COOKIE_NAME}={session_id}"),
        );
        if method != "GET" && method != "HEAD" {
            builder = builder.header("x-csrf-token", format!("csrf-{session_id}"));
        }
        builder
            .body(Body::empty())
            .expect("valid authenticated request")
    }

    fn authenticated_json_request(
        method: &str,
        path: &str,
        session_id: &str,
        body: serde_json::Value,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(
                header::COOKIE,
                format!("{SESSION_COOKIE_NAME}={session_id}"),
            )
            .header(header::CONTENT_TYPE, "application/json");
        if method != "GET" && method != "HEAD" {
            builder = builder.header("x-csrf-token", format!("csrf-{session_id}"));
        }
        builder
            .body(Body::from(body.to_string()))
            .expect("valid authenticated JSON request")
    }

    fn first_module_id(state: &DashboardState) -> String {
        state
            .module_catalog
            .entries
            .first()
            .expect("test module catalog")
            .module
            .id
            .to_string()
    }

    fn first_command_id(state: &DashboardState) -> String {
        state
            .command_catalog
            .entries
            .first()
            .expect("test command catalog")
            .command
            .id
            .clone()
    }

    async fn json_response(response: axum::response::Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded response body");
        serde_json::from_slice(&body).expect("JSON response body")
    }

    #[tokio::test]
    async fn live_module_state_failure_is_unavailable_and_redacted() {
        let state = test_dashboard_state(Persistence {
            deployment_settings: Some(Arc::new(UnavailableDeploymentSettingsRepository)),
            ..Persistence::default()
        });
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_request(
                "GET",
                "/api/module-states/live",
                "test-session",
            ))
            .await
            .expect("live module states response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = json_response(response).await;
        assert_eq!(body["message"], "deployment settings are unavailable");
        let rendered = body.to_string();
        assert!(!rendered.contains("mongodb://"));
        assert!(!rendered.contains("deployment_settings"));
        assert!(!rendered.contains("secret"));
    }

    #[tokio::test]
    async fn guild_settings_api_returns_absent_state_without_writing() {
        let repository = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Absent,
        ));
        let state = test_dashboard_state(Persistence {
            guild_settings: Some(repository.clone()),
            ..Persistence::default()
        });
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_request(
                "GET",
                "/api/guild-settings/42",
                "test-session",
            ))
            .await
            .expect("guild settings response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-dynamo-settings-state"], "absent");
        let body = json_response(response).await;
        assert_eq!(body["guild_id"], 42);
        assert_eq!(repository.reads.load(Ordering::SeqCst), 1);
        assert_eq!(repository.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn guild_settings_api_returns_existing_state_without_writing() {
        let mut settings = GuildSettings::for_guild(42);
        settings.modules.insert(
            "ticket".to_string(),
            GuildModuleSettings {
                enabled: true,
                configuration: serde_json::json!({ "panel_channel_id": "123" }),
            },
        );
        let repository = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Existing(settings),
        ));
        let state = test_dashboard_state(Persistence {
            guild_settings: Some(repository.clone()),
            ..Persistence::default()
        });
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_request(
                "GET",
                "/api/guild-settings/42",
                "test-session",
            ))
            .await
            .expect("guild settings response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-dynamo-settings-state"], "existing");
        let body = json_response(response).await;
        assert_eq!(body["guild_id"], 42);
        assert_eq!(body["modules"]["ticket"]["enabled"], true);
        assert_eq!(
            body["modules"]["ticket"]["configuration"]["panel_channel_id"],
            "123"
        );
        assert_eq!(repository.reads.load(Ordering::SeqCst), 1);
        assert_eq!(repository.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn guild_settings_api_returns_service_unavailable_when_read_fails() {
        let repository = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Unavailable,
        ));
        let state = test_dashboard_state(Persistence {
            guild_settings: Some(repository.clone()),
            ..Persistence::default()
        });
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_request(
                "GET",
                "/api/guild-settings/42",
                "test-session",
            ))
            .await
            .expect("guild settings response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get("x-dynamo-settings-state").is_none());
        let body = json_response(response).await;
        assert_eq!(body["message"], "guild settings are unavailable");
        assert_eq!(repository.reads.load(Ordering::SeqCst), 1);
        assert_eq!(repository.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn guild_page_unavailable_settings_is_actionable_and_redacted() {
        let (discord_api_base, _) = spawn_discord_guilds_server(
            StatusCode::OK,
            r#"[{"id":"42","name":"Guild","icon":null,"permissions":"32"}]"#,
            StdDuration::ZERO,
            2,
        )
        .await;
        let repository = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Unavailable,
        ));
        let state = test_dashboard_state_with_discord(
            Persistence {
                guild_settings: Some(repository),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_request("GET", "/guild/42", "test-session"))
            .await
            .expect("guild page response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded guild page body");
        let rendered = String::from_utf8(body.to_vec()).expect("UTF-8 guild page");
        assert!(rendered.contains("Server settings unavailable"));
        assert!(rendered.contains(">Try again</a>"));
        assert!(!rendered.contains("mongodb://"));
        assert!(!rendered.contains("guild_settings"));
        assert!(!rendered.contains("test-secret"));
    }

    #[tokio::test]
    async fn selector_uses_cached_guilds_but_detail_rechecks_revoked_access() {
        let guilds = r#"[{"id":"99","name":"New Guild","icon":null,"permissions":"32"}]"#;
        let (discord_api_base, requests) =
            spawn_discord_guilds_server(StatusCode::OK, guilds, StdDuration::ZERO, 2).await;
        let state = test_dashboard_state_with_discord(
            Persistence::default(),
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state.clone());

        let selector = app
            .clone()
            .oneshot(authenticated_request("GET", "/selector", "test-session"))
            .await
            .expect("selector response");
        assert_eq!(selector.status(), StatusCode::OK);
        let selector_body = to_bytes(selector.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded selector body");
        let rendered = String::from_utf8(selector_body.to_vec()).expect("UTF-8 selector page");
        assert!(rendered.contains("Guild"));
        assert!(!rendered.contains("New Guild"));

        let denied = app
            .oneshot(authenticated_request("GET", "/guild/42", "test-session"))
            .await
            .expect("guild denial response");
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(session_can_manage_guild(
            &state.sessions.read().await["test-session"],
            99
        ));
        assert!(!session_can_manage_guild(
            &state.sessions.read().await["test-session"],
            42
        ));
    }

    #[tokio::test]
    async fn read_guild_authorization_unavailable_fails_closed() {
        let (discord_api_base, _) = spawn_discord_guilds_server(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"message":"unavailable"}"#,
            StdDuration::ZERO,
            2,
        )
        .await;
        let state = test_dashboard_state_with_discord(
            Persistence::default(),
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let selector = app
            .clone()
            .oneshot(authenticated_request("GET", "/selector", "test-session"))
            .await
            .expect("selector unavailable response");
        assert_eq!(selector.status(), StatusCode::OK);
        let guild = app
            .oneshot(authenticated_request("GET", "/guild/42", "test-session"))
            .await
            .expect("guild unavailable response");
        assert_eq!(guild.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn invalid_guild_authorization_fails_closed_on_detail_page() {
        let (discord_api_base, _) = spawn_discord_guilds_server(
            StatusCode::OK,
            r#"{"unexpected":"response"}"#,
            StdDuration::ZERO,
            1,
        )
        .await;
        let state = test_dashboard_state_with_discord(
            Persistence::default(),
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_request("GET", "/guild/42", "test-session"))
            .await
            .expect("guild detail invalid authorization response");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn refresh_does_not_overwrite_replaced_session_token() {
        let guilds = r#"[{"id":"99","name":"New Guild","icon":null,"permissions":"32"}]"#;
        let (discord_api_base, requests) =
            spawn_discord_guilds_server(StatusCode::OK, guilds, StdDuration::from_millis(150), 1)
                .await;
        let state = test_dashboard_state_with_discord(
            Persistence::default(),
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let refresh_state = state.clone();
        let refresh =
            tokio::spawn(
                async move { refresh_session_guilds(&refresh_state, "test-session").await },
            );
        tokio::time::timeout(StdDuration::from_millis(500), async {
            while requests.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("refresh request reached Discord fixture");
        {
            let mut sessions = state.sessions.write().await;
            let replacement = sessions
                .get_mut("test-session")
                .expect("existing test session");
            replacement.access_token = "replacement-access-token".to_string();
        }

        assert!(
            refresh
                .await
                .expect("refresh task")
                .expect("refresh result")
                .is_none()
        );
        let current = state.sessions.read().await["test-session"].clone();
        assert_eq!(current.access_token, "replacement-access-token");
        assert!(session_can_manage_guild(&current, 42));
        assert!(!session_can_manage_guild(&current, 99));
    }

    #[tokio::test]
    async fn refresh_401_does_not_remove_replaced_session_token() {
        let (discord_api_base, requests) = spawn_discord_guilds_server(
            StatusCode::UNAUTHORIZED,
            r#"{"message":"401: Unauthorized"}"#,
            StdDuration::from_millis(150),
            1,
        )
        .await;
        let state = test_dashboard_state_with_discord(
            Persistence::default(),
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let refresh_state = state.clone();
        let refresh =
            tokio::spawn(
                async move { refresh_session_guilds(&refresh_state, "test-session").await },
            );
        tokio::time::timeout(StdDuration::from_millis(500), async {
            while requests.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("refresh request reached Discord fixture");
        state
            .sessions
            .write()
            .await
            .get_mut("test-session")
            .expect("test session")
            .access_token = "replacement-access-token".to_string();

        assert!(matches!(
            refresh.await.expect("refresh task"),
            Err(RefreshSessionGuildsError::Unauthorized)
        ));
        assert_eq!(
            state.sessions.read().await["test-session"].access_token,
            "replacement-access-token"
        );
    }

    #[tokio::test]
    async fn deployment_page_non_admin_is_forbidden() {
        let state = test_dashboard_state(Persistence::default());
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_request("GET", "/deployment", "test-session"))
            .await
            .expect("deployment response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    fn elevated_request(
        method: &str,
        path: &str,
        session_id: &str,
        elevation_token: &str,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(path).header(
            header::COOKIE,
            format!(
                "{SESSION_COOKIE_NAME}={session_id}; {}={elevation_token}",
                crate::owner_auth::OWNER_ELEVATION_COOKIE_NAME
            ),
        );
        if method != "GET" && method != "HEAD" {
            builder = builder.header("x-csrf-token", format!("csrf-{session_id}"));
        }
        builder.body(Body::empty()).expect("valid elevated request")
    }

    async fn insert_owner_elevation(
        state: &Arc<DashboardState>,
        elevation_token: &str,
        session_id: &str,
        user_id: u64,
    ) {
        let now = Utc::now();
        state.owner_security.write().await.elevations.insert(
            elevation_token.to_string(),
            crate::owner_auth::OwnerElevation {
                user_id,
                session_csrf_token: format!("csrf-{session_id}"),
                elevated_at: now,
                last_seen_at: now,
                last_identity_verified_at: now,
            },
        );
    }

    #[tokio::test]
    async fn deployment_tabs_render_only_active_controls_and_read_command_sync_on_commands() {
        let provider = Arc::new(CountingProviderStateRepository::default());
        let mut state = test_dashboard_state(Persistence {
            provider_state: Some(provider.clone()),
            ..Persistence::default()
        });
        {
            let inner = Arc::get_mut(&mut state).expect("test dashboard state is uniquely owned");
            inner.app_info.owner_user_id = Some(7);
            inner.config.register_globally = true;
            inner.config.owner_totp_secret = Some(
                crate::owner_auth::parse_totp_secret(crate::owner_auth::DEMO_TOTP_SECRET_BASE32)
                    .expect("demo totp secret"),
            );
        }
        insert_session(&state, "test-session", 42).await;
        insert_owner_elevation(&state, "test-elevation", "test-session", 7).await;
        let app = build_dashboard_router(state);

        for (tab, expected_loads) in [("logs", 0), ("overview", 1)] {
            let response = app
                .clone()
                .oneshot(elevated_request(
                    "GET",
                    &format!("/deployment?tab={tab}"),
                    "test-session",
                    "test-elevation",
                ))
                .await
                .expect("deployment tab response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .expect("bounded deployment tab body");
            let rendered = String::from_utf8(body.to_vec()).expect("UTF-8 deployment tab");
            assert!(!rendered.contains("data-action=\"toggle-entity\""));
            assert!(!rendered.contains("data-testid=\"settings-modal-"));
            assert!(!rendered.contains("data-testid=\"deployment-commands-section\""));
            assert_eq!(provider.loads.load(Ordering::SeqCst), expected_loads);
            if tab == "overview" {
                assert!(rendered.contains("data-testid=\"deployment-overview-section\""));
                assert!(rendered.contains("data-testid=\"command-sync-panel\""));
            }
        }

        let response = app
            .clone()
            .oneshot(elevated_request(
                "GET",
                "/deployment?tab=modules",
                "test-session",
                "test-elevation",
            ))
            .await
            .expect("deployment modules response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded deployment modules body");
        let rendered = String::from_utf8(body.to_vec()).expect("UTF-8 deployment modules");
        assert!(rendered.contains("data-testid=\"deployment-modules-section\""));
        assert!(rendered.contains("data-action=\"toggle-entity\""));
        assert!(rendered.contains("data-fragment-url=\"/deployment/settings/module/"));
        // Dialogs are loaded on demand rather than rendered for every item.
        assert!(!rendered.contains("data-testid=\"settings-modal-"));
        assert!(!rendered.contains("data-testid=\"deployment-commands-section\""));
        assert_eq!(provider.loads.load(Ordering::SeqCst), 1);

        let response = app
            .oneshot(elevated_request(
                "GET",
                "/deployment?tab=commands",
                "test-session",
                "test-elevation",
            ))
            .await
            .expect("deployment commands response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded deployment commands body");
        let rendered = String::from_utf8(body.to_vec()).expect("UTF-8 deployment commands");
        assert!(rendered.contains("data-testid=\"deployment-commands-section\""));
        assert!(rendered.contains("data-testid=\"command-sync-panel\""));
        assert!(rendered.contains("data-sync-url=\"/api/deployment-command-sync\""));
        assert!(!rendered.contains("data-testid=\"deployment-modules-section\""));
        assert_eq!(provider.loads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stale_cached_grant_cannot_patch_guild_module() {
        let (discord_api_base, requests) =
            spawn_discord_guilds_server(StatusCode::OK, "[]", StdDuration::ZERO, 1).await;
        let settings = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Absent,
        ));
        let audit = Arc::new(CountingAuditLogRepository::default());
        let state = test_dashboard_state_with_discord(
            Persistence {
                guild_settings: Some(settings.clone()),
                dashboard_audit_logs: Some(audit.clone()),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "stale-session", 42).await;
        let module_id = first_module_id(&state);
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_json_request(
                "PATCH",
                &format!("/api/guild-settings/42/{module_id}"),
                "stale-session",
                serde_json::json!({ "enabled": true }),
            ))
            .await
            .expect("guild module patch response");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(settings.writes.load(Ordering::SeqCst), 0);
        assert_eq!(audit.appends.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stale_cached_grant_cannot_patch_guild_command() {
        let (discord_api_base, requests) =
            spawn_discord_guilds_server(StatusCode::OK, "[]", StdDuration::ZERO, 1).await;
        let settings = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Absent,
        ));
        let audit = Arc::new(CountingAuditLogRepository::default());
        let state = test_dashboard_state_with_discord(
            Persistence {
                guild_settings: Some(settings.clone()),
                dashboard_audit_logs: Some(audit.clone()),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "stale-session", 42).await;
        let command_id = first_command_id(&state);
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_json_request(
                "PATCH",
                &format!("/api/guild-command-settings/42/{command_id}"),
                "stale-session",
                serde_json::json!({ "enabled": true }),
            ))
            .await
            .expect("guild command patch response");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(settings.writes.load(Ordering::SeqCst), 0);
        assert_eq!(audit.appends.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stale_cached_grant_cannot_request_guild_command_sync() {
        let (discord_api_base, requests) =
            spawn_discord_guilds_server(StatusCode::OK, "[]", StdDuration::ZERO, 1).await;
        let provider = Arc::new(CountingProviderStateRepository::default());
        let state = test_dashboard_state_with_discord(
            Persistence {
                provider_state: Some(provider.clone()),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "stale-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_request(
                "POST",
                "/api/guild-command-sync/42",
                "stale-session",
            ))
            .await
            .expect("guild command sync response");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(provider.saves.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn current_authorization_401_removes_session_without_writing() {
        let (discord_api_base, _) = spawn_discord_guilds_server(
            StatusCode::UNAUTHORIZED,
            r#"{"message":"401: Unauthorized"}"#,
            StdDuration::ZERO,
            1,
        )
        .await;
        let settings = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Absent,
        ));
        let state = test_dashboard_state_with_discord(
            Persistence {
                guild_settings: Some(settings.clone()),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "expired-upstream", 42).await;
        let module_id = first_module_id(&state);
        let app = build_dashboard_router(state.clone());

        let response = app
            .oneshot(authenticated_json_request(
                "PATCH",
                &format!("/api/guild-settings/42/{module_id}"),
                "expired-upstream",
                serde_json::json!({ "enabled": true }),
            ))
            .await
            .expect("guild module patch response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!state.sessions.read().await.contains_key("expired-upstream"));
        assert_eq!(settings.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn current_authorization_unavailable_fails_closed_without_writing() {
        let (discord_api_base, _) = spawn_discord_guilds_server(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"message":"unavailable"}"#,
            StdDuration::ZERO,
            1,
        )
        .await;
        let settings = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Absent,
        ));
        let state = test_dashboard_state_with_discord(
            Persistence {
                guild_settings: Some(settings.clone()),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let command_id = first_command_id(&state);
        let app = build_dashboard_router(state);

        let response = app
            .oneshot(authenticated_json_request(
                "PATCH",
                &format!("/api/guild-command-settings/42/{command_id}"),
                "test-session",
                serde_json::json!({ "enabled": true }),
            ))
            .await
            .expect("guild command patch response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(settings.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn current_authorization_timeout_does_not_hold_session_write_lock_or_save() {
        let (discord_api_base, requests) =
            spawn_discord_guilds_server(StatusCode::OK, "[]", StdDuration::from_millis(350), 1)
                .await;
        let provider = Arc::new(CountingProviderStateRepository::default());
        let state = test_dashboard_state_with_discord(
            Persistence {
                provider_state: Some(provider.clone()),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_millis(150),
        );
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state.clone());
        let request = tokio::spawn(async move {
            app.oneshot(authenticated_request(
                "POST",
                "/api/guild-command-sync/42",
                "test-session",
            ))
            .await
            .expect("guild command sync response")
        });

        tokio::time::timeout(StdDuration::from_millis(500), async {
            while requests.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("authorization request reached Discord fixture");
        let lock = tokio::time::timeout(StdDuration::from_millis(50), state.sessions.write())
            .await
            .expect("session write lock remains available during Discord await");
        drop(lock);

        let response = request.await.expect("authorization request task");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(provider.saves.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn current_authorization_allows_each_authorized_write_once() {
        let manageable = r#"[{"id":"42","name":"Guild","icon":null,"permissions":"32"}]"#;
        let (discord_api_base, requests) =
            spawn_discord_guilds_server(StatusCode::OK, manageable, StdDuration::ZERO, 3).await;
        let settings = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Absent,
        ));
        let provider = Arc::new(CountingProviderStateRepository::default());
        let audit = Arc::new(CountingAuditLogRepository::default());
        let state = test_dashboard_state_with_discord(
            Persistence {
                guild_settings: Some(settings.clone()),
                provider_state: Some(provider.clone()),
                dashboard_audit_logs: Some(audit),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let module_id = first_module_id(&state);
        let command_id = first_command_id(&state);
        let app = build_dashboard_router(state);

        let module_response = app
            .clone()
            .oneshot(authenticated_json_request(
                "PATCH",
                &format!("/api/guild-settings/42/{module_id}"),
                "test-session",
                serde_json::json!({ "enabled": true }),
            ))
            .await
            .expect("guild module patch response");
        assert_eq!(module_response.status(), StatusCode::OK);
        assert_eq!(settings.writes.load(Ordering::SeqCst), 1);

        let command_response = app
            .clone()
            .oneshot(authenticated_json_request(
                "PATCH",
                &format!("/api/guild-command-settings/42/{command_id}"),
                "test-session",
                serde_json::json!({ "enabled": true }),
            ))
            .await
            .expect("guild command patch response");
        assert_eq!(command_response.status(), StatusCode::OK);
        assert_eq!(settings.writes.load(Ordering::SeqCst), 2);

        let sync_response = app
            .oneshot(authenticated_request(
                "POST",
                "/api/guild-command-sync/42",
                "test-session",
            ))
            .await
            .expect("guild command sync response");
        assert_eq!(sync_response.status(), StatusCode::OK);
        assert_eq!(provider.saves.load(Ordering::SeqCst), 1);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn production_router_does_not_expose_perf_instance_route() {
        let state = Arc::new(DashboardState {
            config: DashboardConfig {
                host: "127.0.0.1".parse().expect("loopback address"),
                port: 3000,
                public_base_url: "http://127.0.0.1:3000".to_string(),
                bot_token: "test-token".to_string(),
                client_secret: "test-secret".to_string(),
                invite_permissions: 0,
                owner_totp_secret: None,
                register_globally: false,
                command_sync_interval_seconds: 15,
            },
            http: reqwest::Client::new(),
            discord_api_base: DISCORD_API_BASE.to_string(),
            app_info: DiscordApplicationInfo {
                id: "test-app".to_string(),
                name: "Test App".to_string(),
                icon: None,
                owner_user_id: None,
            },
            module_catalog: ModuleCatalog::default(),
            command_catalog: CommandCatalog::default(),
            persistence: Persistence::default(),
            sessions: Default::default(),
            oauth_states: Default::default(),
            owner_security: Default::default(),
            #[cfg(feature = "perf-harness")]
            perf_runtime: None,
            #[cfg(feature = "local-preview")]
            local_preview: None,
        });

        let app = build_dashboard_router(state);
        for (method, path) in [
            ("GET", "/__perf/instance"),
            ("GET", "/__perf/counters"),
            ("POST", "/__perf/browser-outbound-attempt"),
            ("POST", "/__perf/shutdown"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .expect("valid request"),
                )
                .await
                .expect("router response");

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "path {path}");
        }
    }

    #[tokio::test]
    async fn font_routes_serve_locked_bytes_and_cache_headers() {
        let assets: [(&str, &[u8], &str, &str); 6] = [
            (
                FIRA_SANS_LIGHT_PATH,
                FIRA_SANS_LIGHT_BYTES,
                FIRA_SANS_LIGHT_SHA256,
                FIRA_SANS_LIGHT_ETAG,
            ),
            (
                FIRA_SANS_REGULAR_PATH,
                FIRA_SANS_REGULAR_BYTES,
                FIRA_SANS_REGULAR_SHA256,
                FIRA_SANS_REGULAR_ETAG,
            ),
            (
                FIRA_SANS_MEDIUM_PATH,
                FIRA_SANS_MEDIUM_BYTES,
                FIRA_SANS_MEDIUM_SHA256,
                FIRA_SANS_MEDIUM_ETAG,
            ),
            (
                FIRA_SANS_SEMIBOLD_PATH,
                FIRA_SANS_SEMIBOLD_BYTES,
                FIRA_SANS_SEMIBOLD_SHA256,
                FIRA_SANS_SEMIBOLD_ETAG,
            ),
            (
                FIRA_SANS_BOLD_PATH,
                FIRA_SANS_BOLD_BYTES,
                FIRA_SANS_BOLD_SHA256,
                FIRA_SANS_BOLD_ETAG,
            ),
            (
                FIRA_CODE_VARIABLE_PATH,
                FIRA_CODE_VARIABLE_BYTES,
                FIRA_CODE_VARIABLE_SHA256,
                FIRA_CODE_VARIABLE_ETAG,
            ),
        ];

        for (path, expected_bytes, expected_sha256, expected_etag) in assets {
            let hash_in_path = path
                .strip_suffix(".woff2")
                .and_then(|value| value.rsplit_once('-'))
                .map(|(_, hash)| hash)
                .expect("content-addressed WOFF2 path");
            assert_eq!(hash_in_path, expected_sha256);
            assert_eq!(hash_in_path.len(), 64);
            assert!(
                hash_in_path
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
            assert_eq!(expected_etag, format!("\"{expected_sha256}\""));

            let response = font_asset_router::<()>()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("font request"),
                )
                .await
                .expect("font route response");

            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "font/woff2");
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                FONT_CACHE_CONTROL
            );
            assert_eq!(response.headers()[header::ETAG], expected_etag);
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            let body = to_bytes(response.into_body(), expected_bytes.len())
                .await
                .expect("font response body");
            assert_eq!(body.as_ref(), expected_bytes);
        }
    }

    #[tokio::test]
    async fn font_routes_honor_weak_if_none_match() {
        let response = font_asset_router::<()>()
            .oneshot(
                Request::builder()
                    .uri(FIRA_SANS_REGULAR_PATH)
                    .header(header::IF_NONE_MATCH, format!("W/{FIRA_SANS_REGULAR_ETAG}"))
                    .body(Body::empty())
                    .expect("conditional font request"),
            )
            .await
            .expect("conditional font route response");

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], FIRA_SANS_REGULAR_ETAG);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            FONT_CACHE_CONTROL
        );
        assert!(
            to_bytes(response.into_body(), 1)
                .await
                .expect("not-modified body")
                .is_empty()
        );
    }

    #[test]
    fn dashboard_css_uses_only_same_origin_locked_font_faces() {
        let styles = super::assets::DASHBOARD_CSS;
        assert!(!styles.contains(concat!("fonts.", "googleapis.com")));
        assert!(!styles.contains(concat!("fonts.", "gstatic.com")));
        assert!(!styles.contains("@import"));
        assert!(styles.contains("font-display: swap"));
        assert!(styles.contains("font-synthesis: none"));
        assert!(styles.contains("'Fira Sans Fallback'"));
        assert!(styles.contains("'Fira Code Fallback'"));
        for path in [
            FIRA_SANS_LIGHT_PATH,
            FIRA_SANS_REGULAR_PATH,
            FIRA_SANS_MEDIUM_PATH,
            FIRA_SANS_SEMIBOLD_PATH,
            FIRA_SANS_BOLD_PATH,
            FIRA_CODE_VARIABLE_PATH,
        ] {
            assert!(styles.contains(path), "missing font URL {path}");
        }
    }

    #[tokio::test]
    async fn browser_assets_are_content_addressed_cached_and_csp_protected() {
        let state = test_dashboard_state(Persistence::default());
        let app = build_dashboard_router(state);
        for (path, content_type) in [
            (super::assets::DASHBOARD_CSS_PATH, "text/css; charset=utf-8"),
            (
                super::assets::DASHBOARD_JS_PATH,
                "text/javascript; charset=utf-8",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("asset response");
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                FONT_CACHE_CONTROL
            );
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert!(path.len() > 64, "asset path is content addressed");
        }

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("landing response");
        let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .expect("CSP header")
            .to_string();
        assert!(csp.contains("script-src 'self' 'sha256-"));
        assert!(csp.contains("style-src 'self';"));
        assert!(!csp.contains("unsafe-inline"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert_eq!(response.headers()[header::X_FRAME_OPTIONS], "DENY");
    }

    #[tokio::test]
    async fn language_route_sets_cookie_and_pages_follow_it() {
        let state = test_dashboard_state(Persistence::default());
        let app = build_dashboard_router(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/lang/ko?redirect=%2Fselector")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("language response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/selector");
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .expect("cookie")
            .to_string();
        assert!(cookie.starts_with("dynamo_lang=ko"));
        assert!(cookie.contains("Max-Age=31536000"));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/lang/ko?redirect=%2F%5Cevil.example")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("language response");
        assert_eq!(response.headers()[header::LOCATION], "/selector");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::COOKIE, "dynamo_lang=ko")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("landing response");
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("body");
        let rendered = String::from_utf8(body.to_vec()).expect("UTF-8");
        assert!(rendered.contains("<html lang=\"ko\""));
        assert!(rendered.contains("\"__locale\":\"ko-KR\""));
    }

    #[test]
    fn renders_text_field_with_schema_attributes() {
        let field = SettingsField {
            key: "channel_id",
            label: "Channel ID",
            help_text: Some("Target channel"),
            required: false,
            kind: SettingsFieldKind::Text,
        };

        let rendered = super::render::forms::render_field(
            &field,
            &serde_json::json!({ "channel_id": "123" }),
            "modal-guild-module-x",
            Locale::En,
        )
        .into_string();
        let id = "modal-guild-module-x-field-channel_id";
        assert!(rendered.contains("data-setting-key=\"channel_id\""));
        assert!(rendered.contains("data-setting-kind=\"text\""));
        assert!(rendered.contains("value=\"123\""));
        assert!(rendered.contains(&format!("label for=\"{id}\"")));
        assert!(rendered.contains(&format!("id=\"{id}\"")));
        assert!(rendered.contains(&format!("aria-describedby=\"{id}-help {id}-error\"")));
    }

    #[test]
    fn integer_field_renders_min_bound_when_configured() {
        let field = SettingsField {
            key: "refresh_interval_seconds",
            label: "Refresh interval",
            help_text: Some("Minimum 3 seconds."),
            required: false,
            kind: SettingsFieldKind::Integer {
                min: Some(3),
                max: None,
            },
        };

        let rendered = super::render::forms::render_field(
            &field,
            &serde_json::json!({ "refresh_interval_seconds": 3 }),
            "p",
            Locale::En,
        )
        .into_string();

        assert!(rendered.contains("type=\"number\""));
        assert!(rendered.contains("min=\"3\""));
        assert!(!rendered.contains("max="));
        assert!(rendered.contains("value=\"3\""));
    }

    #[test]
    fn integer_schema_serialization_preserves_unbounded_shape() {
        assert_eq!(
            serde_json::to_value(SettingsFieldKind::Integer {
                min: None,
                max: None,
            })
            .expect("serialize integer kind"),
            serde_json::json!({ "type": "integer" })
        );
    }

    #[test]
    fn integer_schema_serialization_includes_min_when_bounded() {
        assert_eq!(
            serde_json::to_value(SettingsFieldKind::Integer {
                min: Some(3),
                max: None,
            })
            .expect("serialize bounded integer kind"),
            serde_json::json!({ "type": "integer", "min": 3 })
        );
    }

    #[test]
    fn sanitize_redirect_rejects_external_targets() {
        assert_eq!(sanitize_redirect_target(Some("/selector")), "/selector");
        assert_eq!(
            sanitize_redirect_target(Some("https://evil.example")),
            "/selector"
        );
        assert_eq!(
            sanitize_redirect_target(Some("//evil.example")),
            "/selector"
        );
    }

    #[test]
    fn request_path_for_logging_drops_oauth_callback_query() {
        let uri: Uri = "/auth/discord/callback?code=secret-code&state=secret-state"
            .parse()
            .expect("uri");

        assert_eq!(request_path_for_logging(&uri), "/auth/discord/callback");
    }

    #[test]
    fn request_path_logging_skips_health_checks() {
        assert!(!request_path_should_be_logged("/healthz"));
        assert!(request_path_should_be_logged("/auth/discord/callback"));
    }

    #[test]
    fn request_id_for_logging_uses_bounded_request_id_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req-123"));

        assert_eq!(request_id_for_logging(&headers).as_deref(), Some("req-123"));
    }

    #[test]
    fn request_id_for_logging_rejects_unbounded_request_id_header() {
        let mut headers = HeaderMap::new();
        let long_value = "x".repeat(129);
        headers.insert(
            "x-request-id",
            HeaderValue::from_str(&long_value).expect("header value"),
        );

        assert_eq!(request_id_for_logging(&headers), None);
    }

    #[test]
    fn guild_manage_check_accepts_manage_guild_or_admin() {
        let manage_guild = DashboardGuild {
            id: 1,
            name: "Guild".to_string(),
            icon: None,
            permissions: (1u64 << 5).to_string(),
        };
        let admin = DashboardGuild {
            id: 1,
            name: "Guild".to_string(),
            icon: None,
            permissions: (1u64 << 3).to_string(),
        };
        let member = DashboardGuild {
            id: 1,
            name: "Guild".to_string(),
            icon: None,
            permissions: "0".to_string(),
        };

        assert!(user_can_manage_guild(&manage_guild));
        assert!(user_can_manage_guild(&admin));
        assert!(!user_can_manage_guild(&member));
    }

    #[test]
    fn audit_logs_section_renders_compact_table() {
        let entry = DashboardAuditLogEntry {
            id: Some("abc123".to_string()),
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-03-16T08:00:00Z")
                .expect("timestamp")
                .with_timezone(&chrono::Utc),
            actor_user_id: 7,
            actor_username: "Tester".to_string(),
            scope: DashboardAuditScope::Guild,
            guild_id: Some(42),
            entity_type: DashboardAuditEntityType::Command,
            entity_id: "etf".to_string(),
            action: DashboardAuditAction::Toggle,
            summary: "Updated guild command etf: enabled=false.".to_string(),
        };
        let names = |_: DashboardAuditEntityType, id: &str| format!("/{id}");
        let page = DashboardAuditLogPage {
            entries: vec![entry],
            page: 1,
            page_size: 20,
            total: 1,
        };
        let rendered = render_audit_logs_section(
            "/guild/42",
            &page,
            Some(DashboardAuditEntityType::Command),
            Some(DashboardAuditAction::Toggle),
            Locale::En,
            &names,
        )
        .into_string();

        assert!(rendered.contains("Change log"));
        assert!(rendered.contains("data-testid=\"logs-table\""));
        assert!(rendered.contains("<colgroup>"));
        assert!(rendered.contains("datetime=\"2026-03-16T08:00:00Z\""));
        assert!(rendered.contains("data-relative-time=\"2026-03-16T08:00:00Z\""));
        assert!(rendered.contains("/etf"));
        assert!(rendered.contains(audit_entity_label(
            DashboardAuditEntityType::Command,
            Locale::En
        )));
        assert!(rendered.contains(audit_action_label(DashboardAuditAction::Toggle, Locale::En)));
        assert!(rendered.contains(">Disabled<"));
        assert!(rendered.contains("data-testid=\"logs-mobile-list\""));
        assert!(rendered.contains("data-testid=\"audit-log-card-0\""));
        assert!(rendered.contains("data-log-field=\"summary\""));
        assert_eq!(rendered.matches("data-log-field=\"action\"").count(), 1);

        let korean = render_audit_logs_section("/guild/42", &page, None, None, Locale::Ko, &names)
            .into_string();
        assert!(korean.contains("변경 기록"));
        assert!(korean.contains(">사용 안 함<"));
    }

    #[test]
    fn audit_logs_section_empty_state_renders_mobile_empty_state() {
        let names = |_: DashboardAuditEntityType, id: &str| id.to_string();
        let rendered = render_audit_logs_section(
            "/deployment",
            &DashboardAuditLogPage::empty(1, 20),
            None,
            None,
            Locale::En,
            &names,
        )
        .into_string();

        assert!(rendered.contains("No changes recorded yet."));
        assert!(rendered.contains("data-testid=\"logs-mobile-list\""));
        assert!(rendered.contains("data-testid=\"logs-mobile-empty\""));
        assert!(rendered.contains("aria-disabled=\"true\""));
    }

    #[tokio::test]
    async fn settings_dialog_fragment_has_dialog_semantics_and_requires_access() {
        let state = test_dashboard_state(Persistence::default());
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let response = app
            .clone()
            .oneshot(authenticated_request(
                "GET",
                "/guild/42/settings/module/stock",
                "test-session",
            ))
            .await
            .expect("fragment response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("body");
        let rendered = String::from_utf8(body.to_vec()).expect("UTF-8");
        assert!(rendered.contains("data-testid=\"settings-modal-modal-guild-module-stock\""));
        assert!(rendered.contains("role=\"dialog\""));
        assert!(rendered.contains("aria-modal=\"true\""));
        assert!(rendered.contains("aria-labelledby=\"modal-title-modal-guild-module-stock\""));
        assert!(rendered.contains("aria-label=\"Close\""));
        assert!(rendered.contains("data-modal-root"));
        assert!(rendered.contains("name=\"enabled\""));
        assert!(rendered.contains("data-discard-bar"));
        assert!(!rendered.contains("<html"));

        let response = app
            .clone()
            .oneshot(authenticated_request(
                "GET",
                "/guild/43/settings/module/stock",
                "test-session",
            ))
            .await
            .expect("forbidden fragment response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = app
            .oneshot(authenticated_request(
                "GET",
                "/guild/42/settings/module/does-not-exist",
                "test-session",
            ))
            .await
            .expect("missing fragment response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn guild_state_api_reports_localized_statuses() {
        let state = test_dashboard_state(Persistence::default());
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);
        let mut request = authenticated_request("GET", "/api/guild-state/42", "test-session");
        request
            .headers_mut()
            .insert(header::ACCEPT_LANGUAGE, HeaderValue::from_static("ko-KR"));
        let response = app.oneshot(request).await.expect("state response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_response(response).await;
        let entities = body["entities"].as_array().expect("entities");
        let stock = entities
            .iter()
            .find(|entity| entity["kind"] == "module" && entity["id"] == "stock")
            .expect("stock module");
        assert!(["active", "off", "blocked"].contains(&stock["state"].as_str().unwrap()));
        assert!(["활성", "꺼짐", "차단됨"].contains(&stock["state_label"].as_str().unwrap()));
        assert!(
            stock["chain"]
                .as_array()
                .is_some_and(|chain| chain.len() == 3)
        );
    }

    #[tokio::test]
    async fn owner_console_requires_app_owner_and_totp_configuration() {
        let mut state = test_dashboard_state(Persistence::default());
        {
            let inner = Arc::get_mut(&mut state).expect("unique state");
            inner.app_info.owner_user_id = Some(999);
        }
        insert_session(&state, "non-owner", 42).await;
        let app = build_dashboard_router(state.clone());

        let page = app
            .clone()
            .oneshot(authenticated_request("GET", "/deployment", "non-owner"))
            .await
            .expect("deployment response");
        assert_eq!(page.status(), StatusCode::FORBIDDEN);

        let api = app
            .oneshot(authenticated_json_request(
                "PATCH",
                "/api/owner/notices",
                "non-owner",
                serde_json::json!({ "module_notices": {} }),
            ))
            .await
            .expect("owner api response");
        assert_eq!(api.status(), StatusCode::FORBIDDEN);

        let mut unconfigured = test_dashboard_state(Persistence::default());
        Arc::get_mut(&mut unconfigured)
            .expect("unique state")
            .app_info
            .owner_user_id = Some(7);
        insert_session(&unconfigured, "owner-session", 42).await;
        let app = build_dashboard_router(unconfigured);

        let setup_page = app
            .clone()
            .oneshot(authenticated_request("GET", "/deployment", "owner-session"))
            .await
            .expect("setup page response");
        assert_eq!(setup_page.status(), StatusCode::OK);
        let setup_html = String::from_utf8(
            to_bytes(setup_page.into_body(), 2 * 1024 * 1024)
                .await
                .expect("body")
                .to_vec(),
        )
        .expect("utf8");
        assert!(setup_html.contains("data-testid=\"owner-totp-setup\""));

        let api_denied = app
            .oneshot(authenticated_request(
                "GET",
                "/api/deployment-settings",
                "owner-session",
            ))
            .await
            .expect("api denied response");
        assert_eq!(api_denied.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn owner_totp_unlock_verifies_live_identity_prevents_replay_and_rate_limits() {
        let (discord_api_base, _requests) = spawn_discord_guilds_server(
            StatusCode::OK,
            r#"{"id":"7","username":"owner"}"#,
            StdDuration::ZERO,
            10,
        )
        .await;
        let mut state = test_dashboard_state_with_discord(
            Persistence::default(),
            discord_api_base,
            StdDuration::from_secs(1),
        );
        {
            let inner = Arc::get_mut(&mut state).expect("unique state");
            inner.app_info.owner_user_id = Some(7);
            inner.config.owner_totp_secret = Some(
                crate::owner_auth::parse_totp_secret(crate::owner_auth::DEMO_TOTP_SECRET_BASE32)
                    .expect("demo totp secret"),
            );
        }
        insert_session(&state, "owner-session", 42).await;
        let app = build_dashboard_router(state);

        let demo_secret =
            crate::owner_auth::parse_totp_secret(crate::owner_auth::DEMO_TOTP_SECRET_BASE32)
                .expect("demo totp secret");
        let current_step = crate::owner_auth::current_totp_step(Utc::now());
        let valid_code = crate::owner_auth::totp_code_for_step(&demo_secret, current_step);
        let verify_req = |code: &str| {
            Request::builder()
                .method("POST")
                .uri("/deployment/verify")
                .header(
                    header::COOKIE,
                    format!("{SESSION_COOKIE_NAME}=owner-session"),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token=csrf-owner-session&totp_code={code}"
                )))
                .expect("valid verify request")
        };

        let unlocked = app
            .clone()
            .oneshot(verify_req(&valid_code))
            .await
            .expect("unlock response");
        assert_eq!(unlocked.status(), StatusCode::SEE_OTHER);
        let set_cookies = unlocked
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>()
            .join(";");
        assert!(set_cookies.contains(crate::owner_auth::OWNER_ELEVATION_COOKIE_NAME));

        let replayed = app
            .clone()
            .oneshot(verify_req(&valid_code))
            .await
            .expect("replay response");
        assert_eq!(replayed.status(), StatusCode::SEE_OTHER);
        let replay_loc = replayed
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(replay_loc.contains("auth_notice=replayed"));

        for attempt in 0..crate::owner_auth::TOTP_MAX_FAILURES {
            let bad = app
                .clone()
                .oneshot(verify_req("000000"))
                .await
                .expect("bad totp response");
            assert_eq!(bad.status(), StatusCode::SEE_OTHER);
            let loc = bad
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if attempt + 1 < crate::owner_auth::TOTP_MAX_FAILURES {
                assert!(loc.contains("auth_notice=invalid"));
            } else {
                assert!(loc.contains("auth_notice=rate_limited"));
            }
        }
    }

    #[tokio::test]
    async fn mutating_routes_enforce_csrf_and_same_site_and_persist_announcement_channel() {
        let guilds_json = r#"[{"id":"42","name":"Guild","icon":null,"permissions":"32"}]"#;
        let (discord_api_base, _) =
            spawn_discord_guilds_server(StatusCode::OK, guilds_json, StdDuration::ZERO, 2).await;
        let settings = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Absent,
        ));
        let audit = Arc::new(CountingAuditLogRepository::default());
        let state = test_dashboard_state_with_discord(
            Persistence {
                guild_settings: Some(settings.clone()),
                dashboard_audit_logs: Some(audit.clone()),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        // Missing X-CSRF-Token is rejected
        let missing_csrf = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/guild-announcement-channel/42")
                    .header(
                        header::COOKIE,
                        format!("{SESSION_COOKIE_NAME}=test-session"),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "announcement_channel_id": "9000000000000001001" })
                            .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

        // Cross-site Sec-Fetch-Site is rejected even with valid X-CSRF-Token
        let mut cross_site = authenticated_json_request(
            "PATCH",
            "/api/guild-announcement-channel/42",
            "test-session",
            serde_json::json!({ "announcement_channel_id": "9000000000000001001" }),
        );
        cross_site
            .headers_mut()
            .insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        let cross_site_res = app
            .clone()
            .oneshot(cross_site)
            .await
            .expect("cross-site response");
        assert_eq!(cross_site_res.status(), StatusCode::FORBIDDEN);

        // Valid CSRF + live guild access persists announcement channel
        let ok_res = app
            .oneshot(authenticated_json_request(
                "PATCH",
                "/api/guild-announcement-channel/42",
                "test-session",
                serde_json::json!({ "announcement_channel_id": "9000000000000001001" }),
            ))
            .await
            .expect("ok response");
        assert_eq!(ok_res.status(), StatusCode::OK);
        let body = json_response(ok_res).await;
        assert_eq!(body["announcement_channel_id"], 9000000000000001001u64);
        assert_eq!(settings.writes.load(Ordering::SeqCst), 1);
        assert_eq!(audit.appends.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invite_ranks_and_ticket_categories_reject_malformed_configurations() {
        let guilds_json = r#"[{"id":"42","name":"Guild","icon":null,"permissions":"32"}]"#;
        let (discord_api_base, _) =
            spawn_discord_guilds_server(StatusCode::OK, guilds_json, StdDuration::ZERO, 3).await;
        let settings = Arc::new(FakeGuildSettingsRepository::new(
            GuildSettingsReadResult::Absent,
        ));
        let state = test_dashboard_state_with_discord(
            Persistence {
                guild_settings: Some(settings.clone()),
                ..Persistence::default()
            },
            discord_api_base,
            StdDuration::from_secs(1),
        );
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        let bad_invite = app
            .clone()
            .oneshot(authenticated_json_request(
                "PATCH",
                "/api/guild-settings/42/invite",
                "test-session",
                serde_json::json!({
                    "configuration": {
                        "ranks": [{ "invites": 0, "role_id": "not-a-snowflake" }]
                    }
                }),
            ))
            .await
            .expect("bad invite response");
        assert_eq!(bad_invite.status(), StatusCode::BAD_REQUEST);

        let bad_ticket = app
            .clone()
            .oneshot(authenticated_json_request(
                "PATCH",
                "/api/guild-settings/42/ticket",
                "test-session",
                serde_json::json!({
                    "configuration": {
                        "categories": [{ "name": "", "staff_roles": ["invalid"] }]
                    }
                }),
            ))
            .await
            .expect("bad ticket response");
        assert_eq!(bad_ticket.status(), StatusCode::BAD_REQUEST);

        let good_invite = app
            .oneshot(authenticated_json_request(
                "PATCH",
                "/api/guild-settings/42/invite",
                "test-session",
                serde_json::json!({
                    "configuration": {
                        "ranks": [{ "invites": 5, "role_id": "9000000000000002001" }]
                    }
                }),
            ))
            .await
            .expect("good invite response");
        assert_eq!(good_invite.status(), StatusCode::OK);
        assert_eq!(settings.writes.load(Ordering::SeqCst), 1);
    }
}
