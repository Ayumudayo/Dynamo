use std::{
    collections::{HashMap, HashSet},
    env,
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, patch, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use dynamo_enablement::{
    ResolvedCommandState, ResolvedModuleState, resolve_command_states, resolve_module_states,
};
use dynamo_module_kit::{
    CommandCatalog, CommandCatalogEntry, ModuleCatalog, ModuleCatalogEntry, SettingsField,
    SettingsFieldKind, SettingsSchema,
};
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
use futures_util::{StreamExt, stream};
use rand::{Rng, distributions::Alphanumeric};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{info, warn};
use url::Url;

mod font_assets;

pub(crate) use font_assets::*;

#[cfg(feature = "perf-harness")]
mod perf_harness;

#[cfg(feature = "perf-harness")]
pub use perf_harness::run_perf_harness;

const SESSION_COOKIE_NAME: &str = "dynamo_dashboard_session";
const SESSION_TTL_HOURS: i64 = 24 * 14;
const OAUTH_STATE_TTL_MINUTES: i64 = 15;
const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DEFAULT_INVITE_PERMISSIONS: u64 = 2_146_958_847;
const DASHBOARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DASHBOARD_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

fn build_dashboard_http_client_with_timeouts(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("Dynamo Dashboard/0.1.0")
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()?)
}

fn build_dashboard_http_client() -> anyhow::Result<reqwest::Client> {
    build_dashboard_http_client_with_timeouts(DASHBOARD_CONNECT_TIMEOUT, DASHBOARD_REQUEST_TIMEOUT)
}

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
        sessions: Arc::new(RwLock::new(HashMap::new())),
        oauth_states: Arc::new(RwLock::new(HashMap::new())),
        #[cfg(feature = "perf-harness")]
        perf_runtime: None,
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
        .layer(middleware::from_fn(log_request))
}

fn build_dashboard_routes() -> Router<Arc<DashboardState>> {
    Router::new()
        .route("/", get(index))
        .route("/login", get(login))
        .route("/auth/discord/callback", get(discord_callback))
        .route("/logout", get(logout))
        .route("/selector", get(selector))
        .route("/deployment", get(deployment_page))
        .route("/guild/{guild_id}", get(guild_page))
        .route("/healthz", get(healthz))
        .route("/api/modules", get(list_modules))
        .route(
            "/api/module-states/default",
            get(list_default_module_states),
        )
        .route("/api/module-states/live", get(list_live_module_states))
        .route("/api/deployment-settings", get(get_deployment_settings))
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
        .route("/api/guild-settings/{guild_id}", get(get_guild_settings))
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
}

#[derive(Debug, Clone)]
struct DashboardConfig {
    host: std::net::IpAddr,
    port: u16,
    public_base_url: String,
    bot_token: String,
    client_secret: String,
    invite_permissions: u64,
    admin_user_ids: Vec<u64>,
    register_globally: bool,
    command_sync_interval_seconds: u64,
}

impl DashboardConfig {
    fn from_env() -> anyhow::Result<Self> {
        let host = env::var("DASHBOARD_HOST")
            .unwrap_or_else(|_| "127.0.0.1".to_string())
            .parse()
            .map_err(|error| {
                anyhow::anyhow!("DASHBOARD_HOST must be a valid IP address: {error}")
            })?;

        let port = env::var("DASHBOARD_PORT")
            .unwrap_or_else(|_| "3000".to_string())
            .parse()
            .map_err(|error| anyhow::anyhow!("DASHBOARD_PORT must be a valid u16: {error}"))?;

        let public_base_url = env::var("DASHBOARD_BASE_URL")
            .unwrap_or_else(|_| format!("http://{host}:{port}"))
            .trim_end_matches('/')
            .to_string();

        let bot_token = env::var("DISCORD_TOKEN")
            .or_else(|_| env::var("BOT_TOKEN"))
            .map_err(|_| anyhow::anyhow!("DISCORD_TOKEN or BOT_TOKEN must be set"))?;

        let client_secret = env::var("DISCORD_CLIENT_SECRET")
            .or_else(|_| env::var("BOT_SECRET"))
            .map_err(|_| {
                anyhow::anyhow!(
                    "DISCORD_CLIENT_SECRET or BOT_SECRET must be set for dashboard OAuth"
                )
            })?;

        let invite_permissions = env::var("DISCORD_BOT_INVITE_PERMISSIONS")
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|error| {
                anyhow::anyhow!("DISCORD_BOT_INVITE_PERMISSIONS must be a valid u64: {error}")
            })?
            .unwrap_or(DEFAULT_INVITE_PERMISSIONS);

        let admin_user_ids = parse_u64_list_env("DASHBOARD_ADMIN_USER_IDS")?;
        let dev_guild_id = env::var("DISCORD_DEV_GUILD_ID")
            .or_else(|_| env::var("GUILD_ID"))
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|error| {
                anyhow::anyhow!("DISCORD_DEV_GUILD_ID or GUILD_ID must be a valid u64: {error}")
            })?;
        let register_globally = match env::var("DISCORD_REGISTER_GLOBALLY") {
            Ok(value) => parse_bool_value("DISCORD_REGISTER_GLOBALLY", &value)?,
            Err(env::VarError::NotPresent) => dev_guild_id.is_none(),
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "DISCORD_REGISTER_GLOBALLY could not be read: {error}"
                ));
            }
        };
        let command_sync_interval_seconds =
            parse_u64_env("DISCORD_COMMAND_SYNC_INTERVAL_SECONDS", 15)?;

        Ok(Self {
            host,
            port,
            public_base_url,
            bot_token,
            client_secret,
            invite_permissions,
            admin_user_ids,
            register_globally,
            command_sync_interval_seconds,
        })
    }
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
    match (
        state.app_info.owner_user_id,
        state.config.admin_user_ids.is_empty(),
    ) {
        (Some(owner_id), true) => format!("owner-only ({owner_id})"),
        (Some(owner_id), false) => format!(
            "owner ({owner_id}) + {} explicit admin(s)",
            state.config.admin_user_ids.len()
        ),
        (None, true) => "bot application owner only".to_string(),
        (None, false) => format!(
            "application owner + {} explicit admin(s)",
            state.config.admin_user_ids.len()
        ),
    }
}

#[derive(Clone)]
struct DashboardState {
    config: DashboardConfig,
    http: reqwest::Client,
    discord_api_base: String,
    app_info: DiscordApplicationInfo,
    module_catalog: ModuleCatalog,
    command_catalog: CommandCatalog,
    persistence: Persistence,
    sessions: Arc<RwLock<HashMap<String, DashboardSession>>>,
    oauth_states: Arc<RwLock<HashMap<String, PendingOauthState>>>,
    #[cfg(feature = "perf-harness")]
    perf_runtime: Option<Arc<perf_harness::PerfRuntime>>,
}

#[derive(Debug, Clone)]
struct DiscordApplicationInfo {
    id: String,
    name: String,
    icon: Option<String>,
    owner_user_id: Option<u64>,
}

#[derive(Debug, Clone)]
struct DashboardSession {
    user: DashboardUser,
    guilds: Vec<DashboardGuild>,
    access_token: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
struct PendingOauthState {
    redirect_to: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DashboardUser {
    id: u64,
    username: String,
    global_name: Option<String>,
    avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DashboardGuild {
    #[serde(deserialize_with = "deserialize_u64_from_discord_id")]
    id: u64,
    name: String,
    icon: Option<String>,
    #[serde(default, alias = "permissions_new")]
    permissions: String,
}

#[derive(Debug, Clone)]
struct GuildCard {
    id: u64,
    name: String,
    icon_url: Option<String>,
    bot_presence: BotGuildPresence,
    manage_url: String,
    invite_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BotGuildPresence {
    Present,
    Missing,
    Unavailable,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct DashboardPageQuery {
    tab: Option<String>,
    log_entity: Option<String>,
    log_action: Option<String>,
    log_page: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct LoginQuery {
    redirect: Option<String>,
}

fn normalized_tab(value: Option<&str>) -> &'static str {
    match value {
        Some("modules") => "modules",
        Some("commands") => "commands",
        Some("logs") => "logs",
        _ => "overview",
    }
}

fn parse_audit_entity_filter(value: Option<&str>) -> Option<DashboardAuditEntityType> {
    match value {
        Some("module") => Some(DashboardAuditEntityType::Module),
        Some("command") => Some(DashboardAuditEntityType::Command),
        _ => None,
    }
}

fn parse_audit_action_filter(value: Option<&str>) -> Option<DashboardAuditAction> {
    match value {
        Some("toggle") => Some(DashboardAuditAction::Toggle),
        Some("save_settings") => Some(DashboardAuditAction::SaveSettings),
        _ => None,
    }
}

fn page_query_for_tab(tab: &str) -> String {
    format!("?tab={tab}")
}

fn page_query_for_logs(
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
    page: u64,
) -> String {
    let mut params = vec!["tab=logs".to_string()];
    if let Some(entity_type) = entity_type {
        params.push(format!("log_entity={}", entity_type.as_str()));
    }
    if let Some(action) = action {
        params.push(format!("log_action={}", action.as_str()));
    }
    if page > 1 {
        params.push(format!("log_page={page}"));
    }
    format!("?{}", params.join("&"))
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
struct CommandSyncPanel {
    state: CommandSyncDisplayState,
    title: String,
    message: String,
    button_label: Option<String>,
    button_action: Option<String>,
    status_text: Option<String>,
}

async fn load_command_sync_store(persistence: &Persistence) -> CommandSyncStateStore {
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

fn build_command_sync_panel(
    scope: SyncScopeKind,
    current_fingerprint: &str,
    scope_state: Option<&CommandSyncScopeState>,
    config: &DashboardConfig,
) -> CommandSyncPanel {
    let scope_state = scope_state.cloned().unwrap_or_default();
    let button_label = match scope {
        SyncScopeKind::Global => Some("Sync Global Commands".to_string()),
        SyncScopeKind::Guild(_) => Some("Sync Commands".to_string()),
    };
    let button_action = match scope {
        SyncScopeKind::Global => Some("requestDeploymentCommandSync(this)".to_string()),
        SyncScopeKind::Guild(guild_id) => {
            Some(format!("requestGuildCommandSync({guild_id}, this)"))
        }
    };

    if scope_state.has_pending_request() {
        return CommandSyncPanel {
            state: CommandSyncDisplayState::Pending,
            title: "Sync Requested".to_string(),
            message: format!(
                "A manual command sync request is queued. The bot will apply it on the next {} second sync cycle.",
                config.command_sync_interval_seconds.max(5)
            ),
            button_label,
            button_action,
            status_text: format_sync_status_text(&scope_state),
        };
    }

    if scope_state.last_result == Some(CommandSyncResult::Failed) {
        return CommandSyncPanel {
            state: CommandSyncDisplayState::Failed,
            title: "Last Sync Failed".to_string(),
            message: scope_state
                .last_error
                .clone()
                .unwrap_or_else(|| "The last command sync attempt failed.".to_string()),
            button_label,
            button_action,
            status_text: format_sync_status_text(&scope_state),
        };
    }

    if scope_state.is_in_sync_with(current_fingerprint) {
        return CommandSyncPanel {
            state: CommandSyncDisplayState::InSync,
            title: "Commands Are In Sync".to_string(),
            message: "The command set currently stored in Discord matches the command set resolved from the dashboard settings.".to_string(),
            button_label,
            button_action,
            status_text: format_sync_status_text(&scope_state),
        };
    }

    CommandSyncPanel {
        state: CommandSyncDisplayState::Required,
        title: "Sync Required".to_string(),
        message: "Dashboard command settings differ from the last command set synced to Discord. Run a sync to apply the current command layout.".to_string(),
        button_label,
        button_action,
        status_text: format_sync_status_text(&scope_state),
    }
}

fn build_unsupported_sync_panel(message: &str) -> CommandSyncPanel {
    CommandSyncPanel {
        state: CommandSyncDisplayState::Unsupported,
        title: "Sync Managed Elsewhere".to_string(),
        message: message.to_string(),
        button_label: None,
        button_action: None,
        status_text: None,
    }
}

fn format_sync_status_text(scope_state: &CommandSyncScopeState) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(synced_at) = scope_state.last_synced_at {
        parts.push(format!(
            "Last synced {}",
            synced_at.format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }
    if let Some(command_count) = scope_state.last_submitted_top_level_commands {
        parts.push(format!("{command_count} top-level commands"));
    }
    if let Some(requested_at) = scope_state.requested_at {
        parts.push(format!(
            "Last requested {}",
            requested_at.format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }
    if let Some(requested_by) = &scope_state.requested_by_username {
        parts.push(format!("Requested by {requested_by}"));
    }
    (!parts.is_empty()).then(|| parts.join(" | "))
}

#[derive(Debug, Deserialize)]
struct DiscordCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn index(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
    let session = load_session(&state, &jar).await;
    Html(render_landing_page(&state, session.as_ref())).into_response()
}

async fn login(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    axum::extract::Query(query): axum::extract::Query<LoginQuery>,
) -> Response {
    if load_session(&state, &jar).await.is_some() {
        let target = sanitize_redirect_target(query.redirect.as_deref());
        return Redirect::to(&target).into_response();
    }

    let state_token = random_token(48);
    let redirect_to = sanitize_redirect_target(query.redirect.as_deref());
    {
        let mut pending = state.oauth_states.write().await;
        pending.retain(|_, value| !is_oauth_state_expired(value));
        pending.insert(
            state_token.clone(),
            PendingOauthState {
                redirect_to,
                created_at: chrono::Utc::now(),
            },
        );
    }

    Redirect::to(&build_discord_authorize_url(&state, &state_token)).into_response()
}

async fn discord_callback(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    axum::extract::Query(query): axum::extract::Query<DiscordCallbackQuery>,
) -> Response {
    if let Some(error) = query.error {
        return Html(render_error_page(
            &state,
            None,
            "Discord Login Failed",
            &format!("Discord returned an OAuth error: {}.", escape_html(&error)),
        ))
        .into_response();
    }

    let Some(code) = query.code.as_deref() else {
        return Html(render_error_page(
            &state,
            None,
            "Discord Login Failed",
            "Discord did not return an authorization code.",
        ))
        .into_response();
    };

    let Some(oauth_state) = query.state.as_deref() else {
        return Html(render_error_page(
            &state,
            None,
            "Discord Login Failed",
            "Missing OAuth state. Please try signing in again.",
        ))
        .into_response();
    };

    let pending = {
        let mut states = state.oauth_states.write().await;
        states.retain(|_, value| !is_oauth_state_expired(value));
        states.remove(oauth_state)
    };

    let Some(pending) = pending else {
        return Html(render_error_page(
            &state,
            None,
            "Discord Login Failed",
            "The login session expired or was already used. Please try again.",
        ))
        .into_response();
    };

    match exchange_oauth_code(&state, code).await {
        Ok(session) => {
            let session_id = random_token(64);
            {
                let mut sessions = state.sessions.write().await;
                sessions.retain(|_, value| !is_session_expired(value));
                sessions.insert(session_id.clone(), session);
            }

            let jar = jar.add(session_cookie(&session_id));
            (jar, Redirect::to(&pending.redirect_to)).into_response()
        }
        Err(error) => {
            warn!(?error, "failed to complete Discord OAuth callback");
            Html(render_error_page(
                &state,
                None,
                "Discord Login Failed",
                "Could not exchange the Discord OAuth code or load your guild list.",
            ))
            .into_response()
        }
    }
}

async fn logout(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE_NAME) {
        state.sessions.write().await.remove(cookie.value());
    }

    let jar = jar.remove(Cookie::from(SESSION_COOKIE_NAME));
    (jar, Redirect::to("/")).into_response()
}

async fn selector(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
    let Some(existing_session) = load_session(&state, &jar).await else {
        return Redirect::to("/login?redirect=%2Fselector").into_response();
    };
    let session = match refresh_read_session(&state, &jar).await {
        Ok(session) => session,
        Err(ReadGuildAuthorizationError::LoginRequired) => {
            return Redirect::to("/login?redirect=%2Fselector").into_response();
        }
        Err(ReadGuildAuthorizationError::Unavailable) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(render_error_page(
                    &state,
                    Some(&existing_session),
                    "Guild Access Unavailable",
                    "Discord could not verify your current server access. Please try again.",
                )),
            )
                .into_response();
        }
    };

    let guild_cards = load_guild_cards(&state, &session).await;
    Html(render_selector_page(&state, &session, &guild_cards)).into_response()
}

async fn deployment_page(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<DashboardPageQuery>,
) -> Response {
    let Some(session) = load_session(&state, &jar).await else {
        return Redirect::to("/login?redirect=%2Fdeployment").into_response();
    };
    if !user_is_dashboard_admin(&state, &session.user) {
        return (
            StatusCode::FORBIDDEN,
            Html(render_error_page(
                &state,
                Some(&session),
                "Dashboard Access Restricted",
                "Deployment-wide settings are reserved for the bot owner or configured dashboard administrators.",
            )),
        )
            .into_response();
    }

    let active_tab = normalized_tab(query.tab.as_deref());
    let log_entity = parse_audit_entity_filter(query.log_entity.as_deref());
    let log_action = parse_audit_action_filter(query.log_action.as_deref());
    let log_page = query.log_page.unwrap_or(1).max(1);
    let (overview, active_section, modals, include_mutation_script) = match active_tab {
        "logs" => {
            let logs_page = match state
                .persistence
                .list_dashboard_audit_logs(DashboardAuditLogQuery {
                    scope: DashboardAuditScope::Deployment,
                    guild_id: None,
                    entity_type: log_entity,
                    action: log_action,
                    page: log_page,
                    page_size: 20,
                })
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    warn!(?error, "failed to load deployment dashboard audit logs");
                    DashboardAuditLogPage::empty(log_page, 20)
                }
            };
            (
                String::new(),
                render_audit_logs_section("/deployment", &logs_page, log_entity, log_action),
                String::new(),
                false,
            )
        }
        tab => {
            let settings = match state.persistence.deployment_settings_or_default().await {
                Ok(settings) => settings,
                Err(_) => {
                    warn!("failed to load deployment settings page");
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Html(render_error_page(
                            &state,
                            Some(&session),
                            "Deployment Settings Unavailable",
                            "Deployment settings could not be loaded. Please try again.",
                        )),
                    )
                        .into_response();
                }
            };
            match tab {
                "modules" => {
                    let resolved_states =
                        resolve_module_states(&state.module_catalog, &settings, None);
                    let modals = state
                        .module_catalog
                        .entries
                        .iter()
                        .zip(&resolved_states)
                        .map(|(entry, resolved)| {
                            let current = settings.modules.get(entry.module.id).cloned().unwrap_or(
                                DeploymentModuleSettings {
                                    installed: true,
                                    enabled: entry.module.enabled_by_default,
                                },
                            );
                            render_deployment_module_modal(
                                entry,
                                resolved,
                                &render_module_runtime_notice(entry.module.id),
                                &current,
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let cards = render_module_summary_cards(
                        "deployment",
                        &state.module_catalog,
                        &settings,
                        None,
                        &resolved_states,
                    );
                    (
                        String::new(),
                        format!(
                            "<section id=\"modules\" class=\"section-block\" data-testid=\"deployment-modules-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Modules</p><h2>Deployment Modules</h2></div><input id=\"module-filter\" class=\"toolbar-search compact-search\" type=\"search\" aria-label=\"Search deployment modules\" aria-describedby=\"module-filter-status module-filter-empty\" placeholder=\"Search modules\" oninput=\"filterModuleCards(this.value)\" /></div><p id=\"module-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"module-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No modules match this search.</p><div class=\"module-grid compact-grid\">{cards}</div></section>"
                        ),
                        modals,
                        true,
                    )
                }
                "commands" => {
                    let resolved_states = resolve_command_states(
                        &state.module_catalog,
                        &state.command_catalog,
                        &settings,
                        None,
                    );
                    let cards = render_command_summary_cards(
                        "deployment",
                        &state.command_catalog,
                        &settings,
                        None,
                        &resolved_states,
                    );
                    let sync_panel = if state.config.register_globally {
                        let store = load_command_sync_store(&state.persistence).await;
                        let (fingerprint, _) =
                            dynamo_app::application_command_fingerprint_for_scope(&settings, None);
                        render_command_sync_panel(&build_command_sync_panel(
                            SyncScopeKind::Global,
                            &fingerprint,
                            Some(&store.global),
                            &state.config,
                        ))
                    } else {
                        render_command_sync_panel(&build_unsupported_sync_panel(
                            "Deployment command sync is disabled in guild-scoped mode. Open a guild page and run Sync Commands there.",
                        ))
                    };
                    let modals = render_deployment_command_modals(
                        &state.command_catalog,
                        &settings,
                        &resolved_states,
                    );
                    (
                        String::new(),
                        format!(
                            "<section id=\"commands\" class=\"section-block\" data-testid=\"deployment-commands-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Commands</p><h2>Deployment Commands</h2></div><input id=\"command-filter\" class=\"toolbar-search compact-search\" type=\"search\" aria-label=\"Search deployment commands\" aria-describedby=\"command-filter-status command-filter-empty\" placeholder=\"Search commands\" oninput=\"filterCommandCards(this.value)\" /></div><p id=\"command-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"command-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No commands match this search and category.</p>{sync_panel}{tabs}<div class=\"module-grid command-grid compact-grid\" data-testid=\"command-card-grid\">{cards}</div></section>",
                            tabs = render_command_category_tabs(&state.command_catalog)
                        ),
                        modals,
                        true,
                    )
                }
                _ => {
                    let modules = resolve_module_states(&state.module_catalog, &settings, None);
                    let commands = resolve_command_states(
                        &state.module_catalog,
                        &state.command_catalog,
                        &settings,
                        None,
                    );
                    let overview = render_overview_section(
                        "Deployment Control",
                        "Global install state and command availability across every guild.",
                        &[
                            (
                                "Modules Enabled",
                                count_enabled_modules(&modules).to_string(),
                            ),
                            (
                                "Commands Enabled",
                                count_enabled_commands(&commands).to_string(),
                            ),
                            (
                                "Runtime Notes",
                                count_runtime_notices(&state.module_catalog).to_string(),
                            ),
                        ],
                    );
                    let panel = format!(
                        "<section class=\"section-block\" data-testid=\"deployment-overview-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Overview</p><h2>Deployment Summary</h2></div></div><div class=\"grid two compact-grid-two\"><article class=\"panel info-panel compact-info-panel\"><h3>Scope</h3><p>Deployment settings define the default module installation and command availability used across every guild.</p></article><article class=\"panel info-panel compact-info-panel\"><h3>Runtime Notes</h3>{}</article></div></section>",
                        render_runtime_notices(&state.module_catalog)
                    );
                    (overview, panel, String::new(), false)
                }
            }
        }
    };
    let script = if include_mutation_script {
        format!("<script>{}</script>", dashboard_script())
    } else {
        String::new()
    };
    let content = format!(
        "{}{modals}{script}",
        render_dashboard_page_shell(
            &overview,
            &render_section_tabs("/deployment", active_tab),
            &active_section,
            active_tab
        )
    );

    Html(render_document(
        &state,
        Some(&session),
        "Deployment Settings",
        "Global module installation, enablement, and command controls.",
        Some("/deployment"),
        Some(active_tab),
        &content,
    ))
    .into_response()
}

async fn guild_page(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
    Query(query): Query<DashboardPageQuery>,
) -> Response {
    let Some(existing_session) = load_session(&state, &jar).await else {
        return Redirect::to(&format!("/login?redirect=%2Fguild%2F{guild_id}")).into_response();
    };
    let session = match refresh_read_session(&state, &jar).await {
        Ok(session) => session,
        Err(ReadGuildAuthorizationError::LoginRequired) => {
            return Redirect::to(&format!("/login?redirect=%2Fguild%2F{guild_id}")).into_response();
        }
        Err(ReadGuildAuthorizationError::Unavailable) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(render_error_page(
                    &state,
                    Some(&existing_session),
                    "Guild Access Unavailable",
                    "Discord could not verify your current server access. Please try again.",
                )),
            )
                .into_response();
        }
    };
    if !session_can_manage_guild(&session, guild_id) {
        return (
            StatusCode::FORBIDDEN,
            Html(render_error_page(
                &state,
                Some(&session),
                "Guild Access Restricted",
                "You do not have dashboard access to that server.",
            )),
        )
            .into_response();
    }
    let Some(card) = load_guild_card(&state, &session, guild_id).await else {
        return (
            StatusCode::FORBIDDEN,
            Html(render_error_page(
                &state,
                Some(&session),
                "Guild Access Restricted",
                "You do not have dashboard access to that server.",
            )),
        )
            .into_response();
    };
    match card.bot_presence {
        BotGuildPresence::Present => {}
        BotGuildPresence::Missing => {
            return Html(render_install_required_page(&state, &session, &card)).into_response();
        }
        BotGuildPresence::Unavailable => {
            return Html(render_error_page(
                &state,
                Some(&session),
                "Bot Status Unavailable",
                "Discord did not return the bot's current server status. Please try again later.",
            ))
            .into_response();
        }
    }

    let active_tab = normalized_tab(query.tab.as_deref());
    let log_entity = parse_audit_entity_filter(query.log_entity.as_deref());
    let log_action = parse_audit_action_filter(query.log_action.as_deref());
    let log_page = query.log_page.unwrap_or(1).max(1);
    let (overview, active_section, modals, include_mutation_script) = if active_tab == "logs" {
        let logs_page = match state
            .persistence
            .list_dashboard_audit_logs(DashboardAuditLogQuery {
                scope: DashboardAuditScope::Guild,
                guild_id: Some(guild_id),
                entity_type: log_entity,
                action: log_action,
                page: log_page,
                page_size: 20,
            })
            .await
        {
            Ok(page) => page,
            Err(error) => {
                warn!(
                    guild_id,
                    ?error,
                    "failed to load guild dashboard audit logs"
                );
                DashboardAuditLogPage::empty(log_page, 20)
            }
        };
        (
            String::new(),
            render_audit_logs_section(
                &format!("/guild/{guild_id}"),
                &logs_page,
                log_entity,
                log_action,
            ),
            String::new(),
            false,
        )
    } else {
        let deployment = match state.persistence.deployment_settings_or_default().await {
            Ok(settings) => settings,
            Err(_) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Html(render_error_page(
                        &state,
                        Some(&session),
                        "Deployment Settings Unavailable",
                        "The effective guild state could not be determined. Please try again.",
                    )),
                )
                    .into_response();
            }
        };
        let Some(_) = state.persistence.guild_settings.as_ref() else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(render_error_page(
                    &state,
                    Some(&session),
                    "Guild Settings Unavailable",
                    "Guild settings could not be loaded. Please try again.",
                )),
            )
                .into_response();
        };
        let (settings, settings_persisted) = match state.persistence.guild_settings(guild_id).await
        {
            Ok(Some(settings)) => (settings, true),
            Ok(None) => (GuildSettings::for_guild(guild_id), false),
            Err(error) => {
                warn!(?error, guild_id, "failed to load guild settings page");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Html(render_error_page(
                        &state,
                        Some(&session),
                        "Guild Settings Unavailable",
                        "Guild settings could not be loaded. Please try again.",
                    )),
                )
                    .into_response();
            }
        };
        match active_tab {
            "modules" => {
                let states =
                    resolve_module_states(&state.module_catalog, &deployment, Some(&settings));
                let modals = state
                    .module_catalog
                    .entries
                    .iter()
                    .zip(&states)
                    .map(|(entry, resolved)| {
                        let current = settings
                            .modules
                            .get(entry.module.id)
                            .cloned()
                            .unwrap_or_default();
                        let fields = render_structured_fields(entry, &current.configuration);
                        render_guild_module_modal(
                            guild_id,
                            entry,
                            resolved,
                            &render_module_runtime_notice(entry.module.id),
                            &current,
                            &fields,
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let cards = render_module_summary_cards(
                    "guild",
                    &state.module_catalog,
                    &deployment,
                    Some(&settings),
                    &states,
                );
                (
                    String::new(),
                    format!(
                        "<section id=\"modules\" class=\"section-block\" data-testid=\"guild-modules-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Modules</p><h2>Guild Modules</h2></div><input id=\"module-filter\" data-testid=\"module-filter\" class=\"toolbar-search compact-search\" type=\"search\" aria-label=\"Search guild modules\" aria-describedby=\"module-filter-status module-filter-empty\" placeholder=\"Search modules\" oninput=\"filterModuleCards(this.value)\" /></div><p id=\"module-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"module-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No modules match this search.</p><div class=\"module-grid compact-grid compact-module-grid\">{cards}</div></section>"
                    ),
                    modals,
                    true,
                )
            }
            "commands" => {
                let states = resolve_command_states(
                    &state.module_catalog,
                    &state.command_catalog,
                    &deployment,
                    Some(&settings),
                );
                let cards = render_command_summary_cards(
                    "guild",
                    &state.command_catalog,
                    &deployment,
                    Some(&settings),
                    &states,
                );
                let sync_panel = if state.config.register_globally {
                    render_command_sync_panel(&build_unsupported_sync_panel(
                        "This bot is using global command registration. Run Sync Global Commands from the deployment page to refresh Discord.",
                    ))
                } else {
                    let store = load_command_sync_store(&state.persistence).await;
                    let (fingerprint, _) = dynamo_app::application_command_fingerprint_for_scope(
                        &deployment,
                        Some(&settings),
                    );
                    render_command_sync_panel(&build_command_sync_panel(
                        SyncScopeKind::Guild(guild_id),
                        &fingerprint,
                        store.guild(guild_id),
                        &state.config,
                    ))
                };
                let modals = render_guild_command_modals(
                    guild_id,
                    &state.command_catalog,
                    &settings,
                    &states,
                );
                (
                    String::new(),
                    format!(
                        "<section id=\"commands\" class=\"section-block\" data-testid=\"guild-commands-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Commands</p><h2>Guild Commands</h2></div><input id=\"command-filter\" data-testid=\"command-filter\" class=\"toolbar-search compact-search\" type=\"search\" aria-label=\"Search guild commands\" aria-describedby=\"command-filter-status command-filter-empty\" placeholder=\"Search commands\" oninput=\"filterCommandCards(this.value)\" /></div><p id=\"command-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"command-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No commands match this search and category.</p>{sync_panel}{tabs}<div class=\"module-grid command-grid compact-grid compact-command-grid\" data-testid=\"command-card-grid\">{cards}</div></section>",
                        tabs = render_command_category_tabs(&state.command_catalog)
                    ),
                    modals,
                    true,
                )
            }
            _ => {
                let modules =
                    resolve_module_states(&state.module_catalog, &deployment, Some(&settings));
                let commands = resolve_command_states(
                    &state.module_catalog,
                    &state.command_catalog,
                    &deployment,
                    Some(&settings),
                );
                let overview = render_overview_section(
                    &card.name,
                    "Guild-scoped module and command controls for this server.",
                    &[
                        (
                            "Modules Enabled",
                            count_enabled_modules(&modules).to_string(),
                        ),
                        (
                            "Commands Enabled",
                            count_enabled_commands(&commands).to_string(),
                        ),
                        ("Guild ID", guild_id.to_string()),
                    ],
                );
                let state_name = guild_settings_ui_state(&settings, settings_persisted);
                let panel = format!(
                    "<section id=\"overview\" class=\"panel section-block\" data-testid=\"guild-runtime-summary\" data-settings-state=\"{state_name}\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Overview</p><h2>Guild Summary</h2></div><span class=\"pill pill-success\">Bot Connected</span></div>{}<div class=\"grid two compact-grid-two\"><article class=\"panel info-panel compact-info-panel\"><h3>Server Info</h3><p>Guild ID <code>{guild_id}</code></p><p>Guild-specific settings override deployment defaults where enabled.</p></article><article class=\"panel info-panel compact-info-panel\"><h3>Runtime Notes</h3>{}</article></div></section>",
                    guild_settings_notice(state_name),
                    render_runtime_notices(&state.module_catalog)
                );
                (overview, panel, String::new(), false)
            }
        }
    };
    let script = if include_mutation_script {
        format!("<script>{}</script>", dashboard_script())
    } else {
        String::new()
    };
    let content = format!(
        "{}{modals}{script}",
        render_dashboard_page_shell(
            &overview,
            &render_section_tabs(&format!("/guild/{guild_id}"), active_tab),
            &active_section,
            active_tab
        )
    );

    Html(render_document(
        &state,
        Some(&session),
        &format!("Guild Settings: {}", card.name),
        "Guild-scoped module and command controls for this server.",
        Some(&format!("/guild/{guild_id}")),
        Some(active_tab),
        &content,
    ))
    .into_response()
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

fn guild_settings_notice(state: &str) -> &'static str {
    match state {
        "absent" => {
            "<p class=\"notice\" data-testid=\"guild-settings-absent\">No guild settings have been saved yet. Deployment defaults are shown.</p>"
        }
        "existing-empty" => {
            "<p class=\"notice\" data-testid=\"guild-settings-empty\">Guild settings are saved, but no module or command overrides are configured.</p>"
        }
        _ => {
            "<p class=\"notice\" data-testid=\"guild-settings-configured\">Saved guild overrides are applied below.</p>"
        }
    }
}

async fn fetch_application_info(
    http: &reqwest::Client,
    config: &DashboardConfig,
) -> anyhow::Result<DiscordApplicationInfo> {
    let request = http
        .get(format!("{DISCORD_API_BASE}/oauth2/applications/@me"))
        .header("Authorization", format!("Bot {}", config.bot_token));
    let response = execute_dashboard_http(request).await?.error_for_status()?;

    let payload: DiscordApplicationResponse = response.json().await?;
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

    Ok(DiscordApplicationInfo {
        id: payload.id,
        name: payload.name,
        icon: payload.icon,
        owner_user_id,
    })
}

async fn load_session(state: &DashboardState, jar: &CookieJar) -> Option<DashboardSession> {
    let session_id = jar.get(SESSION_COOKIE_NAME)?.value().to_string();
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

fn session_cookie_value(jar: &CookieJar) -> Option<String> {
    jar.get(SESSION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
}

fn is_session_expired(session: &DashboardSession) -> bool {
    session.expires_at <= chrono::Utc::now()
}

fn is_oauth_state_expired(pending: &PendingOauthState) -> bool {
    pending.created_at + chrono::Duration::minutes(OAUTH_STATE_TTL_MINUTES) <= chrono::Utc::now()
}

fn sanitize_redirect_target(target: Option<&str>) -> String {
    let candidate = target.unwrap_or("/selector").trim();
    if candidate.starts_with('/') && !candidate.starts_with("//") {
        candidate.to_string()
    } else {
        "/selector".to_string()
    }
}

fn random_token(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

fn session_cookie(session_id: &str) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE_NAME, session_id.to_string());
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    cookie
}

fn build_discord_authorize_url(state: &DashboardState, oauth_state: &str) -> String {
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

async fn exchange_oauth_code(
    state: &DashboardState,
    code: &str,
) -> Result<DashboardSession, anyhow::Error> {
    let redirect_uri = format!("{}/auth/discord/callback", state.config.public_base_url);
    let token_request = state
        .http
        .post(format!("{}/oauth2/token", state.discord_api_base))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .form(&[
            ("client_id", state.app_info.id.as_str()),
            ("client_secret", state.config.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ]);
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

    Ok(DashboardSession {
        user: DashboardUser {
            id: user.id.parse::<u64>()?,
            username: user.username,
            global_name: user.global_name,
            avatar: user.avatar,
        },
        guilds,
        access_token: token_payload.access_token,
        expires_at: chrono::Utc::now() + chrono::Duration::hours(SESSION_TTL_HOURS),
    })
}

async fn load_guild_cards(state: &DashboardState, session: &DashboardSession) -> Vec<GuildCard> {
    let manageable = session
        .guilds
        .iter()
        .filter(|guild| user_can_manage_guild(guild))
        .cloned()
        .collect::<Vec<_>>();

    let mut cards = stream::iter(manageable.into_iter().map(|guild| async move {
        let bot_presence = bot_is_in_guild(state, guild.id).await;
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
    sort_guild_cards(&mut cards);
    cards
}

/// Builds the one card needed by the guild-detail route without rechecking every
/// manageable guild merely to locate the requested one. The selector deliberately
/// continues to use `load_guild_cards`, which keeps its bounded concurrent lookup
/// and stable ordering contract.
async fn load_guild_card(
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

fn sort_guild_cards(cards: &mut [GuildCard]) {
    cards.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn session_can_manage_guild(session: &DashboardSession, guild_id: u64) -> bool {
    session
        .guilds
        .iter()
        .any(|guild| guild.id == guild_id && user_can_manage_guild(guild))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshSessionGuildsError {
    Unauthorized,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadGuildAuthorizationError {
    LoginRequired,
    Unavailable,
}

/// Refreshes the Discord guild grant without holding the session lock during I/O.
/// The token identity guard prevents an in-flight refresh from writing into a
/// replacement session that reused the same cookie key.
async fn refresh_session_guilds(
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
    let guilds_response = send_dashboard_http(state, guilds_request)
        .await
        .map_err(|error| {
            warn!(?error, "Discord guild authorization refresh request failed");
            RefreshSessionGuildsError::Unavailable
        })?;
    if guilds_response.status() == StatusCode::UNAUTHORIZED {
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
        warn!(status = %guilds_response.status(), "Discord guild authorization refresh was unavailable");
        return Err(RefreshSessionGuildsError::Unavailable);
    }
    let guilds: Vec<DashboardGuild> = guilds_response.json().await.map_err(|error| {
        warn!(
            ?error,
            "Discord guild authorization refresh response was invalid"
        );
        RefreshSessionGuildsError::Unavailable
    })?;

    let mut sessions = state.sessions.write().await;
    let Some(session) = sessions.get_mut(session_id) else {
        return Ok(None);
    };
    if session.access_token != access_token {
        return Ok(None);
    }
    session.guilds = guilds;
    Ok(Some(session.clone()))
}

async fn refresh_read_session(
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

fn user_can_manage_guild(guild: &DashboardGuild) -> bool {
    let Ok(bits) = guild.permissions.parse::<u64>() else {
        return false;
    };
    let administrator = 1 << 3;
    let manage_guild = 1 << 5;
    bits & administrator == administrator || bits & manage_guild == manage_guild
}

async fn bot_is_in_guild(state: &DashboardState, guild_id: u64) -> BotGuildPresence {
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

fn classify_bot_guild_status(status: StatusCode) -> BotGuildPresence {
    if status.is_success() {
        BotGuildPresence::Present
    } else if status == StatusCode::NOT_FOUND {
        BotGuildPresence::Missing
    } else {
        BotGuildPresence::Unavailable
    }
}

async fn send_dashboard_http(
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

async fn execute_dashboard_http(
    request: reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    Ok(request.send().await?)
}

fn build_bot_invite_url(state: &DashboardState, guild_id: u64) -> String {
    let mut url = Url::parse("https://discord.com/oauth2/authorize").expect("valid invite url");
    url.query_pairs_mut()
        .append_pair("client_id", &state.app_info.id)
        .append_pair("scope", "bot applications.commands")
        .append_pair("permissions", &state.config.invite_permissions.to_string())
        .append_pair("guild_id", &guild_id.to_string())
        .append_pair("disable_guild_select", "true");
    url.to_string()
}

fn guild_icon_url(guild: &DashboardGuild) -> Option<String> {
    guild.icon.as_ref().map(|icon| {
        format!(
            "https://cdn.discordapp.com/icons/{}/{}.png?size=128",
            guild.id, icon
        )
    })
}

fn user_avatar_url(user: &DashboardUser) -> Option<String> {
    user.avatar.as_ref().map(|avatar| {
        format!(
            "https://cdn.discordapp.com/avatars/{}/{}.png?size=128",
            user.id, avatar
        )
    })
}

fn user_is_dashboard_admin(state: &DashboardState, user: &DashboardUser) -> bool {
    let mut admin_ids: HashSet<u64> = state.config.admin_user_ids.iter().copied().collect();
    if let Some(owner_id) = state.app_info.owner_user_id {
        admin_ids.insert(owner_id);
    }

    admin_ids.contains(&user.id)
}

fn render_landing_page(state: &DashboardState, session: Option<&DashboardSession>) -> String {
    let content = format!(
        "<section class=\"hero\"><div><p class=\"eyebrow\">Discord OAuth Dashboard</p><h1>Manage Dynamo like a real multi-server control panel.</h1><p class=\"lede\">{intro}</p><div class=\"actions\">{primary_action}<a class=\"button button-secondary\" href=\"/healthz\">Health Check</a></div></div><div class=\"hero-card\"><dl><div><dt>Modules</dt><dd>{module_count}</dd></div><div><dt>Leaf Commands</dt><dd>{command_count}</dd></div><div><dt>Runtime Notes</dt><dd>{notice_count}</dd></div></dl></div></section><section class=\"grid two\"><article class=\"panel\"><h2>Server Selector</h2><p>Dyno-like server cards split between servers you can manage now and servers that still need the bot installed.</p></article><article class=\"panel\"><h2>Shared Runtime Guard</h2><p>Dashboard state, runtime checks, and command sync all resolve from the same module and command enablement rules.</p></article></section>{runtime_notices}",
        intro = if session.is_some() {
            "Open the server listing to choose a guild, or review the shared runtime state from this dashboard."
        } else {
            "Sign in with Discord, pick the servers you can manage, and adjust module and command behavior without touching the terminal."
        },
        primary_action = if session.is_some() {
            "<a class=\"button button-primary\" href=\"/selector\">Server Listing</a>"
        } else {
            "<a class=\"button button-primary\" href=\"/login\">Sign in with Discord</a>"
        },
        module_count = state.module_catalog.entries.len(),
        command_count = state.command_catalog.entries.len(),
        notice_count = count_runtime_notices(&state.module_catalog),
        runtime_notices = render_runtime_notices(&state.module_catalog),
    );

    render_document(
        state,
        session,
        &format!("{} Dashboard", state.app_info.name),
        "OAuth-protected control plane for Dynamo.",
        Some("/"),
        None,
        &content,
    )
}

fn render_selector_page(
    state: &DashboardState,
    session: &DashboardSession,
    guild_cards: &[GuildCard],
) -> String {
    let manageable_now = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Present)
        .count();
    let needs_install = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Missing)
        .count();
    let unavailable = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Unavailable)
        .count();
    let connected_markup = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Present)
        .map(render_guild_card)
        .collect::<Vec<_>>()
        .join("\n");
    let install_markup = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Missing)
        .map(render_guild_card)
        .collect::<Vec<_>>()
        .join("\n");
    let unavailable_markup = guild_cards
        .iter()
        .filter(|card| card.bot_presence == BotGuildPresence::Unavailable)
        .map(render_guild_card)
        .collect::<Vec<_>>()
        .join("\n");

    let content = format!(
        "<section class=\"hero compact dyno-hero\"><div><p class=\"eyebrow\">Server Listing</p><h1>Choose a server to manage.</h1><p class=\"lede\">Only guilds where your account has Manage Server or Administrator are shown. Connected servers can be configured immediately.</p><div class=\"actions\"><a class=\"button button-primary\" href=\"#connected-servers\">Connected Servers</a><a class=\"button button-secondary\" href=\"#install-required\">Needs Install</a></div></div><div class=\"hero-card\"><dl><div><dt>Manage Now</dt><dd>{manageable_now}</dd></div><div><dt>Needs Install</dt><dd>{needs_install}</dd></div><div><dt>Status Unavailable</dt><dd>{unavailable}</dd></div><div><dt>Total Eligible</dt><dd>{total}</dd></div></dl></div></section><section class=\"panel toolbar-panel\"><div class=\"toolbar\"><div><p class=\"eyebrow\">Guild Search</p><h2>Server Listing</h2></div><input class=\"toolbar-search\" id=\"guild-filter\" type=\"search\" aria-label=\"Search guilds\" aria-describedby=\"guild-filter-status guild-filter-empty\" placeholder=\"Search guilds\" oninput=\"filterGuildCards(this.value)\" /></div><p id=\"guild-filter-status\" class=\"filter-feedback\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\"></p><p id=\"guild-filter-empty\" class=\"filter-empty\" role=\"status\" aria-live=\"polite\" aria-atomic=\"true\" hidden>No servers match this search.</p></section><section id=\"connected-servers\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Connected</p><h2>Manageable Servers</h2></div><span class=\"pill pill-success\">{manageable_now}</span></div><div class=\"module-grid\">{connected_markup}</div></section><section id=\"install-required\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Install Required</p><h2>Servers Missing The Bot</h2></div><span class=\"pill pill-warn\">{needs_install}</span></div><div class=\"module-grid\">{install_markup}</div></section><section id=\"status-unavailable\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Unavailable</p><h2>Server Status Could Not Be Checked</h2></div><span class=\"pill\">{unavailable}</span></div><div class=\"module-grid\">{unavailable_markup}</div></section>",
        manageable_now = manageable_now,
        needs_install = needs_install,
        unavailable = unavailable,
        total = guild_cards.len(),
        connected_markup = if connected_markup.is_empty() {
            "<article class=\"panel empty-state\"><h3>No connected servers</h3><p>Invite the bot into one of your manageable servers to unlock guild settings here.</p></article>".to_string()
        } else {
            connected_markup
        },
        install_markup = if install_markup.is_empty() {
            "<article class=\"panel empty-state\"><h3>Nothing pending</h3><p>Every eligible server already has the bot installed.</p></article>".to_string()
        } else {
            install_markup
        },
        unavailable_markup = if unavailable_markup.is_empty() {
            "<article class=\"panel empty-state\"><h3>All statuses available</h3><p>Discord returned a current status for every eligible server.</p></article>".to_string()
        } else {
            unavailable_markup
        },
    );

    render_document(
        state,
        Some(session),
        "Server Selector",
        "Pick a guild and move into module-level controls.",
        Some("/selector"),
        None,
        &content,
    )
}

fn render_guild_card(card: &GuildCard) -> String {
    let badge = match card.bot_presence {
        BotGuildPresence::Present => "<span class=\"pill pill-success\">Connected</span>",
        BotGuildPresence::Missing => "<span class=\"pill pill-warn\">Install Required</span>",
        BotGuildPresence::Unavailable => "<span class=\"pill\">Status Unavailable</span>",
    };
    let action = match card.bot_presence {
        BotGuildPresence::Present => format!(
            "<a class=\"button button-primary card-action\" href=\"{}\">Manage Server</a>",
            card.manage_url
        ),
        BotGuildPresence::Missing => format!(
            "<a class=\"button button-secondary card-action\" href=\"{}\">Invite Bot</a>",
            card.invite_url
        ),
        BotGuildPresence::Unavailable => {
            "<a class=\"button button-secondary\" href=\"/selector\">Retry Status</a>".to_string()
        }
    };
    let media = card
        .icon_url
        .as_ref()
        .map(|url| {
            format!(
                "<img class=\"guild-avatar\" src=\"{}\" alt=\"{} icon\" />",
                escape_html(url),
                escape_html(&card.name)
            )
        })
        .unwrap_or_else(|| {
            format!(
                "<div class=\"guild-avatar guild-avatar-fallback\">{}</div>",
                escape_html(&initials(&card.name))
            )
        });

    format!(
        "<article class=\"panel guild-card\" data-guild-name=\"{data_name}\"><div class=\"guild-card-head\">{media}<div><h2>{name}</h2>{badge}</div></div><p>{description}</p><div class=\"guild-card-meta\"><span>Guild ID</span><code>{guild_id}</code></div>{action}</article>",
        data_name = escape_html(&card.name.to_ascii_lowercase()),
        media = media,
        name = escape_html(&card.name),
        badge = badge,
        description = match card.bot_presence {
            BotGuildPresence::Present => "Open guild-scoped module and command settings.",
            BotGuildPresence::Missing => {
                "The bot is not in this server yet. Install it first, then return here."
            }
            BotGuildPresence::Unavailable => {
                "Discord did not return the bot's current status. Try again later."
            }
        },
        guild_id = card.id,
        action = action,
    )
}

fn render_install_required_page(
    state: &DashboardState,
    session: &DashboardSession,
    guild: &GuildCard,
) -> String {
    let content = format!(
        "<section class=\"hero compact\"><div><p class=\"eyebrow\">Guild Setup</p><h1>{name} is not connected yet.</h1><p class=\"lede\">Install the bot into this server first. When the bot joins, this page will expose guild-level controls automatically.</p><div class=\"actions\"><a class=\"button button-primary\" href=\"{invite_url}\">Invite Bot</a><a class=\"button button-secondary\" href=\"/selector\">Back to Selector</a></div></div></section>",
        name = escape_html(&guild.name),
        invite_url = guild.invite_url,
    );

    render_document(
        state,
        Some(session),
        &format!("Install Bot: {}", guild.name),
        "This guild is eligible for management, but the bot has not been installed yet.",
        Some("/selector"),
        None,
        &content,
    )
}

fn render_error_page(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    title: &str,
    message: &str,
) -> String {
    let content = format!(
        "<section class=\"hero compact\" role=\"alert\"><div><p class=\"eyebrow\">Dashboard</p><h1>{}</h1><p class=\"lede\">{}</p><div class=\"actions\"><a class=\"button button-primary\" href=\"\">Retry</a><a class=\"button button-secondary\" href=\"/selector\">Server Selector</a><a class=\"button button-secondary\" href=\"/\">Home</a></div></div></section>",
        escape_html(title),
        message,
    );

    render_document(state, session, title, message, None, None, &content)
}

fn render_document(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    title: &str,
    subtitle: &str,
    active_path: Option<&str>,
    active_tab: Option<&str>,
    content: &str,
) -> String {
    let nav = render_nav(state, session, active_path, active_tab);
    let session_summary = session.map(render_session_summary).unwrap_or_else(|| {
        "<a class=\"button button-primary\" href=\"/login\">Sign in with Discord</a>".to_string()
    });
    let app_icon = state.app_info.icon.as_ref().map(|icon| {
        format!(
            "https://cdn.discordapp.com/app-icons/{}/{}.png?size=128",
            state.app_info.id, icon
        )
    });

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" /><title>{title}</title><style>{styles}</style></head><body><div class=\"backdrop\"></div><div class=\"app-shell\" data-testid=\"dashboard-shell\"><aside class=\"sidebar\"><div class=\"sidebar-brand\">{brand_media}<div><p class=\"eyebrow\">Dynamo</p><h1>{app_name}</h1></div></div><nav class=\"sidebar-nav\">{nav}</nav><div class=\"sidebar-footer\"><span class=\"sidebar-footnote\">Rust dashboard control plane</span></div></aside><main class=\"content-shell\"><header class=\"content-topbar\"><div class=\"content-topbar-copy\"><p class=\"eyebrow\">Control Plane</p><h2>{page_title}</h2><p class=\"lede\">{subtitle}</p></div><div class=\"content-topbar-right\"><div class=\"stat-strip\"><div class=\"stat\"><span>Modules</span><strong>{module_count}</strong></div><div class=\"stat\"><span>Commands</span><strong>{command_count}</strong></div></div><div class=\"session-box\">{session_summary}</div></div></header><section class=\"content-body\" data-testid=\"content-body\">{content}</section></main></div><script>{ui_script}</script></body></html>",
        title = escape_html(title),
        styles = dashboard_styles(),
        ui_script = dashboard_ui_script(),
        brand_media =
            app_icon
                .map(|url| format!(
                    "<img class=\"app-avatar\" src=\"{}\" alt=\"app icon\" />",
                    escape_html(&url)
                ))
                .unwrap_or_else(
                    || "<div class=\"app-avatar app-avatar-fallback\">DY</div>".to_string()
                ),
        app_name = escape_html(&state.app_info.name),
        nav = nav,
        session_summary = session_summary,
        page_title = escape_html(title),
        subtitle = escape_html(subtitle),
        module_count = state.module_catalog.entries.len(),
        command_count = state.command_catalog.entries.len(),
        content = content,
    )
}

fn render_nav(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    active_path: Option<&str>,
    active_tab: Option<&str>,
) -> String {
    let default_dashboard = "/";
    let show_section_nav = active_path
        .map(|path| path == "/deployment" || path.starts_with("/guild/"))
        .unwrap_or(false);
    let dashboard_admin = session
        .map(|session| user_is_dashboard_admin(state, &session.user))
        .unwrap_or(false);
    let server_listing_active = active_path == Some("/selector")
        || active_path.is_some_and(|path| path.starts_with("/guild/"));
    let mut items = vec![nav_link(
        "Dashboard",
        default_dashboard,
        active_path == Some("/"),
    )];
    if session.is_some() {
        items.push(nav_link(
            "Server Listing",
            "/selector",
            server_listing_active,
        ));
        if dashboard_admin {
            items.push(nav_link(
                "Deployment",
                "/deployment",
                active_path == Some("/deployment"),
            ));
        }
        if show_section_nav {
            let base_path = active_path.unwrap_or(default_dashboard);
            let subnav = [
                (
                    "Modules",
                    format!("{base_path}{}", page_query_for_tab("modules")),
                    active_tab == Some("modules"),
                ),
                (
                    "Commands",
                    format!("{base_path}{}", page_query_for_tab("commands")),
                    active_tab == Some("commands"),
                ),
                (
                    "Logs",
                    format!("{base_path}{}", page_query_for_tab("logs")),
                    active_tab == Some("logs"),
                ),
            ]
            .into_iter()
            .map(|(label, href, active)| nav_sub_link(label, &href, active))
            .collect::<Vec<_>>()
            .join("");
            items.push(format!("<div class=\"nav-submenu\">{subnav}</div>"));
        }
        items.push(nav_link("Logout", "/logout", false));
    } else {
        items.push(nav_link("Sign in", "/login", false));
    }

    items.join("")
}

fn render_section_tabs(base_path: &str, active_tab: &str) -> String {
    let tabs = [
        ("overview", "Overview"),
        ("modules", "Modules"),
        ("commands", "Commands"),
        ("logs", "Logs"),
    ]
    .into_iter()
    .map(|(tab, label)| {
        let href = format!("{base_path}{}", page_query_for_tab(tab));
        format!(
            "<a class=\"tab-button{}\" data-testid=\"page-tab-{tab}\" href=\"{href}\"{}>{label}</a>",
            if active_tab == tab { " active" } else { "" },
            if active_tab == tab { " aria-current=\"page\"" } else { "" },
            label = escape_html(label),
        )
    })
    .collect::<Vec<_>>()
    .join("");

    format!("<div class=\"tab-row page-tab-row\">{tabs}</div>")
}

fn nav_link(label: &str, href: &str, active: bool) -> String {
    format!(
        "<a class=\"nav-link{}\" href=\"{}\"{}>{}</a>",
        if active { " active" } else { "" },
        href,
        if active { " aria-current=\"page\"" } else { "" },
        escape_html(label)
    )
}

fn nav_sub_link(label: &str, href: &str, active: bool) -> String {
    format!(
        "<a class=\"nav-sub-link{}\" href=\"{}\"{}>{}</a>",
        if active { " active" } else { "" },
        href,
        if active { " aria-current=\"page\"" } else { "" },
        escape_html(label)
    )
}

fn render_session_summary(session: &DashboardSession) -> String {
    let avatar = user_avatar_url(&session.user)
        .map(|url| {
            format!(
                "<img class=\"user-avatar\" src=\"{}\" alt=\"user avatar\" />",
                escape_html(&url)
            )
        })
        .unwrap_or_else(|| {
            format!(
                "<div class=\"user-avatar user-avatar-fallback\">{}</div>",
                escape_html(&initials(
                    session
                        .user
                        .global_name
                        .as_deref()
                        .unwrap_or(&session.user.username)
                ))
            )
        });
    let display_name = session
        .user
        .global_name
        .as_deref()
        .unwrap_or(&session.user.username);

    format!(
        "<div class=\"session-summary\">{avatar}<div><strong>{display_name}</strong><span>{username}</span></div></div>",
        avatar = avatar,
        display_name = escape_html(display_name),
        username = escape_html(&session.user.username),
    )
}

fn dashboard_styles() -> &'static str {
    static STYLES: OnceLock<String> = OnceLock::new();
    STYLES
        .get_or_init(|| {
            let mut styles =
                String::with_capacity(FONT_FACE_CSS.len() + DASHBOARD_BASE_STYLES.len());
            styles.push_str(FONT_FACE_CSS);
            styles.push('\n');
            styles.push_str(DASHBOARD_BASE_STYLES);
            styles
        })
        .as_str()
}

const DASHBOARD_BASE_STYLES: &str = r#"
:root {
  --bg: #0c0f17;
  --sidebar: #0b0e15;
  --panel: #171b24;
  --panel-strong: #1f2430;
  --panel-border: rgba(255, 255, 255, 0.04);
  --text: #f8fafc;
  --muted: #7f8ba3;
  --accent: #dd2e53;
  --accent-text: #ff9aae;
  --accent-button: #bc173d;
  --accent-button-hover: #d61f4b;
  --accent-strong: #ff4d6d;
  --accent-soft: rgba(221, 46, 83, 0.16);
  --success: #48e5b2;
  --danger: #f97316;
  --shadow: 0 18px 48px rgba(0, 0, 0, 0.28);
}
* { box-sizing: border-box; }
html, body { margin: 0; min-height: 100%; background: var(--bg); color: var(--text); font-family: 'Fira Sans', 'Fira Sans Fallback', sans-serif; font-synthesis: none; }
body { position: relative; }
.backdrop {
  position: fixed; inset: 0;
  background:
    radial-gradient(circle at left bottom, rgba(221, 46, 83, 0.18), transparent 18%),
    radial-gradient(circle at top left, rgba(61, 84, 143, 0.12), transparent 24%),
    linear-gradient(180deg, #0b0e15, #0c0f17);
  pointer-events: none;
}
.app-shell { position: relative; display: grid; grid-template-columns: 252px minmax(0, 1fr); min-height: 100vh; }
.sidebar {
  position: sticky; top: 0; align-self: start; height: 100vh; padding: 24px 18px;
  background: rgba(8, 10, 16, 0.96); border-right: 1px solid rgba(255,255,255,0.05);
  display: flex; flex-direction: column; gap: 28px;
}
.content-shell { padding: 28px 28px 40px; min-width: 0; }
.content-topbar, .panel, section, article, details { border: 1px solid var(--panel-border); background: var(--panel); box-shadow: var(--shadow); }
.content-topbar {
  display: flex; justify-content: space-between; gap: 18px; align-items: center;
  padding: 18px 20px; border-radius: 18px; margin-bottom: 20px;
}
.content-topbar-right { display: flex; align-items: center; gap: 16px; flex-wrap: wrap; justify-content: end; }
.sidebar-brand, .session-summary, .guild-card-head { display: flex; align-items: center; gap: 14px; }
.app-avatar, .user-avatar, .guild-avatar, .app-avatar-fallback, .user-avatar-fallback, .guild-avatar-fallback {
  width: 56px; height: 56px; border-radius: 18px; object-fit: cover; flex: none;
  display: grid; place-items: center; font-family: 'Fira Code', 'Fira Code Fallback', monospace; font-weight: 700;
  background: linear-gradient(135deg, rgba(221, 46, 83, 0.22), rgba(61, 84, 143, 0.18));
  border: 1px solid rgba(255,255,255,0.06);
}
.eyebrow { margin: 0 0 6px; color: var(--accent-text); font-size: 12px; letter-spacing: 0.16em; text-transform: uppercase; font-family: 'Fira Code', 'Fira Code Fallback', monospace; }
h1, h2, h3, legend { margin: 0; font-family: 'Fira Code', 'Fira Code Fallback', monospace; }
.sidebar-nav { display: grid; gap: 8px; }
.nav-link {
  color: var(--muted); text-decoration: none; padding: 13px 14px; border-radius: 14px;
  transition: background-color 180ms ease, color 180ms ease, border-color 180ms ease;
  border: 1px solid transparent; cursor: pointer; font-weight: 600;
}
.nav-link.active { color: var(--text); background: rgba(221, 46, 83, 0.14); border-color: rgba(221,46,83,0.2); }
.nav-link:hover:not(.active) { color: var(--text); background: rgba(221, 46, 83, 0.06); border-color: rgba(221,46,83,0.10); }
.nav-submenu { display: grid; gap: 2px; margin: 0 0 4px 12px; padding: 0 0 0 8px; }
.nav-sub-link { color: var(--muted); text-decoration: none; padding: 7px 10px; border-radius: 8px; cursor: pointer; font-size: 0.90rem; }
.nav-sub-link.active { color: var(--accent-text); font-weight: 600; }
.nav-sub-link:hover:not(.active) { color: var(--text); background: rgba(221,46,83,0.04); }
.sidebar-footer { margin-top: auto; padding-top: 12px; border-top: 1px solid rgba(255,255,255,0.06); }
.sidebar-footnote { color: var(--muted); font-size: 12px; }
.lede { margin: 8px 0 0; color: var(--muted); max-width: 70ch; line-height: 1.6; }
.stat-strip, .hero-card dl { display: grid; grid-template-columns: repeat(2, minmax(120px, 1fr)); gap: 12px; }
.stat, .hero-card dl > div {
  padding: 14px 16px; border-radius: 16px; background: var(--panel-strong); border: 1px solid rgba(255,255,255,0.04);
}
.stat span, dt { display: block; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; }
.stat strong, dd { margin: 8px 0 0; font-size: 24px; font-weight: 700; }
.hero { display: grid; grid-template-columns: 1.6fr 1fr; gap: 16px; padding: 20px; border-radius: 18px; margin-bottom: 18px; }
.hero.compact { grid-template-columns: 1.4fr 0.8fr; }
.actions { display: flex; flex-wrap: wrap; gap: 12px; margin-top: 18px; }
.button {
  display: inline-flex; align-items: center; justify-content: center; text-decoration: none; cursor: pointer;
  padding: 12px 18px; border-radius: 10px; border: 1px solid transparent; font-weight: 700;
  transition: transform 180ms ease, background-color 180ms ease, border-color 180ms ease, color 180ms ease;
}
.button:hover { transform: translateY(-1px); }
.button-primary { background: var(--accent-button); color: #fff6fa; }
.button-primary:hover { background: var(--accent-button-hover); }
.button-secondary { background: var(--panel-strong); color: var(--text); border-color: rgba(255,255,255,0.06); }
.grid { display: grid; gap: 14px; }
.grid.two { grid-template-columns: repeat(2, minmax(0, 1fr)); margin-bottom: 16px; }
.grid.three { grid-template-columns: repeat(3, minmax(0, 1fr)); }
.panel, section, article, details { padding: 14px; border-radius: 14px; margin-bottom: 14px; }
.guild-card p, .panel p { color: var(--muted); line-height: 1.6; }
.pill {
  display: inline-flex; align-items: center; padding: 6px 10px; border-radius: 999px;
  font-size: 12px; font-family: 'Fira Code', 'Fira Code Fallback', monospace; border: 1px solid rgba(255,255,255,0.08);
}
.pill-success { color: #bbf7d0; background: rgba(72, 229, 178, 0.14); }
.pill-warn { color: #fdba74; background: rgba(249, 115, 22, 0.12); }
.toolbar-panel, .section-block { margin-bottom: 16px; }
.dashboard-page-shell { display: grid; gap: 0; }
.dashboard-page-shell-task-first { display: flex; flex-direction: column; }
.dashboard-page-shell-task-first .dashboard-page-overview { order: 1; }
.dashboard-page-shell-task-first .dashboard-page-tabs { order: 2; }
.dashboard-page-shell-task-first .dashboard-page-active { order: 3; }
.dashboard-page-overview, .dashboard-page-tabs, .dashboard-page-active { min-width: 0; }
.toolbar { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
.toolbar-search { width: min(100%, 320px); max-width: 320px; margin: 0; }
.toolbar-select { width: min(100%, 180px); margin: 0; }
.compact-toolbar { justify-content: flex-start; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }
.compact-search { max-width: 240px; height: 40px; padding: 10px 12px; }
.section-heading { display: flex; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 14px; flex-wrap: wrap; }
.section-heading > div { min-width: 0; flex: 1 1 220px; }
.section-heading .toolbar-search { flex: 1 1 220px; }
.compact-heading { margin-bottom: 12px; }
.module-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; }
.compact-module-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); }
.command-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); }
.compact-command-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); }
.summary-card-head, .detail-panel-head { display: flex; justify-content: space-between; align-items: start; gap: 12px; }
.detail-panel-status { display: flex; align-items: center; }
.summary-card { min-height: 156px; display: flex; flex-direction: column; justify-content: space-between; }
.summary-card:hover, .guild-card:hover, .info-panel:hover, .logs-panel:hover {
  border-color: rgba(221,46,83,0.12);
  background: #1a1f2a;
  box-shadow: 0 14px 28px rgba(0,0,0,0.18);
}
.summary-card h3, .detail-panel h2, .command-detail-card h3 { font-size: 0.95rem; line-height: 1.15; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
.summary-card p, .detail-panel p, .command-detail-card p { font-size: 0.88rem; margin: 8px 0 0; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.summary-card .state-summary { display: block; -webkit-line-clamp: unset; overflow: visible; overflow-wrap: anywhere; }
.summary-card-subtitle { color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; margin-top: 8px; }
.detail-panel p, .summary-card p, .info-panel p { color: var(--muted); }
.detail-meta { margin: 8px 0 0; }
.detail-stack { display: grid; gap: 18px; }
.command-detail-card { border: 1px solid rgba(255,255,255,0.06); background: var(--panel-strong); border-radius: 14px; padding: 16px; }
.summary-card-meta { display: inline-flex; align-items: center; gap: 8px; margin: 6px 10px 0 0; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; }
.summary-card-meta code { color: var(--text); background: rgba(255,255,255,0.04); padding: 3px 7px; border-radius: 8px; font-size: 12px; }
.guild-card-meta { display: flex; align-items: center; gap: 10px; margin: 14px 0 16px; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; }
.guild-card-meta code { color: var(--text); background: rgba(255,255,255,0.04); padding: 4px 8px; border-radius: 8px; }
.empty-state { min-height: 220px; display: flex; flex-direction: column; justify-content: center; }
.filter-feedback { margin: 0 0 12px; color: var(--muted); font-size: 0.88rem; }
.filter-empty { margin: 0 0 12px; color: var(--muted); }
.card-action { margin-top: 10px; }
.tab-row { display: flex; flex-wrap: wrap; gap: 8px; margin: 0 0 12px; }
.page-tab-row { margin-bottom: 14px; }
.tab-button {
  appearance: none; border: 1px solid rgba(255,255,255,0.06); background: var(--panel-strong); color: var(--muted);
  padding: 7px 11px; border-radius: 10px; font: inherit; font-size: 0.85rem; font-weight: 700; cursor: pointer; width: auto; margin: 0; min-height: 38px;
}
.tab-button.active, .tab-button:hover { color: var(--text); background: rgba(221, 46, 83, 0.16); border-color: rgba(221,46,83,0.24); }
.sync-panel {
  display: flex; justify-content: space-between; align-items: center; gap: 14px;
  padding: 14px 16px; margin: 0 0 14px; border-radius: 14px;
  border: 1px solid rgba(255,255,255,0.06); background: var(--panel-strong);
}
.sync-panel-copy { min-width: 0; }
.sync-panel h3 { margin: 0; font-size: 0.98rem; }
.sync-panel p { margin: 6px 0 0; font-size: 0.88rem; color: var(--muted); line-height: 1.55; }
.sync-panel-status { margin-top: 8px; font-size: 12px; color: var(--muted-soft); }
.sync-panel-actions { display: flex; align-items: center; gap: 10px; flex-shrink: 0; }
.sync-panel-ok { border-color: rgba(72,229,178,0.18); background: linear-gradient(180deg, rgba(72,229,178,0.08), rgba(21,24,32,0.92)); }
.sync-panel-warn { border-color: rgba(221,46,83,0.26); background: linear-gradient(180deg, rgba(221,46,83,0.10), rgba(21,24,32,0.92)); }
.sync-panel-info { border-color: rgba(6,138,221,0.22); background: linear-gradient(180deg, rgba(6,138,221,0.09), rgba(21,24,32,0.92)); }
.sync-panel-error { border-color: rgba(245,158,11,0.24); background: linear-gradient(180deg, rgba(245,158,11,0.08), rgba(21,24,32,0.92)); }
.sync-panel-muted { border-color: rgba(255,255,255,0.06); background: rgba(255,255,255,0.02); }
.toggle-switch { position: relative; display: inline-flex; width: 44px; height: 24px; align-items: center; cursor: pointer; }
.toggle-switch input { position: absolute; inset: 0; opacity: 0; margin: 0; cursor: pointer; }
.toggle-slider { width: 44px; height: 24px; border-radius: 999px; background: #2a313e; border: 1px solid rgba(255,255,255,0.06); position: relative; transition: background-color 150ms ease; }
.toggle-slider::after { content: ''; position: absolute; top: 2px; left: 2px; width: 18px; height: 18px; border-radius: 50%; background: #aab3c5; transition: transform 150ms ease, background-color 150ms ease; }
.toggle-switch input:checked + .toggle-slider { background: rgba(72, 229, 178, 0.24); }
.toggle-switch input:checked + .toggle-slider::after { transform: translateX(20px); background: var(--success); }
.settings-modal-overlay { position: fixed; inset: 0; background: rgba(7, 9, 14, 0.78); display: grid; place-items: center; padding: 18px; z-index: 50; }
.settings-modal-overlay[hidden] { display: none !important; }
.settings-modal { width: min(640px, calc(100vw - 24px)); max-width: 100%; max-height: min(82vh, 860px); overflow: auto; background: #11151e; border: 1px solid rgba(255,255,255,0.08); border-radius: 18px; box-shadow: 0 30px 80px rgba(0,0,0,0.45); outline: none; }
.settings-modal-head { display: flex; justify-content: space-between; align-items: center; gap: 12px; padding: 14px 16px 10px; position: sticky; top: 0; background: #11151e; z-index: 2; }
.settings-modal-body { padding: 0 16px 16px; }
.modal-close { width: auto; min-width: 40px; padding: 8px 12px; font-size: 24px; line-height: 1; background: transparent; color: var(--text); }
form, .advanced-json-form { margin-top: 12px; }
label, small, legend { color: var(--text); }
small, .section-help { color: var(--muted); font-size: 12px; line-height: 1.5; }
input, textarea, select, button {
  width: 100%; margin-top: 6px; margin-bottom: 0; border-radius: 10px; border: 1px solid rgba(255,255,255,0.06);
  background: #262b36; color: var(--text); padding: 10px 12px; font: inherit;
}
input[type='checkbox'] { width: auto; margin-right: 8px; }
button { width: auto; cursor: pointer; background: var(--accent-soft); color: #ffd5df; }
button:hover { background: rgba(221, 46, 83, 0.24); }
a:focus-visible, button:focus-visible, input:focus-visible, select:focus-visible, textarea:focus-visible, [tabindex]:focus-visible {
  outline: 3px solid #7dd3fc; outline-offset: 3px;
}
.toggle-switch input:focus-visible + .toggle-slider { outline: 3px solid #7dd3fc; outline-offset: 3px; }
fieldset { border: 1px solid rgba(255,255,255,0.06); border-radius: 14px; padding: 12px; margin-top: 12px; }
.settings-section > p { margin: 4px 0 0; }
.settings-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; margin-top: 10px; }
.settings-grid-basic { margin-bottom: 12px; }
.settings-field { display: grid; gap: 6px; align-content: start; }
.settings-field-span-2 { grid-column: 1 / -1; }
.settings-field-textarea textarea { min-height: 112px; resize: vertical; }
.toggle-field { display: flex; justify-content: space-between; align-items: start; gap: 12px; cursor: pointer; }
.toggle-field input { margin-top: 2px; }
.toggle-field-copy { display: grid; gap: 4px; }
.toggle-field-copy strong { font-size: 0.92rem; }
.modal-status-row { display: grid; gap: 8px; margin-top: 4px; }
.modal-actions { position: sticky; bottom: 0; display: flex; align-items: center; justify-content: flex-end; gap: 10px; margin-top: 14px; padding-top: 12px; background: linear-gradient(180deg, rgba(17,21,30,0), #11151e 24px); }
.modal-status { margin-right: auto; font-size: 12px; color: var(--muted); }
.modal-status[data-kind='success'], .card-status[data-kind='success'] { color: #bbf7d0; }
.modal-status[data-kind='error'], .card-status[data-kind='error'] { color: #fdba74; }
.card-status { font-size: 12px; color: var(--muted); min-height: 16px; }
.compact-actions { display: flex; align-items: center; gap: 8px; margin-top: 12px; }
.button-compact { padding: 8px 12px; font-size: 0.86rem; }
.button-disabled { opacity: 0.46; pointer-events: none; cursor: default; }
.compact-grid-two { gap: 12px; }
.compact-info-panel { min-height: 120px; }
.logs-panel { overflow-x: auto; }
.logs-table { width: 100%; border-collapse: collapse; table-layout: fixed; }
.logs-table th, .logs-table td { text-align: left; padding: 10px 12px; border-bottom: 1px solid rgba(255,255,255,0.06); vertical-align: top; font-size: 0.9rem; }
.logs-table th { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; }
.logs-table tbody tr:hover { background: rgba(255,255,255,0.02); }
.logs-mobile-list { display: none; gap: 10px; }
.audit-log-card { border: 1px solid rgba(255,255,255,0.06); border-radius: 14px; background: var(--panel-strong); padding: 12px; }
.audit-log-card-head { display: flex; align-items: start; justify-content: space-between; gap: 12px; margin-bottom: 10px; }
.audit-log-card-time { margin: 0; color: var(--muted); font-size: 12px; letter-spacing: 0.04em; }
.audit-log-card-fields { display: grid; gap: 10px; margin: 0; }
.audit-log-card-field { display: grid; gap: 4px; }
.audit-log-card-field-summary { padding-top: 4px; border-top: 1px solid rgba(255,255,255,0.06); }
.audit-log-card-field dt { color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; }
.audit-log-card-field dd { margin: 0; color: var(--text); line-height: 1.5; overflow-wrap: anywhere; }
.logs-mobile-empty { color: var(--muted); text-align: center; padding: 18px 12px; border: 1px dashed rgba(255,255,255,0.08); border-radius: 14px; }
.empty-cell { color: var(--muted); text-align: center; padding: 18px 12px !important; }
.logs-pagination { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-top: 10px; }
.page-count { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; }
details summary { cursor: pointer; color: var(--text); font-weight: 600; }
article { margin-top: 16px; }
a { color: var(--accent-text); }
.runtime-notice { border-color: rgba(249,115,22,0.22); background: rgba(124, 45, 18, 0.22); }
.content-body > section[id], .content-body > section.section-block { scroll-margin-top: 24px; }
@media (max-width: 1100px) {
  .app-shell { grid-template-columns: 1fr; }
  .sidebar { position: relative; height: auto; }
  .hero, .hero.compact, .grid.two, .grid.three, .module-grid, .command-grid, .compact-module-grid, .compact-command-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .toolbar { flex-direction: column; align-items: stretch; }
  .content-shell { padding: 20px; }
  .settings-grid { grid-template-columns: 1fr; }
}
@media (max-width: 820px) {
  .sidebar { padding: 14px; gap: 14px; }
  .sidebar-brand { gap: 10px; }
  .sidebar-brand .app-avatar, .sidebar-brand .app-avatar-fallback { width: 40px; height: 40px; border-radius: 13px; }
  .sidebar-brand h1 { font-size: 1.1rem; }
  .sidebar-nav { gap: 4px; }
  .nav-link { padding: 9px 10px; }
  .sidebar-footer { padding-top: 0; border-top: 0; }
  .sidebar-footnote { display: none; }
  .content-topbar, .hero, .hero.compact, .grid.two, .grid.three, .module-grid, .command-grid, .compact-module-grid, .compact-command-grid { grid-template-columns: 1fr; }
  .content-topbar { display: grid; align-items: stretch; }
  .content-topbar-right { justify-content: start; }
  .settings-modal { width: min(96vw, 640px); }
  .section-heading { align-items: stretch; }
  .section-heading .toolbar-search, .section-heading .compact-search { max-width: none; width: 100%; }
  .dashboard-page-shell-task-first { display: flex; flex-direction: column; }
  .dashboard-page-shell-task-first .dashboard-page-tabs { order: 1; }
  .dashboard-page-shell-task-first .dashboard-page-active { order: 2; }
  .dashboard-page-shell-task-first .dashboard-page-overview { order: 3; }
  .dashboard-page-shell-task-first .dashboard-page-overview .hero { padding: 16px; margin-top: 8px; }
  .dashboard-page-shell-task-first .dashboard-page-overview .hero h1 { font-size: 1.35rem; }
  .dashboard-page-shell-task-first .dashboard-page-overview .hero .lede { font-size: 0.92rem; }
  .logs-panel { overflow-x: visible; }
  .logs-table { display: none; }
  .logs-mobile-list { display: grid; }
  .logs-pagination { flex-wrap: wrap; }
}
@media (max-width: 560px) {
  .sync-panel { flex-direction: column; align-items: stretch; }
  .sync-panel-actions, .sync-panel-actions .button { width: 100%; }
  .empty-state { min-height: 120px; }
  .settings-modal-overlay { padding: 12px; align-items: start; overflow-y: auto; }
  .settings-modal-head { align-items: start; }
  .toggle-field { flex-direction: column; }
  .modal-actions { flex-direction: column-reverse; align-items: stretch; }
  .modal-actions .button { width: 100%; }
  .modal-status { margin-right: 0; min-height: 18px; }
}
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { transition: none !important; animation: none !important; }
}
"#;

fn dashboard_ui_script() -> &'static str {
    r#"
function filterGuildCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-guild-name]');
  let visibleCount = 0;
  for (const card of cards) {
    const guildName = card.getAttribute('data-guild-name') || '';
    const visible = guildName.includes(value);
    card.style.display = visible ? '' : 'none';
    if (visible) visibleCount += 1;
  }
  updateFilterFeedback('guild-filter-status', 'guild-filter-empty', visibleCount, 'server');
}

function filterModuleCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-module-name]');
  let visibleCount = 0;
  for (const card of cards) {
    const moduleName = card.getAttribute('data-module-name') || '';
    const visible = moduleName.includes(value);
    card.style.display = visible ? '' : 'none';
    if (visible) visibleCount += 1;
  }
  updateFilterFeedback('module-filter-status', 'module-filter-empty', visibleCount, 'module');
}

function filterCommandCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-command-name]');
  let visibleCount = 0;
  for (const card of cards) {
    const commandName = card.getAttribute('data-command-name') || '';
    const category = window.__activeCommandCategory || 'all';
    const categoryMatch = category === 'all' || card.getAttribute('data-command-category') === category;
    const visible = commandName.includes(value) && categoryMatch;
    card.style.display = visible ? '' : 'none';
    if (visible) visibleCount += 1;
  }
  updateFilterFeedback('command-filter-status', 'command-filter-empty', visibleCount, 'command');
}

function updateFilterFeedback(statusId, emptyId, visibleCount, itemLabel) {
  const status = document.getElementById(statusId);
  const empty = document.getElementById(emptyId);
  const noun = `${itemLabel}${visibleCount === 1 ? '' : 's'}`;
  if (status) status.textContent = `Showing ${visibleCount} ${noun}.`;
  if (empty) empty.hidden = visibleCount !== 0;
}

function setCommandCategory(category, button) {
  window.__activeCommandCategory = category;
  document.querySelectorAll('.command-category-row .command-tab, .command-category-row .tab-button').forEach((item) => {
    item.classList.remove('active');
    item.setAttribute('aria-pressed', 'false');
  });
  if (button) {
    button.classList.add('active');
    button.setAttribute('aria-pressed', 'true');
  }
  const currentSearch = document.getElementById('command-filter');
  filterCommandCards(currentSearch ? currentSearch.value : '');
}
"#
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn display_name(user: &DashboardUser) -> String {
    user.global_name
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| user.username.clone())
}

fn count_runtime_notices(catalog: &ModuleCatalog) -> usize {
    catalog
        .entries
        .iter()
        .filter(|entry| runtime_notice_text(entry.module.id).is_some())
        .count()
}

fn parse_u64_list_env(key: &str) -> Result<Vec<u64>, anyhow::Error> {
    let Some(raw) = env::var(key).ok() else {
        return Ok(Vec::new());
    };

    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| anyhow::anyhow!("{key} must contain valid u64 values: {error}"))
        })
        .collect()
}

fn parse_bool_value(key: &str, value: &str) -> Result<bool, anyhow::Error> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("{key} must be one of true/false/1/0/yes/no/on/off"),
    }
}

fn parse_u64_env(key: &str, default: u64) -> Result<u64, anyhow::Error> {
    match env::var(key) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|error| anyhow::anyhow!("{key} must be a valid u64: {error}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(anyhow::anyhow!("{key} could not be read: {error}")),
    }
}

fn deserialize_u64_from_discord_id<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DiscordId {
        String(String),
        Number(u64),
    }

    match DiscordId::deserialize(deserializer)? {
        DiscordId::String(value) => value.parse::<u64>().map_err(serde::de::Error::custom),
        DiscordId::Number(value) => Ok(value),
    }
}

#[derive(Debug, Deserialize)]
struct DiscordApplicationResponse {
    id: String,
    name: String,
    icon: Option<String>,
    owner: Option<DiscordOwner>,
    team: Option<DiscordTeam>,
}

#[derive(Debug, Deserialize)]
struct DiscordOwner {
    id: String,
}

#[derive(Debug, Deserialize)]
struct DiscordTeam {
    owner_user_id: String,
}

#[derive(Debug, Deserialize)]
struct DiscordTokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct DiscordOAuthUser {
    id: String,
    username: String,
    global_name: Option<String>,
    avatar: Option<String>,
}

fn render_runtime_notices(catalog: &ModuleCatalog) -> String {
    let notices = catalog
        .entries
        .iter()
        .filter_map(|entry| {
            runtime_notice_text(entry.module.id).map(|note| (entry.module.display_name, note))
        })
        .map(|(display_name, note)| {
            format!(
                "<li><strong>{}</strong>: {}</li>",
                escape_html(display_name),
                escape_html(note)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    if notices.is_empty() {
        String::new()
    } else {
        format!(
            "<section><h2>Runtime Notices</h2><ul>{}</ul></section>",
            notices
        )
    }
}

fn render_deployment_module_modal(
    entry: &ModuleCatalogEntry,
    resolved: &ResolvedModuleState,
    runtime_notice: &str,
    current: &DeploymentModuleSettings,
) -> String {
    render_settings_modal(
        &modal_id_for_module("deployment", entry.module.id),
        entry.module.display_name,
        &format!(
            "<div class=\"modal-status-row\"><p class=\"detail-meta\"><strong>Status:</strong> {}</p>{}</div><form class=\"settings-form\" data-testid=\"deployment-module-form-{}\" onsubmit=\"return patchDeploymentModule(event, '{}')\"><div class=\"settings-grid settings-grid-basic\"><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Installed</strong><small>Controls whether this module is installed deployment-wide.</small></span><input type=\"checkbox\" name=\"installed\" {}/></label></div><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Enabled</strong><small>Controls whether this module is enabled by default.</small></span><input type=\"checkbox\" name=\"enabled\" {}/></label></div></div><div class=\"modal-actions\"><button class=\"button button-secondary\" data-testid=\"cancel-settings-{}\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">Cancel</button><button class=\"button button-primary\" data-testid=\"save-settings-{}\" type=\"submit\">Save</button><span class=\"modal-status\" id=\"deployment-status-{status_key}\"></span></div></form>",
            render_deployment_status(resolved),
            runtime_notice,
            status_key(entry.module.id),
            escape_html(entry.module.id),
            if current.installed { "checked" } else { "" },
            if current.enabled { "checked" } else { "" },
            status_key(entry.module.id),
            status_key(entry.module.id),
            modal_id = modal_id_for_module("deployment", entry.module.id),
            status_key = escape_html(entry.module.id)
        ),
    )
}

fn render_guild_module_modal(
    guild_id: u64,
    entry: &ModuleCatalogEntry,
    resolved: &ResolvedModuleState,
    runtime_notice: &str,
    current: &GuildModuleSettings,
    structured_fields: &str,
) -> String {
    render_settings_modal(
        &modal_id_for_module("guild", entry.module.id),
        entry.module.display_name,
        &format!(
            "<div class=\"modal-status-row\"><p class=\"detail-meta\"><strong>Status:</strong> {}</p>{}</div><form class=\"settings-form\" data-testid=\"guild-module-form-{}\" onsubmit=\"return patchGuildModule(event, '{}', '{}')\"><div class=\"settings-grid settings-grid-basic\"><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Enabled in guild</strong><small>Overrides deployment defaults for this server.</small></span><input type=\"checkbox\" name=\"enabled\" {}/></label></div></div>{}<div class=\"modal-actions\"><button class=\"button button-secondary\" data-testid=\"cancel-settings-{}\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">Cancel</button><button class=\"button button-primary\" data-testid=\"save-settings-{}\" type=\"submit\">Save</button><span class=\"modal-status\" id=\"guild-status-{status_key}\"></span></div></form>",
            render_guild_status(resolved),
            runtime_notice,
            status_key(entry.module.id),
            guild_id,
            escape_html(entry.module.id),
            if current.enabled { "checked" } else { "" },
            structured_fields,
            status_key(entry.module.id),
            status_key(entry.module.id),
            modal_id = modal_id_for_module("guild", entry.module.id),
            status_key = escape_html(entry.module.id),
        ),
    )
}

fn render_deployment_command_modals(
    command_catalog: &CommandCatalog,
    settings: &DeploymentSettings,
    resolved_states: &[ResolvedCommandState],
) -> String {
    let resolved_by_id = resolved_states
        .iter()
        .map(|state| (state.command.id.as_str(), state))
        .collect::<HashMap<_, _>>();
    command_catalog
        .entries
        .iter()
        .map(|entry| {
            let current = settings
                .commands
                .get(&entry.command.id)
                .cloned()
                .unwrap_or_default();
            let resolved = resolved_by_id.get(entry.command.id.as_str()).copied();
            let structured_fields = render_command_structured_fields(entry, &current.configuration);
            render_settings_modal(
                &modal_id_for_command("deployment", &entry.command.id),
                &entry.command.display_name,
                &format!(
                    "<div class=\"modal-status-row\"><p class=\"detail-meta\"><strong>Status:</strong> {}</p></div><form class=\"settings-form\" data-testid=\"deployment-command-form-{}\" onsubmit=\"return patchDeploymentCommand(event, '{}')\"><div class=\"settings-grid settings-grid-basic\"><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Installed</strong><small>Controls whether the command is installed deployment-wide.</small></span><input type=\"checkbox\" name=\"installed\" {}/></label></div><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Enabled</strong><small>Controls whether the command is enabled by default.</small></span><input type=\"checkbox\" name=\"enabled\" {}/></label></div></div>{}<div class=\"modal-actions\"><button class=\"button button-secondary\" data-testid=\"cancel-settings-{}\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">Cancel</button><button class=\"button button-primary\" data-testid=\"save-settings-{}\" type=\"submit\">Save</button><span class=\"modal-status\" id=\"deployment-command-status-{status_key}\"></span></div></form>",
                    resolved
                        .map(render_deployment_command_status)
                        .unwrap_or_else(|| "unknown".to_string()),
                    status_key(&entry.command.id),
                    escape_html(&entry.command.id),
                    if current.installed { "checked" } else { "" },
                    if current.enabled { "checked" } else { "" },
                    structured_fields,
                    status_key(&entry.command.id),
                    status_key(&entry.command.id),
                    modal_id = modal_id_for_command("deployment", &entry.command.id),
                    status_key = status_key(&entry.command.id),
                ),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_guild_command_modals(
    guild_id: u64,
    command_catalog: &CommandCatalog,
    settings: &GuildSettings,
    resolved_states: &[ResolvedCommandState],
) -> String {
    let resolved_by_id = resolved_states
        .iter()
        .map(|state| (state.command.id.as_str(), state))
        .collect::<HashMap<_, _>>();
    command_catalog
        .entries
        .iter()
        .map(|entry| {
            let current = settings
                .commands
                .get(&entry.command.id)
                .cloned()
                .unwrap_or_default();
            let resolved = resolved_by_id.get(entry.command.id.as_str()).copied();
            let structured_fields = render_command_structured_fields(entry, &current.configuration);
            render_settings_modal(
                &modal_id_for_command("guild", &entry.command.id),
                &entry.command.display_name,
                &format!(
                    "<div class=\"modal-status-row\"><p class=\"detail-meta\"><strong>Status:</strong> {}</p></div><form class=\"settings-form\" data-testid=\"guild-command-form-{}\" onsubmit=\"return patchGuildCommand(event, '{}', '{}')\"><div class=\"settings-grid settings-grid-basic\"><div class=\"settings-field settings-field-span-2 settings-field-toggle\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>Enabled in guild</strong><small>Overrides deployment command defaults for this server.</small></span><input type=\"checkbox\" name=\"enabled\" {}/></label></div></div>{}<div class=\"modal-actions\"><button class=\"button button-secondary\" data-testid=\"cancel-settings-{}\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">Cancel</button><button class=\"button button-primary\" data-testid=\"save-settings-{}\" type=\"submit\">Save</button><span class=\"modal-status\" id=\"guild-command-status-{status_key}\"></span></div></form>",
                    resolved
                        .map(render_guild_command_status)
                        .unwrap_or_else(|| "unknown".to_string()),
                    status_key(&entry.command.id),
                    guild_id,
                    escape_html(&entry.command.id),
                    if current.enabled { "checked" } else { "" },
                    structured_fields,
                    status_key(&entry.command.id),
                    status_key(&entry.command.id),
                    modal_id = modal_id_for_command("guild", &entry.command.id),
                    status_key = status_key(&entry.command.id),
                ),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_settings_modal(modal_id: &str, title: &str, body: &str) -> String {
    format!(
        "<div id=\"{modal_id}\" class=\"settings-modal-overlay\" data-testid=\"settings-modal-{modal_testid}\" hidden onclick=\"dismissSettingsModal(event, '{modal_id}')\"><div class=\"settings-modal\" data-modal-root role=\"dialog\" aria-modal=\"true\" aria-labelledby=\"modal-title-{modal_id}\" tabindex=\"-1\" onclick=\"event.stopPropagation()\"><div class=\"settings-modal-head\"><div><p class=\"eyebrow\">Settings</p><h3 id=\"modal-title-{modal_id}\" title=\"{title}\">{title}</h3></div><button class=\"modal-close\" data-testid=\"modal-close-{modal_testid}\" type=\"button\" aria-label=\"Close settings\" onclick=\"closeSettingsModal('{modal_id}')\">×</button></div><div class=\"settings-modal-body\">{body}</div></div></div>",
        modal_id = modal_id,
        modal_testid = status_key(modal_id),
        title = escape_html(title),
        body = body,
    )
}

fn render_module_toggle(
    scope: &str,
    module_id: &str,
    _deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    resolved: &ResolvedModuleState,
) -> String {
    match scope {
        "guild" => format!(
            "<label class=\"toggle-switch\"><input data-testid=\"module-toggle-{testid}\" type=\"checkbox\" aria-label=\"Enable module {module_id} for this guild\" {checked} onchange=\"toggleGuildModule('{guild_id}', '{module_id}', this.checked, this)\" /><span class=\"toggle-slider\"></span></label>",
            testid = status_key(module_id),
            checked = if resolved.guild_enabled {
                "checked"
            } else {
                ""
            },
            guild_id = guild.map(|g| g.guild_id.to_string()).unwrap_or_default(),
            module_id = escape_html(module_id),
        ),
        _ => format!(
            "<label class=\"toggle-switch\"><input data-testid=\"module-toggle-{testid}\" type=\"checkbox\" aria-label=\"Enable module {module_id} deployment-wide\" {checked} onchange=\"toggleDeploymentModule('{module_id}', this.checked, this)\" /><span class=\"toggle-slider\"></span></label>",
            testid = status_key(module_id),
            checked = if resolved.deployment_enabled {
                "checked"
            } else {
                ""
            },
            module_id = escape_html(module_id),
        ),
    }
}

fn render_command_toggle(
    scope: &str,
    command_id: &str,
    _deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    resolved: &ResolvedCommandState,
) -> String {
    match scope {
        "guild" => format!(
            "<label class=\"toggle-switch\"><input data-testid=\"command-toggle-{testid}\" type=\"checkbox\" aria-label=\"Enable command {command_id} for this guild\" {checked} onchange=\"toggleGuildCommand('{guild_id}', '{command_id}', this.checked, this)\" /><span class=\"toggle-slider\"></span></label>",
            testid = status_key(command_id),
            checked = if resolved.guild_enabled {
                "checked"
            } else {
                ""
            },
            guild_id = guild.map(|g| g.guild_id.to_string()).unwrap_or_default(),
            command_id = escape_html(command_id),
        ),
        _ => format!(
            "<label class=\"toggle-switch\"><input data-testid=\"command-toggle-{testid}\" type=\"checkbox\" aria-label=\"Enable command {command_id} deployment-wide\" {checked} onchange=\"toggleDeploymentCommand('{command_id}', this.checked, this)\" /><span class=\"toggle-slider\"></span></label>",
            testid = status_key(command_id),
            checked = if resolved.deployment_enabled {
                "checked"
            } else {
                ""
            },
            command_id = escape_html(command_id),
        ),
    }
}

fn render_command_category_tabs(catalog: &CommandCatalog) -> String {
    let mut seen = HashSet::new();
    let mut tabs = vec![
        "<button class=\"tab-button active\" data-testid=\"command-tab-all\" type=\"button\" aria-pressed=\"true\" onclick=\"setCommandCategory('all', this)\">All</button>".to_string(),
    ];

    for entry in &catalog.entries {
        let key = command_category_key(entry);
        let label = command_category_label(entry);
        if seen.insert(key.clone()) {
            tabs.push(format!(
                "<button class=\"tab-button command-tab\" data-testid=\"command-tab-{key}\" type=\"button\" aria-pressed=\"false\" onclick=\"setCommandCategory('{key}', this)\">{label}</button>",
                key = escape_html(&key),
                label = escape_html(&label),
            ));
        }
    }

    format!(
        "<div class=\"tab-row command-category-row\">{}</div>",
        tabs.join("")
    )
}

fn render_command_sync_panel(panel: &CommandSyncPanel) -> String {
    let status_class = match panel.state {
        CommandSyncDisplayState::InSync => "sync-panel sync-panel-ok",
        CommandSyncDisplayState::Required => "sync-panel sync-panel-warn",
        CommandSyncDisplayState::Pending => "sync-panel sync-panel-info",
        CommandSyncDisplayState::Failed => "sync-panel sync-panel-error",
        CommandSyncDisplayState::Unsupported => "sync-panel sync-panel-muted",
    };
    let action = match (&panel.button_label, &panel.button_action) {
        (Some(label), Some(action)) => format!(
            "<button class=\"button button-primary button-compact\" type=\"button\" onclick=\"{action}\">{label}</button>",
            action = action,
            label = escape_html(label),
        ),
        _ => String::new(),
    };
    let status_text = panel
        .status_text
        .as_ref()
        .map(|value| escape_html(value))
        .unwrap_or_default();

    format!(
        "<div class=\"{status_class}\" data-testid=\"command-sync-panel\"><div class=\"sync-panel-copy\"><h3>{title}</h3><p>{message}</p><p class=\"sync-panel-status\"><span>{status_text}</span><span id=\"command-sync-inline-status\" class=\"card-status\"></span></p></div><div class=\"sync-panel-actions\">{action}</div></div>",
        status_class = status_class,
        title = escape_html(&panel.title),
        message = escape_html(&panel.message),
        status_text = status_text,
        action = action,
    )
}

fn render_audit_logs_section(
    base_path: &str,
    page: &DashboardAuditLogPage,
    entity_type: Option<DashboardAuditEntityType>,
    action: Option<DashboardAuditAction>,
) -> String {
    let rows = if page.entries.is_empty() {
        "<tr><td colspan=\"5\" class=\"empty-cell\">No dashboard audit events recorded yet.</td></tr>"
            .to_string()
    } else {
        page.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                format!(
                    "<tr data-testid=\"audit-log-row-{index}\"><td>{time}</td><td>{actor}</td><td>{target}</td><td>{action}</td><td>{summary}</td></tr>",
                    index = index,
                    time = escape_html(&entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
                    actor = escape_html(&format!("{} ({})", entry.actor_username, entry.actor_user_id)),
                    target = escape_html(&format!(
                        "{} / {}",
                        audit_entity_label(entry.entity_type),
                        entry.entity_id
                    )),
                    action = escape_html(audit_action_label(entry.action)),
                    summary = escape_html(&entry.summary),
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };

    let prev_link = if page.has_prev() {
        format!(
            "<a class=\"button button-secondary button-compact\" href=\"{base}{query}\" data-testid=\"logs-prev\">Previous</a>",
            base = base_path,
            query = page_query_for_logs(entity_type, action, page.page.saturating_sub(1)),
        )
    } else {
        "<span class=\"button button-secondary button-compact button-disabled\">Previous</span>"
            .to_string()
    };
    let next_link = if page.has_next() {
        format!(
            "<a class=\"button button-secondary button-compact\" href=\"{base}{query}\" data-testid=\"logs-next\">Next</a>",
            base = base_path,
            query = page_query_for_logs(entity_type, action, page.page.saturating_add(1)),
        )
    } else {
        "<span class=\"button button-secondary button-compact button-disabled\">Next</span>"
            .to_string()
    };

    format!(
        "<section class=\"section-block\" data-testid=\"logs-section\"><div class=\"section-heading compact-heading\"><div><p class=\"eyebrow\">Logs</p><h2>Dashboard Audit Trail</h2></div></div><form class=\"toolbar compact-toolbar\" method=\"get\" action=\"{base_path}\"><input type=\"hidden\" name=\"tab\" value=\"logs\" /><select name=\"log_entity\" class=\"toolbar-select\" data-testid=\"logs-entity-filter\"><option value=\"\" {all_entity}>All entities</option><option value=\"module\" {module_selected}>Modules</option><option value=\"command\" {command_selected}>Commands</option></select><select name=\"log_action\" class=\"toolbar-select\" data-testid=\"logs-action-filter\"><option value=\"\" {all_action}>All actions</option><option value=\"toggle\" {toggle_selected}>Toggles</option><option value=\"save_settings\" {save_selected}>Settings saves</option></select><button class=\"button button-secondary button-compact\" type=\"submit\">Apply</button></form><div class=\"panel logs-panel\"><table class=\"logs-table\" data-testid=\"logs-table\"><thead><tr><th>Time</th><th>Actor</th><th>Target</th><th>Action</th><th>Summary</th></tr></thead><tbody>{rows}</tbody></table><div class=\"logs-mobile-list\" data-testid=\"logs-mobile-list\">{mobile_cards}</div></div><div class=\"logs-pagination\"><span class=\"page-count\">Page {page_number} of {page_total}</span><div class=\"actions compact-actions\">{prev_link}{next_link}</div></div></section>",
        base_path = base_path,
        all_entity = if entity_type.is_none() {
            "selected"
        } else {
            ""
        },
        module_selected = if entity_type == Some(DashboardAuditEntityType::Module) {
            "selected"
        } else {
            ""
        },
        command_selected = if entity_type == Some(DashboardAuditEntityType::Command) {
            "selected"
        } else {
            ""
        },
        all_action = if action.is_none() { "selected" } else { "" },
        toggle_selected = if action == Some(DashboardAuditAction::Toggle) {
            "selected"
        } else {
            ""
        },
        save_selected = if action == Some(DashboardAuditAction::SaveSettings) {
            "selected"
        } else {
            ""
        },
        rows = rows,
        mobile_cards = render_audit_log_mobile_cards(page),
        page_number = page.page,
        page_total = page.total.max(1).div_ceil(page.page_size.max(1)),
        prev_link = prev_link,
        next_link = next_link,
    )
}

fn render_audit_log_mobile_cards(page: &DashboardAuditLogPage) -> String {
    if page.entries.is_empty() {
        return "<article class=\"logs-mobile-empty\" data-testid=\"logs-mobile-empty\">No dashboard audit events recorded yet.</article>".to_string();
    }

    page.entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "<article class=\"audit-log-card\" data-testid=\"audit-log-card-{index}\"><div class=\"audit-log-card-head\"><p class=\"audit-log-card-time\" data-log-field=\"time\">{time}</p><span class=\"pill\" data-log-field=\"action\">{action}</span></div><dl class=\"audit-log-card-fields\"><div class=\"audit-log-card-field\"><dt>Actor</dt><dd data-log-field=\"actor\">{actor}</dd></div><div class=\"audit-log-card-field\"><dt>Target</dt><dd data-log-field=\"target\">{target}</dd></div><div class=\"audit-log-card-field audit-log-card-field-summary\"><dt>Summary</dt><dd data-log-field=\"summary\">{summary}</dd></div></dl></article>",
                index = index,
                time = escape_html(&entry.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
                actor = escape_html(&format!("{} ({})", entry.actor_username, entry.actor_user_id)),
                target = escape_html(&format!(
                    "{} / {}",
                    audit_entity_label(entry.entity_type),
                    entry.entity_id
                )),
                action = escape_html(audit_action_label(entry.action)),
                summary = escape_html(&entry.summary),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_dashboard_page_shell(
    overview: &str,
    page_tabs: &str,
    active_section: &str,
    active_tab: &str,
) -> String {
    if active_tab == "overview" {
        return format!(
            "<div class=\"dashboard-page-shell\"><div class=\"dashboard-page-overview\" data-testid=\"dashboard-page-overview\">{overview}</div><div class=\"dashboard-page-tabs\" data-testid=\"dashboard-page-tabs\">{page_tabs}</div><div class=\"dashboard-page-active\" data-testid=\"dashboard-page-active\">{active_section}</div></div>",
            overview = overview,
            page_tabs = page_tabs,
            active_section = active_section,
        );
    }

    format!(
        "<div class=\"dashboard-page-shell dashboard-page-shell-task-first\"><div class=\"dashboard-page-tabs\" data-testid=\"dashboard-page-tabs\">{page_tabs}</div><div class=\"dashboard-page-active\" data-testid=\"dashboard-page-active\">{active_section}</div></div>",
        page_tabs = page_tabs,
        active_section = active_section,
    )
}

fn audit_entity_label(entity_type: DashboardAuditEntityType) -> &'static str {
    match entity_type {
        DashboardAuditEntityType::Module => "Module",
        DashboardAuditEntityType::Command => "Command",
    }
}

fn audit_action_label(action: DashboardAuditAction) -> &'static str {
    match action {
        DashboardAuditAction::Toggle => "Toggle",
        DashboardAuditAction::SaveSettings => "Save settings",
    }
}

fn render_overview_section(title: &str, subtitle: &str, stats: &[(&str, String)]) -> String {
    let stat_markup = stats
        .iter()
        .map(|(label, value)| {
            format!(
                "<div class=\"stat\"><span>{}</span><strong>{}</strong></div>",
                escape_html(label),
                escape_html(value)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        "<section class=\"hero compact dyno-hero\"><div><p class=\"eyebrow\">Dashboard</p><h1>{}</h1><p class=\"lede\">{}</p></div><div class=\"hero-card\"><dl>{}</dl></div></section>",
        escape_html(title),
        escape_html(subtitle),
        stat_markup
    )
}

fn render_module_summary_cards(
    scope: &str,
    catalog: &ModuleCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    resolved_states: &[ResolvedModuleState],
) -> String {
    catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let toggle = render_module_toggle(scope, entry.module.id, deployment, guild, resolved);
            let state_summary = if scope == "guild" {
                format!(
                    "<p class=\"detail-meta state-summary\" data-testid=\"module-state-{testid}\">{}</p>",
                    escape_html(&render_guild_status(resolved)),
                    testid = status_key(entry.module.id),
                )
            } else {
                String::new()
            };
            format!(
                "<article class=\"panel summary-card module-card\" data-testid=\"module-card-{testid}\" data-module-name=\"{data_name}\"><div class=\"summary-card-head\"><h3 title=\"{name}\">{name}</h3>{toggle}</div><p title=\"{description}\">{description}</p>{state_summary}<div class=\"actions compact-actions\"><button class=\"button button-secondary button-compact\" data-testid=\"module-settings-button-{testid}\" type=\"button\" onclick=\"openSettingsModal('{modal_id}', this)\">Settings</button><span class=\"card-status\" id=\"card-status-module-{testid}\"></span></div></article>",
                testid = status_key(entry.module.id),
                data_name = escape_html(&entry.module.display_name.to_ascii_lowercase()),
                name = escape_html(entry.module.display_name),
                toggle = toggle,
                description = escape_html(entry.module.description),
                state_summary = state_summary,
                modal_id = modal_id_for_module(scope, entry.module.id),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_command_summary_cards(
    scope: &str,
    catalog: &CommandCatalog,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    resolved_states: &[ResolvedCommandState],
) -> String {
    catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let toggle = render_command_toggle(scope, &entry.command.id, deployment, guild, resolved);
            let state_summary = if scope == "guild" {
                format!(
                    "<p class=\"detail-meta state-summary\" data-testid=\"command-state-{testid}\">{}</p>",
                    escape_html(&render_guild_command_status(resolved)),
                    testid = status_key(&entry.command.id),
                )
            } else {
                String::new()
            };
            format!(
                "<article class=\"panel summary-card command-card\" data-testid=\"command-card-{testid}\" data-command-name=\"{command_name}\" data-command-category=\"{category_key}\"><div class=\"summary-card-head\"><h3 title=\"{display_name}\">{display_name}</h3>{toggle}</div><p title=\"{description}\">{description}</p><div class=\"summary-card-subtitle\">{category_label}</div>{state_summary}<div class=\"actions compact-actions\"><button class=\"button button-secondary button-compact\" data-testid=\"command-settings-button-{testid}\" type=\"button\" onclick=\"openSettingsModal('{modal_id}', this)\">Settings</button><span class=\"card-status\" id=\"card-status-command-{testid}\"></span></div></article>",
                testid = status_key(&entry.command.id),
                command_name = escape_html(&entry.command.display_name.to_ascii_lowercase()),
                category_key = escape_html(&command_category_key(entry)),
                display_name = escape_html(&entry.command.display_name),
                toggle = toggle,
                description = escape_html(entry.command.description.as_deref().unwrap_or("No description provided.")),
                category_label = escape_html(&command_category_label(entry)),
                state_summary = state_summary,
                modal_id = modal_id_for_command(scope, &entry.command.id),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn command_category_key(entry: &CommandCatalogEntry) -> String {
    entry
        .command
        .category
        .clone()
        .unwrap_or_else(|| {
            module_category_label_from_name(entry.command.module_display_name).to_string()
        })
        .to_ascii_lowercase()
        .replace(' ', "-")
}

fn command_category_label(entry: &CommandCatalogEntry) -> String {
    entry
        .command
        .category
        .clone()
        .unwrap_or_else(|| entry.command.module_display_name.to_string())
}

fn module_category_label_from_name(name: &str) -> &str {
    name
}

fn modal_id_for_module(scope: &str, module_id: &str) -> String {
    format!("modal-{}-module-{}", scope, status_key(module_id))
}

fn modal_id_for_command(scope: &str, command_id: &str) -> String {
    format!("modal-{}-command-{}", scope, status_key(command_id))
}

fn count_enabled_modules(states: &[ResolvedModuleState]) -> usize {
    states
        .iter()
        .filter(|state| state.effective_enabled)
        .count()
}

fn count_enabled_commands(states: &[ResolvedCommandState]) -> usize {
    states
        .iter()
        .filter(|state| state.effective_enabled)
        .count()
}

fn render_module_runtime_notice(module_id: &str) -> String {
    runtime_notice_text(module_id)
        .map(|note| {
            format!(
                "<p style=\"padding:8px 12px; border:1px solid #d99; background:#fff6f6\"><strong>Runtime notice:</strong> {}</p>",
                escape_html(note)
            )
        })
        .unwrap_or_default()
}

fn runtime_notice_text(module_id: &str) -> Option<&'static str> {
    let _ = module_id;
    None
}

fn render_structured_fields(entry: &ModuleCatalogEntry, configuration: &Value) -> String {
    render_settings_sections(
        &entry.settings,
        configuration,
        "<p>No configurable fields for this module.</p>",
    )
}

fn render_settings_sections(
    settings: &SettingsSchema,
    configuration: &Value,
    empty_markup: &str,
) -> String {
    let fields = settings
        .sections
        .iter()
        .map(|section| {
            let rendered_fields = section
                .fields
                .iter()
                .map(|field| render_field(field, configuration))
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "<fieldset class=\"settings-section\"><legend>{}</legend><p class=\"section-help\">{}</p><div class=\"settings-grid\">{}</div></fieldset>",
                escape_html(section.title),
                escape_html(section.description.unwrap_or("")),
                rendered_fields
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    if fields.is_empty() {
        empty_markup.to_string()
    } else {
        fields
    }
}

fn render_command_structured_fields(entry: &CommandCatalogEntry, configuration: &Value) -> String {
    render_settings_sections(
        &entry.settings,
        configuration,
        "<p>No configurable fields for this command.</p>",
    )
}

fn render_field(field: &SettingsField, configuration: &Value) -> String {
    let testid = status_key(field.key);
    let control_id = format!("field-{testid}-control");
    let help_id = format!("field-{testid}-help");
    let help_text = field
        .help_text
        .map(escape_html)
        .map(|text| format!("<small id=\"{help_id}\">{text}</small>"))
        .unwrap_or_default();
    let described_by = if field.help_text.is_some() {
        format!(" aria-describedby=\"{help_id}\"")
    } else {
        String::new()
    };
    let required = if field.required { "required" } else { "" };
    let field_key = escape_html(field.key);
    let field_label = escape_html(field.label);

    match &field.kind {
        SettingsFieldKind::Toggle => {
            let checked = field_bool_value(configuration, field.key).unwrap_or(false);
            format!(
                "<div class=\"settings-field settings-field-span-2 settings-field-toggle\" data-testid=\"field-{testid}\"><label class=\"toggle-field\"><span class=\"toggle-field-copy\"><strong>{label}</strong>{help_text}</span><input type=\"checkbox\" data-setting-key=\"{key}\" data-setting-kind=\"toggle\" {checked}/></label></div>",
                testid = status_key(field.key),
                key = field_key,
                label = field_label,
                checked = if checked { "checked" } else { "" },
                help_text = help_text,
            )
        }
        SettingsFieldKind::Integer { min, max } => {
            let value = field_string_value(configuration, field.key);
            let min_attr = min
                .map(|value| format!(" min=\"{value}\""))
                .unwrap_or_default();
            let max_attr = max
                .map(|value| format!(" max=\"{value}\""))
                .unwrap_or_default();
            format!(
                "<div class=\"settings-field\" data-testid=\"field-{testid}\"><label for=\"{control_id}\">{label}</label>{help_text}<input id=\"{control_id}\" type=\"number\" data-setting-key=\"{key}\" data-setting-kind=\"integer\" value=\"{value}\"{min_attr}{max_attr}{described_by} {required}/></div>",
                testid = testid,
                control_id = control_id,
                described_by = described_by,
                label = field_label,
                help_text = help_text,
                key = field_key,
                value = escape_html(&value.unwrap_or_default()),
                min_attr = min_attr,
                max_attr = max_attr,
                required = required,
            )
        }
        SettingsFieldKind::Text => {
            let value = field_string_value(configuration, field.key).unwrap_or_default();
            if value.len() > 40 || value.starts_with('[') || value.starts_with('{') {
                format!(
                    "<div class=\"settings-field settings-field-span-2 settings-field-textarea\" data-testid=\"field-{testid}\"><label for=\"{control_id}\">{label}</label>{help_text}<textarea id=\"{control_id}\" data-setting-key=\"{key}\" data-setting-kind=\"text\" rows=\"4\" cols=\"80\"{described_by} {required}>{value}</textarea></div>",
                    testid = testid,
                    control_id = control_id,
                    described_by = described_by,
                    label = field_label,
                    help_text = help_text,
                    key = field_key,
                    required = required,
                    value = escape_html(&value),
                )
            } else {
                format!(
                    "<div class=\"settings-field\" data-testid=\"field-{testid}\"><label for=\"{control_id}\">{label}</label>{help_text}<input id=\"{control_id}\" type=\"text\" data-setting-key=\"{key}\" data-setting-kind=\"text\" value=\"{value}\"{described_by} {required}/></div>",
                    testid = testid,
                    control_id = control_id,
                    described_by = described_by,
                    label = field_label,
                    help_text = help_text,
                    key = field_key,
                    value = escape_html(&value),
                    required = required,
                )
            }
        }
        SettingsFieldKind::Select { options } => {
            let current = field_string_value(configuration, field.key).unwrap_or_default();
            let options = options
                .iter()
                .map(|option| {
                    format!(
                        "<option value=\"{value}\" {selected}>{label}</option>",
                        value = escape_html(option.value),
                        selected = if current == option.value {
                            "selected"
                        } else {
                            ""
                        },
                        label = escape_html(option.label),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "<div class=\"settings-field\" data-testid=\"field-{testid}\"><label for=\"{control_id}\">{label}</label>{help_text}<select id=\"{control_id}\" data-setting-key=\"{key}\" data-setting-kind=\"select\"{described_by} {required}>{options}</select></div>",
                testid = testid,
                control_id = control_id,
                described_by = described_by,
                label = field_label,
                help_text = help_text,
                key = field_key,
                required = required,
                options = options,
            )
        }
    }
}

fn render_deployment_status(state: &ResolvedModuleState) -> String {
    format!(
        "installed: {} | deployment: {} | effective: {}",
        yes_no(state.installed),
        yes_no(state.deployment_enabled),
        yes_no(state.effective_enabled),
    )
}

fn render_guild_status(state: &ResolvedModuleState) -> String {
    format!(
        "Installed: {} | Deployment: {} | Local guild: {} | Effective: {} | {}",
        on_off(state.installed),
        on_off(state.deployment_enabled),
        on_off(state.guild_enabled),
        on_off(state.effective_enabled),
        module_blocker(state),
    )
}

fn render_deployment_command_status(state: &ResolvedCommandState) -> String {
    format!(
        "module: {} | installed: {} | deployment: {} | effective: {}",
        yes_no(state.module_effective_enabled),
        yes_no(state.installed),
        yes_no(state.deployment_enabled),
        yes_no(state.effective_enabled),
    )
}

fn render_guild_command_status(state: &ResolvedCommandState) -> String {
    format!(
        "Parent module: {} | Installed: {} | Deployment: {} | Local guild: {} | Effective: {} | {}",
        on_off(state.module_effective_enabled),
        on_off(state.installed),
        on_off(state.deployment_enabled),
        on_off(state.guild_enabled),
        on_off(state.effective_enabled),
        command_blocker(state),
    )
}

fn module_blocker(state: &ResolvedModuleState) -> &'static str {
    if !state.installed {
        "Blocked by deployment installation"
    } else if !state.deployment_enabled {
        "Blocked by deployment"
    } else if !state.guild_enabled {
        "Blocked by local guild setting"
    } else {
        "No blocker"
    }
}

fn command_blocker(state: &ResolvedCommandState) -> &'static str {
    if !state.module_effective_enabled {
        "Blocked by parent module"
    } else if !state.installed {
        "Blocked by deployment installation"
    } else if !state.deployment_enabled {
        "Blocked by deployment"
    } else if !state.guild_enabled {
        "Blocked by local guild setting"
    } else {
        "No blocker"
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "On" } else { "Off" }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn field_bool_value(configuration: &Value, key: &str) -> Option<bool> {
    value_at_path(configuration, key).and_then(Value::as_bool)
}

fn field_string_value(configuration: &Value, key: &str) -> Option<String> {
    let value = value_at_path(configuration, key)?;
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string_pretty(value).ok(),
    }
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn status_key(value: &str) -> String {
    value.replace(':', "-")
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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

async fn list_modules(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    if let Err(response) = require_api_session(&state, &jar).await {
        return response;
    }
    Json(state.module_catalog.clone()).into_response()
}

async fn list_default_module_states(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    if let Err(response) = require_api_session(&state, &jar).await {
        return response;
    }
    Json(resolve_module_states(
        &state.module_catalog,
        &DeploymentSettings::default(),
        None,
    ))
    .into_response()
}

async fn list_live_module_states(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    if let Err(response) = require_api_session(&state, &jar).await {
        return response;
    }
    let deployment_settings = match state.persistence.deployment_settings_or_default().await {
        Ok(settings) => settings,
        Err(_) => {
            warn!("failed to load live deployment module states");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "status": "error",
                    "message": "deployment settings are unavailable"
                })),
            )
                .into_response();
        }
    };

    Json(serde_json::json!({
        "status": "ok",
        "states": resolve_module_states(&state.module_catalog, &deployment_settings, None)
    }))
    .into_response()
}

#[allow(clippy::result_large_err)]
async fn require_api_session(
    state: &DashboardState,
    jar: &CookieJar,
) -> Result<DashboardSession, Response> {
    load_session(state, jar).await.ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(error_payload("dashboard login required".to_string())),
        )
            .into_response()
    })
}

#[allow(clippy::result_large_err)]
async fn require_api_admin(
    state: &DashboardState,
    jar: &CookieJar,
) -> Result<DashboardSession, Response> {
    let session = require_api_session(state, jar).await?;
    if user_is_dashboard_admin(state, &session.user) {
        Ok(session)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(error_payload(
                "deployment settings require dashboard admin access".to_string(),
            )),
        )
            .into_response())
    }
}

#[allow(clippy::result_large_err)]
async fn require_api_guild_access(
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

#[allow(clippy::result_large_err)]
async fn require_current_api_guild_access(
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

fn dashboard_actor_label(session: &DashboardSession) -> String {
    session
        .user
        .global_name
        .clone()
        .unwrap_or_else(|| session.user.username.clone())
}

fn build_audit_entry(
    session: &DashboardSession,
    scope: DashboardAuditScope,
    guild_id: Option<u64>,
    entity_type: DashboardAuditEntityType,
    entity_id: &str,
    action: DashboardAuditAction,
    summary: String,
) -> DashboardAuditLogEntry {
    DashboardAuditLogEntry {
        id: None,
        timestamp: chrono::Utc::now(),
        actor_user_id: session.user.id,
        actor_username: dashboard_actor_label(session),
        scope,
        guild_id,
        entity_type,
        entity_id: entity_id.to_string(),
        action,
        summary,
    }
}

async fn record_dashboard_audit_events(
    state: &DashboardState,
    entries: Vec<DashboardAuditLogEntry>,
) {
    for entry in entries {
        if let Err(error) = state.persistence.append_dashboard_audit_log(entry).await {
            warn!(?error, "failed to persist dashboard audit log entry");
        }
    }
}

#[derive(Debug, Deserialize)]
struct DeploymentModuleSettingsPatch {
    installed: Option<bool>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct DeploymentCommandSettingsPatch {
    installed: Option<bool>,
    enabled: Option<bool>,
    configuration: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GuildModuleSettingsPatch {
    enabled: Option<bool>,
    configuration: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GuildCommandSettingsPatch {
    enabled: Option<bool>,
    configuration: Option<serde_json::Value>,
}

async fn get_deployment_settings(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    if let Err(response) = require_api_admin(&state, &jar).await {
        return response;
    }
    match state.persistence.deployment_settings_or_default().await {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to load deployment settings: {error}"
            ))),
        )
            .into_response(),
    }
}

async fn patch_deployment_module_settings(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(module_id): Path<String>,
    Json(patch): Json<DeploymentModuleSettingsPatch>,
) -> impl IntoResponse {
    let session = match require_api_admin(&state, &jar).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !module_exists(&state.module_catalog, &module_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(error_payload(format!("unknown module id: {module_id}"))),
        )
            .into_response();
    }

    let Some(repo) = state.persistence.deployment_settings.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "deployment settings repository is not configured".to_string(),
            )),
        )
            .into_response();
    };

    let current_settings = match repo.get().await {
        Ok(settings) => settings,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_payload(format!(
                    "failed to load deployment settings: {error}"
                ))),
            )
                .into_response();
        }
    };

    let mut next = current_settings
        .modules
        .get(&module_id)
        .cloned()
        .unwrap_or(DeploymentModuleSettings::default());

    if let Some(installed) = patch.installed {
        next.installed = installed;
    }
    if let Some(enabled) = patch.enabled {
        next.enabled = enabled;
    }

    let current = current_settings
        .modules
        .get(&module_id)
        .cloned()
        .unwrap_or(DeploymentModuleSettings::default());

    match repo.upsert_module_settings(&module_id, next.clone()).await {
        Ok(settings) => {
            if current != next {
                record_dashboard_audit_events(
                    &state,
                    vec![build_audit_entry(
                        &session,
                        DashboardAuditScope::Deployment,
                        None,
                        DashboardAuditEntityType::Module,
                        &module_id,
                        DashboardAuditAction::Toggle,
                        format!(
                            "Updated deployment module {module_id}: installed={} enabled={}.",
                            next.installed, next.enabled
                        ),
                    )],
                )
                .await;
            }
            Json(settings).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist deployment settings: {error}"
            ))),
        )
            .into_response(),
    }
}

async fn patch_deployment_command_settings(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(command_id): Path<String>,
    Json(patch): Json<DeploymentCommandSettingsPatch>,
) -> impl IntoResponse {
    let session = match require_api_admin(&state, &jar).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !command_exists(&state.command_catalog, &command_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(error_payload(format!("unknown command id: {command_id}"))),
        )
            .into_response();
    }

    let Some(repo) = state.persistence.deployment_settings.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "deployment settings repository is not configured".to_string(),
            )),
        )
            .into_response();
    };

    let current_settings = match repo.get().await {
        Ok(settings) => settings,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_payload(format!(
                    "failed to load deployment settings: {error}"
                ))),
            )
                .into_response();
        }
    };

    let mut next = current_settings
        .commands
        .get(&command_id)
        .cloned()
        .unwrap_or_default();

    if let Some(installed) = patch.installed {
        next.installed = installed;
    }
    if let Some(enabled) = patch.enabled {
        next.enabled = enabled;
    }
    if let Some(configuration) = patch.configuration {
        next.configuration = configuration;
    }

    let current = current_settings
        .commands
        .get(&command_id)
        .cloned()
        .unwrap_or_default();

    match repo
        .upsert_command_settings(&command_id, next.clone())
        .await
    {
        Ok(settings) => {
            let mut entries = Vec::new();
            if current.installed != next.installed || current.enabled != next.enabled {
                entries.push(build_audit_entry(
                    &session,
                    DashboardAuditScope::Deployment,
                    None,
                    DashboardAuditEntityType::Command,
                    &command_id,
                    DashboardAuditAction::Toggle,
                    format!(
                        "Updated deployment command {command_id}: installed={} enabled={}.",
                        next.installed, next.enabled
                    ),
                ));
            }
            if current.configuration != next.configuration {
                entries.push(build_audit_entry(
                    &session,
                    DashboardAuditScope::Deployment,
                    None,
                    DashboardAuditEntityType::Command,
                    &command_id,
                    DashboardAuditAction::SaveSettings,
                    format!("Saved deployment settings for command {command_id}."),
                ));
            }
            if !entries.is_empty() {
                record_dashboard_audit_events(&state, entries).await;
            }
            Json(settings).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist deployment command settings: {error}"
            ))),
        )
            .into_response(),
    }
}

async fn get_guild_settings(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> impl IntoResponse {
    if let Err(response) = require_api_guild_access(&state, &jar, guild_id).await {
        return response;
    }
    let Some(repo) = state.persistence.guild_settings.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload("guild settings are unavailable".to_string())),
        )
            .into_response();
    };
    match repo.get(guild_id).await {
        Ok(Some(settings)) => {
            ([("x-dynamo-settings-state", "existing")], Json(settings)).into_response()
        }
        Ok(None) => (
            [("x-dynamo-settings-state", "absent")],
            Json(GuildSettings::for_guild(guild_id)),
        )
            .into_response(),
        Err(error) => {
            warn!(?error, guild_id, "failed to load guild settings API");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(error_payload("guild settings are unavailable".to_string())),
            )
                .into_response()
        }
    }
}

async fn patch_guild_module_settings(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path((guild_id, module_id)): Path<(u64, String)>,
    Json(patch): Json<GuildModuleSettingsPatch>,
) -> impl IntoResponse {
    if let Err(response) = require_api_session(&state, &jar).await {
        return response;
    }
    if !module_exists(&state.module_catalog, &module_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(error_payload(format!("unknown module id: {module_id}"))),
        )
            .into_response();
    }

    let Some(repo) = state.persistence.guild_settings.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "guild settings repository is not configured".to_string(),
            )),
        )
            .into_response();
    };

    let current_settings = match repo.get(guild_id).await {
        Ok(settings) => settings.unwrap_or_else(|| GuildSettings::for_guild(guild_id)),
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_payload(format!(
                    "failed to load guild settings: {error}"
                ))),
            )
                .into_response();
        }
    };

    let mut next = current_settings
        .modules
        .get(&module_id)
        .cloned()
        .unwrap_or(GuildModuleSettings::default());

    if let Some(enabled) = patch.enabled {
        next.enabled = enabled;
    }
    if let Some(configuration) = patch.configuration {
        next.configuration = configuration;
    }

    let current = current_settings
        .modules
        .get(&module_id)
        .cloned()
        .unwrap_or(GuildModuleSettings::default());
    let session = match require_current_api_guild_access(&state, &jar, guild_id).await {
        Ok(session) => session,
        Err(response) => return response,
    };

    match repo
        .upsert_module_settings(guild_id, &module_id, next)
        .await
    {
        Ok(settings) => {
            let next_state = settings
                .modules
                .get(&module_id)
                .cloned()
                .unwrap_or_default();
            let mut entries = Vec::new();
            if current.enabled != next_state.enabled {
                entries.push(build_audit_entry(
                    &session,
                    DashboardAuditScope::Guild,
                    Some(guild_id),
                    DashboardAuditEntityType::Module,
                    &module_id,
                    DashboardAuditAction::Toggle,
                    format!(
                        "Updated guild module {module_id}: enabled={}.",
                        next_state.enabled
                    ),
                ));
            }
            if current.configuration != next_state.configuration {
                entries.push(build_audit_entry(
                    &session,
                    DashboardAuditScope::Guild,
                    Some(guild_id),
                    DashboardAuditEntityType::Module,
                    &module_id,
                    DashboardAuditAction::SaveSettings,
                    format!("Saved guild settings for module {module_id}."),
                ));
            }
            if !entries.is_empty() {
                record_dashboard_audit_events(&state, entries).await;
            }
            Json(settings).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist guild settings: {error}"
            ))),
        )
            .into_response(),
    }
}

async fn patch_guild_command_settings(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path((guild_id, command_id)): Path<(u64, String)>,
    Json(patch): Json<GuildCommandSettingsPatch>,
) -> impl IntoResponse {
    if let Err(response) = require_api_session(&state, &jar).await {
        return response;
    }
    if !command_exists(&state.command_catalog, &command_id) {
        return (
            StatusCode::NOT_FOUND,
            Json(error_payload(format!("unknown command id: {command_id}"))),
        )
            .into_response();
    }

    let Some(repo) = state.persistence.guild_settings.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "guild settings repository is not configured".to_string(),
            )),
        )
            .into_response();
    };

    let current_settings = match repo.get(guild_id).await {
        Ok(settings) => settings.unwrap_or_else(|| GuildSettings::for_guild(guild_id)),
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_payload(format!(
                    "failed to load guild settings: {error}"
                ))),
            )
                .into_response();
        }
    };

    let mut next = current_settings
        .commands
        .get(&command_id)
        .cloned()
        .unwrap_or_default();

    if let Some(enabled) = patch.enabled {
        next.enabled = enabled;
    }
    if let Some(configuration) = patch.configuration {
        next.configuration = configuration;
    }

    let current = current_settings
        .commands
        .get(&command_id)
        .cloned()
        .unwrap_or_default();
    let session = match require_current_api_guild_access(&state, &jar, guild_id).await {
        Ok(session) => session,
        Err(response) => return response,
    };

    match repo
        .upsert_command_settings(guild_id, &command_id, next)
        .await
    {
        Ok(settings) => {
            let next_state = settings
                .commands
                .get(&command_id)
                .cloned()
                .unwrap_or_default();
            let mut entries = Vec::new();
            if current.enabled != next_state.enabled {
                entries.push(build_audit_entry(
                    &session,
                    DashboardAuditScope::Guild,
                    Some(guild_id),
                    DashboardAuditEntityType::Command,
                    &command_id,
                    DashboardAuditAction::Toggle,
                    format!(
                        "Updated guild command {command_id}: enabled={}.",
                        next_state.enabled
                    ),
                ));
            }
            if current.configuration != next_state.configuration {
                entries.push(build_audit_entry(
                    &session,
                    DashboardAuditScope::Guild,
                    Some(guild_id),
                    DashboardAuditEntityType::Command,
                    &command_id,
                    DashboardAuditAction::SaveSettings,
                    format!("Saved guild settings for command {command_id}."),
                ));
            }
            if !entries.is_empty() {
                record_dashboard_audit_events(&state, entries).await;
            }
            Json(settings).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist guild command settings: {error}"
            ))),
        )
            .into_response(),
    }
}

async fn post_deployment_command_sync(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let session = match require_api_admin(&state, &jar).await {
        Ok(session) => session,
        Err(response) => return response,
    };

    if !state.config.register_globally {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_payload(
                "Deployment command sync is only available while DISCORD_REGISTER_GLOBALLY=true."
                    .to_string(),
            )),
        )
            .into_response();
    }

    let mut sync_state = load_command_sync_store(&state.persistence).await;
    sync_state.global.request_sync(
        chrono::Utc::now(),
        Some(session.user.id),
        Some(display_name(&session.user)),
    );

    match save_command_sync_store(&state.persistence, &sync_state).await {
        Ok(()) => Json(serde_json::json!({
            "status": "ok",
            "message": "Global command sync requested."
        }))
        .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist command sync request: {error}"
            ))),
        )
            .into_response(),
    }
}

async fn post_guild_command_sync(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> impl IntoResponse {
    if let Err(response) = require_api_session(&state, &jar).await {
        return response;
    }

    if state.config.register_globally {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_payload(
                "Guild command sync is unavailable while global command registration is enabled."
                    .to_string(),
            )),
        )
            .into_response();
    }

    let mut sync_state = load_command_sync_store(&state.persistence).await;
    let session = match require_current_api_guild_access(&state, &jar, guild_id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    sync_state.guild_mut(guild_id).request_sync(
        chrono::Utc::now(),
        Some(session.user.id),
        Some(display_name(&session.user)),
    );

    match save_command_sync_store(&state.persistence, &sync_state).await {
        Ok(()) => Json(serde_json::json!({
            "status": "ok",
            "message": "Guild command sync requested."
        }))
        .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist command sync request: {error}"
            ))),
        )
            .into_response(),
    }
}

fn module_exists(catalog: &ModuleCatalog, module_id: &str) -> bool {
    catalog
        .entries
        .iter()
        .any(|entry| entry.module.id == module_id)
}

fn command_exists(catalog: &CommandCatalog, command_id: &str) -> bool {
    catalog.find_by_id(command_id).is_some()
}

fn error_payload(message: String) -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "message": message
    })
}

const DASHBOARD_MUTATION_TRANSPORT: &str = include_str!("../assets/dashboard-mutations.js");

fn dashboard_script() -> String {
    format!(
        "{DASHBOARD_MUTATION_TRANSPORT}\n{}",
        r#"
function setInlineStatus(id, message, kind = 'info') {
  const target = document.getElementById(id);
  if (!target) return;
  target.dataset.kind = kind;
  target.setAttribute('role', kind === 'error' ? 'alert' : 'status');
  target.setAttribute('aria-live', kind === 'error' ? 'assertive' : 'polite');
  target.setAttribute('aria-atomic', 'true');
  target.textContent = message || '';
}

function setUnknownStatus(id, message) {
  setInlineStatus(id, message, 'error');
  const target = document.getElementById(id);
  if (!target) return;
  const reload = document.createElement('button');
  reload.type = 'button';
  reload.className = 'button button-secondary button-compact';
  reload.textContent = 'Reload';
  reload.addEventListener('click', () => window.location.reload());
  target.append(' ', reload);
}

function snapshotForm(form) {
  return Array.from(form.elements).map((element) => ({
    element,
    disabled: element.disabled,
    checked: element.type === 'checkbox' ? element.checked : undefined,
    value: element.type === 'checkbox' ? undefined : element.value,
  }));
}

function restoreForm(snapshot) {
  for (const item of snapshot) {
    if (item.checked !== undefined) item.element.checked = item.checked;
    if (item.value !== undefined) item.element.value = item.value;
  }
}

async function runFormMutation(form, request, statusId, reconcile) {
  if (form.dataset.dynamoMutationPending === 'true') return { kind: 'pending' };
  form.dataset.dynamoMutationPending = 'true';
  const snapshot = snapshotForm(form);
  for (const item of snapshot) item.element.disabled = true;
  try {
    const outcome = await request();
    if (outcome.kind === 'success') return outcome;
    if (outcome.kind === 'definite-failure') {
      setInlineStatus(statusId, `Error: ${outcome.message}`, 'error');
      return outcome;
    }
    setUnknownStatus(statusId, outcome.message);
    const refreshed = await reconcile();
    if (refreshed.kind === 'success') {
      setInlineStatus(statusId, 'Current values were reloaded after an uncertain save.', 'info');
    } else {
      restoreForm(snapshot);
      setUnknownStatus(statusId, 'The save outcome is unknown and current values could not be reloaded.');
    }
    return outcome;
  } finally {
    for (const item of snapshot) item.element.disabled = item.disabled;
    delete form.dataset.dynamoMutationPending;
  }
}

async function reconcileSettings(url, apply) {
  const outcome = await DynamoMutationTransport.request(fetch, url, { method: 'GET' });
  if (outcome.kind === 'success') apply(outcome.body);
  return outcome;
}

function setToggleConfirmed(input, value) {
  if (!input) return;
  input.checked = value;
  input.defaultChecked = value;
  input.dataset.dynamoConfirmed = String(value);
}

function toggleConfirmedValue(input) {
  if (!input) return false;
  if (input.dataset.dynamoConfirmed === undefined) input.dataset.dynamoConfirmed = String(input.defaultChecked);
  return input.dataset.dynamoConfirmed === 'true';
}

async function runToggleMutation(input, request, statusId) {
  if (!input || input.dataset.dynamoMutationPending === 'true') return false;
  const confirmed = toggleConfirmedValue(input);
  input.dataset.dynamoMutationPending = 'true';
  input.disabled = true;
  try {
    const outcome = await request();
    if (outcome.kind === 'success') {
      setToggleConfirmed(input, input.checked);
      setInlineStatus(statusId, 'Saved', 'success');
    } else if (outcome.kind === 'definite-failure') {
      setToggleConfirmed(input, confirmed);
      setInlineStatus(statusId, `Error: ${outcome.message}`, 'error');
    } else {
      setToggleConfirmed(input, confirmed);
      setUnknownStatus(statusId, outcome.message);
    }
  } finally {
    input.disabled = false;
    delete input.dataset.dynamoMutationPending;
  }
  return false;
}

const MODAL_FOCUSABLE_SELECTOR = 'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
const settingsModalState = {
  activeModalId: null,
  returnFocus: null,
};

function activeSettingsModal() {
  if (!settingsModalState.activeModalId) return null;
  return document.getElementById(settingsModalState.activeModalId);
}

function anyOpenSettingsModal() {
  return document.querySelector('.settings-modal-overlay:not([hidden])');
}

function findModalDialog(modal) {
  return modal ? modal.querySelector('[data-modal-root]') : null;
}

function isFocusableVisible(element) {
  return !!(element.offsetWidth || element.offsetHeight || element.getClientRects().length);
}

function modalFocusableElements(modal) {
  const dialog = findModalDialog(modal);
  if (!dialog) return [];
  return Array.from(dialog.querySelectorAll(MODAL_FOCUSABLE_SELECTOR)).filter((element) => isFocusableVisible(element));
}

function focusFirstModalControl(modal) {
  const dialog = findModalDialog(modal);
  if (!dialog) return;
  const focusable = modalFocusableElements(modal);
  const preferred = focusable.find((element) => element.matches('input, select, textarea, button:not(.modal-close)'));
  (preferred || focusable[0] || dialog).focus();
}

function openSettingsModal(modalId, trigger) {
  const modal = document.getElementById(modalId);
  if (!modal) return false;
  const openModal = anyOpenSettingsModal();
  if (openModal && openModal !== modal) {
    openModal.hidden = true;
  }
  settingsModalState.activeModalId = modalId;
  settingsModalState.returnFocus = trigger instanceof HTMLElement
    ? trigger
    : document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
  modal.hidden = false;
  document.body.style.overflow = 'hidden';
  window.requestAnimationFrame(() => focusFirstModalControl(modal));
  return false;
}

function closeSettingsModal(modalId) {
  const modal = document.getElementById(modalId);
  if (!modal) return false;
  const wasActive = settingsModalState.activeModalId === modalId;
  modal.hidden = true;
  if (!anyOpenSettingsModal()) {
    document.body.style.overflow = '';
  }
  if (wasActive) {
    settingsModalState.activeModalId = null;
    const returnFocus = settingsModalState.returnFocus;
    settingsModalState.returnFocus = null;
    if (returnFocus && typeof returnFocus.focus === 'function') {
      returnFocus.focus();
    }
  }
  return false;
}

function dismissSettingsModal(event, modalId) {
  if (event.target && event.target.id === modalId) {
    closeSettingsModal(modalId);
  }
  return false;
}

document.addEventListener('keydown', (event) => {
  const openModal = activeSettingsModal() || document.querySelector('.settings-modal-overlay:not([hidden])');
  if (!openModal) return;
  if (event.key === 'Escape') {
    event.preventDefault();
    closeSettingsModal(openModal.id);
    return;
  }
  if (event.key !== 'Tab') return;

  const dialog = findModalDialog(openModal);
  if (!dialog) return;
  const focusable = modalFocusableElements(openModal);
  if (!focusable.length) {
    event.preventDefault();
    dialog.focus();
    return;
  }

  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
});

async function toggleDeploymentModule(moduleId, enabled, input) {
  return runToggleMutation(input, () => DynamoMutationTransport.request(fetch, `/api/deployment-settings/${moduleId}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled }),
  }), `card-status-module-${statusKey(moduleId)}`);
}

async function patchDeploymentModule(event, moduleId) {
  event.preventDefault();
  const form = event.target;
  const body = {
    installed: form.installed.checked,
    enabled: form.enabled.checked,
  };
  const outcome = await runFormMutation(form, () => DynamoMutationTransport.request(fetch, `/api/deployment-settings/${moduleId}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  }), `deployment-status-${moduleId}`, () => reconcileSettings('/api/deployment-settings', (settings) => {
    const current = settings.modules?.[moduleId];
    if (current) { form.installed.checked = !!current.installed; form.enabled.checked = !!current.enabled; }
  }));
  if (outcome.kind === 'success') {
    setInlineStatus(`deployment-status-${moduleId}`, 'Saved', 'success');
    closeSettingsModal(`modal-deployment-module-${statusKey(moduleId)}`);
  }
  return false;
}

async function toggleDeploymentCommand(commandId, enabled, input) {
  return runToggleMutation(input, () => DynamoMutationTransport.request(fetch, `/api/deployment-command-settings/${encodeURIComponent(commandId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled }),
  }), `card-status-command-${statusKey(commandId)}`);
}

async function patchDeploymentCommand(event, commandId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = collectConfiguration(form);
  } catch (error) {
    setInlineStatus(`deployment-command-status-${statusKey(commandId)}`, `Error: ${error.message}`, 'error');
    return false;
  }

  const outcome = await runFormMutation(form, () => DynamoMutationTransport.request(fetch, `/api/deployment-command-settings/${encodeURIComponent(commandId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      installed: form.installed.checked,
      enabled: form.enabled.checked,
      configuration,
    }),
  }), `deployment-command-status-${statusKey(commandId)}`, () => reconcileSettings('/api/deployment-settings', (settings) => {
    const current = settings.commands?.[commandId];
    if (current) { form.installed.checked = !!current.installed; form.enabled.checked = !!current.enabled; }
  }));
  if (outcome.kind === 'success') {
    setInlineStatus(`deployment-command-status-${statusKey(commandId)}`, 'Saved', 'success');
    closeSettingsModal(`modal-deployment-command-${statusKey(commandId)}`);
  }
  return false;
}

function collectConfiguration(form) {
  const config = {};
  const fields = form.querySelectorAll('[data-setting-key]');
  for (const field of fields) {
    const key = field.dataset.settingKey;
    if (!key || key === '__empty') continue;

    const kind = field.dataset.settingKind;
    if (kind === 'toggle') {
      setPath(config, key, !!field.checked);
      continue;
    }

    const raw = (field.value ?? '').trim();
    if (raw === '') continue;

    if (kind === 'integer') {
      const parsed = Number.parseInt(raw, 10);
      if (Number.isNaN(parsed)) {
        throw new Error(`Invalid integer for ${key}`);
      }
      setPath(config, key, parsed);
      continue;
    }

    if ((raw.startsWith('[') && raw.endsWith(']')) || (raw.startsWith('{') && raw.endsWith('}'))) {
      setPath(config, key, JSON.parse(raw));
      continue;
    }

    setPath(config, key, raw);
  }

  return config;
}

function setPath(target, key, value) {
  const segments = key.split('.');
  let cursor = target;
  for (let i = 0; i < segments.length - 1; i += 1) {
    const segment = segments[i];
    if (typeof cursor[segment] !== 'object' || cursor[segment] === null || Array.isArray(cursor[segment])) {
      cursor[segment] = {};
    }
    cursor = cursor[segment];
  }
  cursor[segments[segments.length - 1]] = value;
}

function statusKey(value) {
  return value.replaceAll(':', '-');
}

async function patchGuildModule(event, guildId, moduleId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = collectConfiguration(form);
  } catch (error) {
    setInlineStatus(`guild-status-${moduleId}`, `Error: ${error.message}`, 'error');
    return false;
  }

  const body = {
    enabled: form.enabled.checked,
    configuration,
  };
  const outcome = await runFormMutation(form, () => DynamoMutationTransport.request(fetch, `/api/guild-settings/${guildId}/${moduleId}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  }), `guild-status-${moduleId}`, () => reconcileSettings(`/api/guild-settings/${guildId}`, (settings) => {
    const current = settings.modules?.[moduleId];
    if (current) form.enabled.checked = !!current.enabled;
  }));
  if (outcome.kind === 'success') {
    setInlineStatus(`guild-status-${moduleId}`, 'Saved', 'success');
    closeSettingsModal(`modal-guild-module-${statusKey(moduleId)}`);
  }
  return false;
}

async function toggleGuildModule(guildId, moduleId, enabled, input) {
  return runToggleMutation(input, () => DynamoMutationTransport.request(fetch, `/api/guild-settings/${guildId}/${moduleId}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled }),
  }), `card-status-module-${statusKey(moduleId)}`);
}

async function patchGuildCommand(event, guildId, commandId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = collectConfiguration(form);
  } catch (error) {
    setInlineStatus(`guild-command-status-${statusKey(commandId)}`, `Error: ${error.message}`, 'error');
    return false;
  }

  const outcome = await runFormMutation(form, () => DynamoMutationTransport.request(fetch, `/api/guild-command-settings/${guildId}/${encodeURIComponent(commandId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      enabled: form.enabled.checked,
      configuration,
    }),
  }), `guild-command-status-${statusKey(commandId)}`, () => reconcileSettings(`/api/guild-settings/${guildId}`, (settings) => {
    const current = settings.commands?.[commandId];
    if (current) form.enabled.checked = !!current.enabled;
  }));
  if (outcome.kind === 'success') {
    setInlineStatus(`guild-command-status-${statusKey(commandId)}`, 'Saved', 'success');
    closeSettingsModal(`modal-guild-command-${statusKey(commandId)}`);
  }
  return false;
}

async function toggleGuildCommand(guildId, commandId, enabled, input) {
  return runToggleMutation(input, () => DynamoMutationTransport.request(fetch, `/api/guild-command-settings/${guildId}/${encodeURIComponent(commandId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled }),
  }), `card-status-command-${statusKey(commandId)}`);
}

async function requestDeploymentCommandSync(button) {
  if (button?.disabled) return false;
  if (button) button.disabled = true;
  try {
    const outcome = await DynamoMutationTransport.request(fetch, '/api/deployment-command-sync', { method: 'POST' });
    if (outcome.kind === 'success') { setInlineStatus('command-sync-inline-status', 'Sync requested', 'success'); window.location.reload(); }
    else if (outcome.kind === 'definite-failure') setInlineStatus('command-sync-inline-status', `Error: ${outcome.message}`, 'error');
    else { setUnknownStatus('command-sync-inline-status', outcome.message); window.location.reload(); }
  } finally { if (button) button.disabled = false; }
  return false;
}

async function requestGuildCommandSync(guildId, button) {
  if (button?.disabled) return false;
  if (button) button.disabled = true;
  try {
    const outcome = await DynamoMutationTransport.request(fetch, `/api/guild-command-sync/${guildId}`, { method: 'POST' });
    if (outcome.kind === 'success') { setInlineStatus('command-sync-inline-status', 'Sync requested', 'success'); window.location.reload(); }
    else if (outcome.kind === 'definite-failure') setInlineStatus('command-sync-inline-status', `Error: ${outcome.message}`, 'error');
    else { setUnknownStatus('command-sync-inline-status', outcome.message); window.location.reload(); }
  } finally { if (button) button.disabled = false; }
  return false;
}

"#
    )
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::{Duration as StdDuration, Instant as StdInstant};

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
        RefreshSessionGuildsError, SESSION_COOKIE_NAME, audit_action_label, audit_entity_label,
        build_dashboard_http_client_with_timeouts, build_dashboard_router,
        classify_bot_guild_status, dashboard_script, dashboard_styles, dashboard_ui_script,
        escape_html, font_asset_router, guild_settings_notice, guild_settings_ui_state,
        refresh_session_guilds, render_audit_logs_section, render_dashboard_page_shell,
        render_error_page, render_field, render_guild_card, render_guild_status,
        render_landing_page, render_module_toggle, render_nav, render_section_tabs,
        render_settings_modal, request_id_for_logging, request_path_for_logging,
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
        let rendered = render_guild_card(&GuildCard {
            id: 42,
            name: "Unavailable Guild".to_string(),
            icon_url: None,
            bot_presence: BotGuildPresence::Unavailable,
            manage_url: "/guild/42".to_string(),
            invite_url: "https://discord.com/invite".to_string(),
        });

        assert!(rendered.contains("Status Unavailable"));
        assert!(!rendered.contains("Install Required"));
        assert!(!rendered.contains("Invite Bot"));
        assert!(!rendered.contains("https://discord.com/invite"));
        assert!(rendered.contains("Retry"));
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
            dynamo_enablement::resolve_module_states(registry.catalog(), &deployment, Some(&guild))
                .into_iter()
                .find(|state| state.module.id == module.id)
                .expect("resolved module");

        let toggle = render_module_toggle("guild", module.id, &deployment, Some(&guild), &resolved);
        let status = render_guild_status(&resolved);

        assert!(toggle.contains(" checked onchange="));
        assert!(toggle.contains("aria-label=\"Enable"));
        let deployment_toggle =
            render_module_toggle("deployment", module.id, &deployment, None, &resolved);
        assert!(!deployment_toggle.contains(" checked onchange="));
        assert!(status.contains("Local guild: On"));
        assert!(status.contains("Effective: Off"));
        assert!(status.contains("Blocked by deployment"));
    }

    #[test]
    fn guild_settings_ui_state_distinguishes_absent_empty_and_configured() {
        let empty = GuildSettings::for_guild(42);
        assert_eq!(guild_settings_ui_state(&empty, false), "absent");
        assert_eq!(guild_settings_ui_state(&empty, true), "existing-empty");
        assert!(guild_settings_notice("absent").contains("guild-settings-absent"));
        assert!(guild_settings_notice("existing-empty").contains("guild-settings-empty"));

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
        assert!(guild_settings_notice("existing-configured").contains("guild-settings-configured"));
    }

    #[test]
    fn dashboard_error_page_offers_retry_without_echoing_internal_details() {
        let state = test_dashboard_state(Persistence::default());
        let rendered = render_error_page(
            &state,
            None,
            "Settings Unavailable",
            "Settings could not be loaded.",
        );

        assert!(rendered.contains(">Retry</a>"));
        assert!(rendered.contains("role=\"alert\""));
        assert!(!rendered.contains("mongodb://"));
        assert!(!rendered.contains("test-secret"));
    }

    #[test]
    fn small_ui_pass_has_visible_focus_and_mobile_layout_contracts() {
        let css = dashboard_styles();
        assert!(css.contains(":focus-visible"));
        assert!(css.contains(".toggle-switch input:focus-visible + .toggle-slider"));
        assert!(css.contains(".content-topbar { display: grid;"));
        assert!(css.contains(".sync-panel { flex-direction: column;"));
        assert!(css.contains(".nav-link:hover:not(.active)"));
        assert!(!css.contains(".nav-link:hover, .nav-link.active"));
        assert!(css.contains(".nav-submenu { display: grid; gap: 2px;"));
        assert!(
            css.contains(".nav-sub-link.active { color: var(--accent-text); font-weight: 600; }")
        );
        assert!(!css.contains(".nav-submenu { border-left"));

        let script = dashboard_script();
        let live_semantics = script
            .find("target.setAttribute('role'")
            .expect("live role");
        let content_update = script
            .find("target.textContent")
            .expect("status content update");
        assert!(live_semantics < content_update);
    }

    #[test]
    fn effective_state_summary_is_not_line_clamped() {
        let css = dashboard_styles();
        assert!(css.contains(
            ".summary-card .state-summary { display: block; -webkit-line-clamp: unset; overflow: visible; overflow-wrap: anywhere; }"
        ));

        let state_summary = "<p class=\"detail-meta state-summary\">Effective: Disabled. Blocked by: parent module.</p>";
        assert!(state_summary.contains("state-summary"));
        assert!(!state_summary.contains("-webkit-line-clamp"));
    }

    #[test]
    fn filters_and_navigation_expose_current_semantic_state() {
        let script = dashboard_ui_script();
        assert!(script.contains("updateFilterFeedback('command-filter-status'"));
        assert!(script.contains("item.setAttribute('aria-pressed', 'false')"));
        assert!(script.contains("button.setAttribute('aria-pressed', 'true')"));

        let tabs = render_section_tabs("/guild/42", "modules");
        assert!(tabs.contains(
            "data-testid=\"page-tab-modules\" href=\"/guild/42?tab=modules\" aria-current=\"page\""
        ));

        let css = dashboard_styles();
        assert!(css.contains("--accent-text: #ff9aae;"));
        assert!(css.contains("--accent-button: #bc173d;"));
        assert!(css.contains(".sidebar-footnote { display: none; }"));
    }

    #[test]
    fn sidebar_has_one_active_destination_for_selector_and_section_pages() {
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
            guilds: Vec::new(),
            access_token: "access-token".to_string(),
            expires_at: Utc::now() + Duration::minutes(5),
        };

        let selector = render_nav(&state, Some(&session), Some("/selector"), None);
        assert_eq!(selector.matches("nav-link active").count(), 1);
        assert!(selector.contains(
            "class=\"nav-link active\" href=\"/selector\" aria-current=\"page\">Server Listing"
        ));
        assert!(selector.contains("class=\"nav-link\" href=\"/\">Dashboard"));

        let modules = render_nav(&state, Some(&session), Some("/guild/42"), Some("modules"));
        assert_eq!(modules.matches("nav-link active").count(), 1);
        assert!(
            modules
                .contains("class=\"nav-sub-link active\" href=\"/guild/42?tab=modules\" aria-current=\"page\">Modules")
        );
        assert!(modules.contains(
            "class=\"nav-link active\" href=\"/selector\" aria-current=\"page\">Server Listing"
        ));

        let deployment = render_nav(
            &state,
            Some(&session),
            Some("/deployment"),
            Some("commands"),
        );
        assert_eq!(deployment.matches("nav-link active").count(), 1);
        assert!(deployment.contains(
            "class=\"nav-link active\" href=\"/deployment\" aria-current=\"page\">Deployment"
        ));
        assert!(
            deployment.contains(
                "class=\"nav-sub-link active\" href=\"/deployment?tab=commands\" aria-current=\"page\">Commands"
            )
        );

        let dashboard = render_landing_page(&state, Some(&session));
        assert!(
            dashboard.contains("class=\"button button-primary\" href=\"/selector\">Server Listing")
        );
        assert!(!dashboard.contains("Sign in with Discord"));
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
                admin_user_ids: Vec::new(),
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
            #[cfg(feature = "perf-harness")]
            perf_runtime: None,
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
            },
        );
    }

    fn authenticated_request(method: &str, path: &str, session_id: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(
                header::COOKIE,
                format!("{SESSION_COOKIE_NAME}={session_id}"),
            )
            .body(Body::empty())
            .expect("valid authenticated request")
    }

    fn authenticated_json_request(
        method: &str,
        path: &str,
        session_id: &str,
        body: serde_json::Value,
    ) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(
                header::COOKIE,
                format!("{SESSION_COOKIE_NAME}={session_id}"),
            )
            .header(header::CONTENT_TYPE, "application/json")
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
        assert!(rendered.contains("Guild Settings Unavailable"));
        assert!(rendered.contains(">Retry</a>"));
        assert!(!rendered.contains("mongodb://"));
        assert!(!rendered.contains("guild_settings"));
        assert!(!rendered.contains("test-secret"));
    }

    #[tokio::test]
    async fn selector_refreshes_revoked_and_new_guild_access() {
        let guilds = r#"[{"id":"99","name":"New Guild","icon":null,"permissions":"32"}]"#;
        let (discord_api_base, requests) =
            spawn_discord_guilds_server(StatusCode::OK, guilds, StdDuration::ZERO, 3).await;
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
        assert!(rendered.contains("New Guild"));
        assert!(!rendered.contains("Guild ID <code>42</code>"));

        let denied = app
            .oneshot(authenticated_request("GET", "/guild/42", "test-session"))
            .await
            .expect("guild denial response");
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        assert!(session_can_manage_guild(
            &state.sessions.read().await["test-session"],
            99
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
        assert_eq!(selector.status(), StatusCode::SERVICE_UNAVAILABLE);
        let guild = app
            .oneshot(authenticated_request("GET", "/guild/42", "test-session"))
            .await
            .expect("guild unavailable response");
        assert_eq!(guild.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn invalid_read_guild_authorization_fails_closed() {
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
            .oneshot(authenticated_request("GET", "/selector", "test-session"))
            .await
            .expect("selector invalid authorization response");
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

    #[tokio::test]
    async fn deployment_tabs_render_only_active_controls_and_read_command_sync_on_commands() {
        let provider = Arc::new(CountingProviderStateRepository::default());
        let mut state = test_dashboard_state(Persistence {
            provider_state: Some(provider.clone()),
            ..Persistence::default()
        });
        Arc::get_mut(&mut state)
            .expect("test dashboard state is uniquely owned")
            .app_info
            .owner_user_id = Some(7);
        Arc::get_mut(&mut state)
            .expect("test dashboard state is uniquely owned")
            .config
            .register_globally = true;
        insert_session(&state, "test-session", 42).await;
        let app = build_dashboard_router(state);

        for tab in ["overview", "logs"] {
            let response = app
                .clone()
                .oneshot(authenticated_request(
                    "GET",
                    &format!("/deployment?tab={tab}"),
                    "test-session",
                ))
                .await
                .expect("deployment tab response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .expect("bounded deployment tab body");
            let rendered = String::from_utf8(body.to_vec()).expect("UTF-8 deployment tab");
            assert!(!rendered.contains("DynamoMutationTransport"));
            assert!(!rendered.contains("data-testid=\"settings-modal-"));
            assert!(!rendered.contains("data-testid=\"deployment-commands-section\""));
        }
        assert_eq!(provider.loads.load(Ordering::SeqCst), 0);

        let response = app
            .clone()
            .oneshot(authenticated_request(
                "GET",
                "/deployment?tab=modules",
                "test-session",
            ))
            .await
            .expect("deployment modules response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded deployment modules body");
        let rendered = String::from_utf8(body.to_vec()).expect("UTF-8 deployment modules");
        assert!(rendered.contains("DynamoMutationTransport"));
        assert!(rendered.contains("data-testid=\"deployment-modules-section\""));
        assert!(rendered.contains("data-testid=\"settings-modal-"));
        assert!(!rendered.contains("data-testid=\"deployment-commands-section\""));
        assert!(!rendered.contains("settings-modal-modal-deployment-command-"));
        assert_eq!(provider.loads.load(Ordering::SeqCst), 0);

        let response = app
            .oneshot(authenticated_request(
                "GET",
                "/deployment?tab=commands",
                "test-session",
            ))
            .await
            .expect("deployment commands response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded deployment commands body");
        let rendered = String::from_utf8(body.to_vec()).expect("UTF-8 deployment commands");
        assert!(rendered.contains("DynamoMutationTransport"));
        assert!(rendered.contains("data-testid=\"deployment-commands-section\""));
        assert!(rendered.contains("data-testid=\"settings-modal-"));
        assert!(!rendered.contains("data-testid=\"deployment-modules-section\""));
        assert_eq!(provider.loads.load(Ordering::SeqCst), 1);
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
                admin_user_ids: Vec::new(),
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
            #[cfg(feature = "perf-harness")]
            perf_runtime: None,
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
        let styles = dashboard_styles();
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

    #[test]
    fn escapes_html_characters() {
        assert_eq!(
            escape_html("<script>alert(\"x\")</script>"),
            "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;"
        );
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

        let rendered = render_field(&field, &serde_json::json!({ "channel_id": "123" }));
        assert!(rendered.contains("data-setting-key=\"channel_id\""));
        assert!(rendered.contains("data-setting-kind=\"text\""));
        assert!(rendered.contains("value=\"123\""));
        assert!(rendered.contains("label for=\"field-channel_id-control\""));
        assert!(rendered.contains("id=\"field-channel_id-control\""));
        assert!(rendered.contains("aria-describedby=\"field-channel_id-help\""));
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

        let rendered = render_field(
            &field,
            &serde_json::json!({ "refresh_interval_seconds": 3 }),
        );

        assert!(rendered.contains("type=\"number\""));
        assert!(rendered.contains("min=\"3\""));
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
    fn discord_guild_id_deserializes_from_string() {
        let guild: DashboardGuild = serde_json::from_value(serde_json::json!({
            "id": "110340875107733504",
            "name": "Test Guild",
            "icon": null,
            "permissions": "32"
        }))
        .expect("dashboard guild");

        assert_eq!(guild.id, 110340875107733504);
    }

    #[test]
    fn audit_logs_section_renders_compact_table() {
        let rendered = render_audit_logs_section(
            "/guild/42",
            &DashboardAuditLogPage {
                entries: vec![DashboardAuditLogEntry {
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
                    action: DashboardAuditAction::SaveSettings,
                    summary: "Saved guild settings for command etf.".to_string(),
                }],
                page: 1,
                page_size: 20,
                total: 1,
            },
            Some(DashboardAuditEntityType::Command),
            Some(DashboardAuditAction::SaveSettings),
        );

        assert!(rendered.contains("Dashboard Audit Trail"));
        assert!(rendered.contains("data-testid=\"logs-table\""));
        assert!(rendered.contains("Saved guild settings for command etf."));
        assert!(rendered.contains(audit_entity_label(DashboardAuditEntityType::Command)));
        assert!(rendered.contains(audit_action_label(DashboardAuditAction::SaveSettings)));
        assert!(rendered.contains("data-testid=\"logs-mobile-list\""));
        assert!(rendered.contains("data-testid=\"audit-log-card-0\""));
        assert!(rendered.contains("data-log-field=\"summary\""));
        assert_eq!(rendered.matches("data-log-field=\"action\"").count(), 1);
    }

    #[test]
    fn audit_logs_section_empty_state_renders_mobile_empty_state() {
        let rendered = render_audit_logs_section(
            "/deployment",
            &DashboardAuditLogPage::empty(1, 20),
            None,
            None,
        );

        assert!(rendered.contains("No dashboard audit events recorded yet."));
        assert!(rendered.contains("data-testid=\"logs-mobile-list\""));
        assert!(rendered.contains("data-testid=\"logs-mobile-empty\""));
    }

    #[test]
    fn settings_modal_renders_dialog_semantics() {
        let rendered = render_settings_modal(
            "modal-guild-command-etf",
            "ETF Settings",
            "<form><input name=\"ticker\" /></form>",
        );

        assert!(rendered.contains("role=\"dialog\""));
        assert!(rendered.contains("aria-modal=\"true\""));
        assert!(rendered.contains("aria-labelledby=\"modal-title-modal-guild-command-etf\""));
        assert!(rendered.contains("aria-label=\"Close settings\""));
        assert!(rendered.contains("data-modal-root"));
    }

    #[test]
    fn task_first_shell_marks_non_overview_tabs() {
        let rendered = render_dashboard_page_shell(
            "<section data-testid=\"page-overview\"></section>",
            "<nav data-testid=\"page-tabs\"></nav>",
            "<section data-testid=\"page-active\"></section>",
            "logs",
        );

        assert!(rendered.contains("dashboard-page-shell dashboard-page-shell-task-first"));
        assert!(rendered.contains("data-testid=\"dashboard-page-tabs\""));
        assert!(rendered.contains("data-testid=\"dashboard-page-active\""));
        let tabs_index = rendered
            .find("data-testid=\"dashboard-page-tabs\"")
            .expect("tabs in task-first shell");
        let active_index = rendered
            .find("data-testid=\"dashboard-page-active\"")
            .expect("active section in task-first shell");

        assert!(tabs_index < active_index);
        assert!(!rendered.contains("data-testid=\"dashboard-page-overview\""));
        assert!(!rendered.contains("data-testid=\"page-overview\""));
    }

    #[test]
    fn task_first_shell_keeps_overview_default_layout() {
        let rendered = render_dashboard_page_shell(
            "<section data-testid=\"page-overview\"></section>",
            "<nav data-testid=\"page-tabs\"></nav>",
            "<section data-testid=\"page-active\"></section>",
            "overview",
        );

        assert!(rendered.contains("dashboard-page-shell\""));
        assert!(!rendered.contains("dashboard-page-shell-task-first"));
        let overview_index = rendered
            .find("data-testid=\"dashboard-page-overview\"")
            .expect("overview in overview shell");
        let tabs_index = rendered
            .find("data-testid=\"dashboard-page-tabs\"")
            .expect("tabs in overview shell");
        let active_index = rendered
            .find("data-testid=\"dashboard-page-active\"")
            .expect("active section in overview shell");

        assert!(overview_index < tabs_index);
        assert!(tabs_index < active_index);
    }
}
