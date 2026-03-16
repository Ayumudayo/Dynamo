use std::{
    collections::{HashMap, HashSet},
    env,
    net::SocketAddr,
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::Path,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, patch},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use dynamo_core::{
    CommandCatalog, CommandCatalogEntry, DeploymentModuleSettings, DeploymentSettings,
    GuildModuleSettings, ModuleCatalog, ModuleCatalogEntry, Persistence, ResolvedCommandState,
    ResolvedModuleState, SettingsField, SettingsFieldKind, SettingsSchema, StartupPhase,
    StartupReport, StartupStatus, catalog_startup_summary, format_kv_list, resolve_command_states,
    resolve_module_states,
};
use futures_util::{StreamExt, stream};
use rand::{Rng, distributions::Alphanumeric};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{info, warn};
use url::Url;

const MUSIC_RUNTIME_NOTICE: &str = "Regular voice channels currently require Discord DAVE/E2EE support. This stable build does not support DAVE yet, so music commands only work for stage-channel smoke tests.";
const SESSION_COOKIE_NAME: &str = "dynamo_dashboard_session";
const SESSION_TTL_HOURS: i64 = 24 * 14;
const OAUTH_STATE_TTL_MINUTES: i64 = 15;
const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DEFAULT_INVITE_PERMISSIONS: u64 = 2_146_958_847;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    let http = reqwest::Client::builder()
        .user_agent("Dynamo Dashboard/0.1.0")
        .build()?;
    let persistence = dynamo_app::persistence_from_env().await?;
    validate_dashboard_persistence(&config, &module_catalog, &command_catalog, &persistence)?;
    let app_info = fetch_application_info(&http, &config).await?;
    let state = Arc::new(DashboardState {
        config,
        http,
        app_info,
        module_catalog,
        command_catalog,
        persistence,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        oauth_states: Arc::new(RwLock::new(HashMap::new())),
    });

    let app = Router::new()
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
        .route("/api/guild-settings/{guild_id}", get(get_guild_settings))
        .route(
            "/api/guild-settings/{guild_id}/{module_id}",
            patch(patch_guild_module_settings),
        )
        .route(
            "/api/guild-command-settings/{guild_id}/{command_id}",
            patch(patch_guild_command_settings),
        )
        .with_state(state.clone());

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

#[derive(Debug, Clone)]
struct DashboardConfig {
    host: std::net::IpAddr,
    port: u16,
    public_base_url: String,
    bot_token: String,
    client_secret: String,
    invite_permissions: u64,
    admin_user_ids: Vec<u64>,
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

        Ok(Self {
            host,
            port,
            public_base_url,
            bot_token,
            client_secret,
            invite_permissions,
            admin_user_ids,
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
        .detail("module_ids", catalog_summary.module_ids.join(", "))
        .detail(
            "per_category_command_counts",
            format_kv_list(&catalog_summary.per_category_command_counts),
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
    catalog_summary: &dynamo_core::CatalogStartupSummary,
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
        .detail("module_ids", catalog_summary.module_ids.join(", "))
        .detail(
            "leaf_command_count",
            catalog_summary.discovered_leaf_command_count.to_string(),
        )
        .detail(
            "per_category_command_counts",
            format_kv_list(&catalog_summary.per_category_command_counts),
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
    app_info: DiscordApplicationInfo,
    module_catalog: ModuleCatalog,
    command_catalog: CommandCatalog,
    persistence: Persistence,
    sessions: Arc<RwLock<HashMap<String, DashboardSession>>>,
    oauth_states: Arc<RwLock<HashMap<String, PendingOauthState>>>,
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
    manageable: bool,
    bot_present: bool,
    manage_url: String,
    invite_url: String,
}

#[derive(Debug, Deserialize)]
struct LoginQuery {
    redirect: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DiscordCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn index(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
    if load_session(&state, &jar).await.is_some() {
        return Redirect::to("/selector").into_response();
    }

    Html(render_landing_page(&state)).into_response()
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
    let Some(session) = load_session(&state, &jar).await else {
        return Redirect::to("/login?redirect=%2Fselector").into_response();
    };

    let guild_cards = load_guild_cards(&state, &session).await;
    Html(render_selector_page(&state, &session, &guild_cards)).into_response()
}

async fn deployment_page(jar: CookieJar, State(state): State<Arc<DashboardState>>) -> Response {
    let Some(session) = load_session(&state, &jar).await else {
        return Redirect::to("/login?redirect=%2Fdeployment").into_response();
    };
    if !user_is_dashboard_admin(&state, &session.user) {
        return Html(render_error_page(
            &state,
            Some(&session),
            "Dashboard Access Restricted",
            "Deployment-wide settings are reserved for the bot owner or configured dashboard administrators.",
        ))
        .into_response();
    }

    let settings = state
        .persistence
        .deployment_settings_or_default()
        .await
        .unwrap_or_default();
    let resolved_states = resolve_module_states(&state.module_catalog, &settings, None);
    let resolved_command_states = resolve_command_states(
        &state.module_catalog,
        &state.command_catalog,
        &settings,
        None,
    );

    let module_modals = state
        .module_catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let current = settings.modules.get(entry.module.id).cloned().unwrap_or(
                DeploymentModuleSettings {
                    installed: true,
                    enabled: entry.module.enabled_by_default,
                },
            );
            let runtime_notice = render_module_runtime_notice(entry.module.id);

            render_deployment_module_modal(entry, resolved, &runtime_notice, &current)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let module_cards = render_module_summary_cards(
        "deployment",
        &state.module_catalog,
        &settings,
        None,
        &resolved_states,
    );
    let command_cards = render_command_summary_cards(
        "deployment",
        &state.command_catalog,
        &settings,
        None,
        &resolved_command_states,
    );
    let command_modals = render_deployment_command_modals(
        &state.command_catalog,
        &settings,
        &resolved_command_states,
    );
    let overview = render_overview_section(
        "Deployment Control",
        "Global install state and command availability across every guild.",
        &[
            (
                "Modules Enabled",
                count_enabled_modules(&resolved_states).to_string(),
            ),
            (
                "Commands Enabled",
                count_enabled_commands(&resolved_command_states).to_string(),
            ),
            (
                "Runtime Notes",
                count_runtime_notices(&state.module_catalog).to_string(),
            ),
        ],
    );
    let content = format!(
        "{overview}<section id=\"activity\" class=\"panel section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Activity</p><h2>Runtime Notes</h2></div></div>{runtime_notices}</section><section id=\"modules\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Modules</p><h2>Deployment Modules</h2></div><input id=\"module-filter\" class=\"toolbar-search\" type=\"search\" placeholder=\"Search modules\" oninput=\"filterModuleCards(this.value)\" /></div><div class=\"module-grid compact-grid\">{module_cards}</div></section><section id=\"commands\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Commands</p><h2>Deployment Commands</h2></div><input id=\"command-filter\" class=\"toolbar-search\" type=\"search\" placeholder=\"Search commands\" oninput=\"filterCommandCards(this.value)\" /></div>{command_tabs}<div class=\"module-grid command-grid compact-grid\">{command_cards}</div></section>{module_modals}{command_modals}<script>{script}</script>",
        overview = overview,
        runtime_notices = render_runtime_notices(&state.module_catalog),
        module_cards = module_cards,
        command_tabs = render_command_category_tabs(&state.command_catalog),
        command_cards = command_cards,
        module_modals = module_modals,
        command_modals = command_modals,
        script = dashboard_script(),
    );

    Html(render_document(
        &state,
        Some(&session),
        "Deployment Settings",
        "Global module installation, enablement, and command controls.",
        Some("/deployment"),
        &content,
    ))
    .into_response()
}

async fn guild_page(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> Response {
    let Some(session) = load_session(&state, &jar).await else {
        return Redirect::to(&format!("/login?redirect=%2Fguild%2F{guild_id}")).into_response();
    };
    let guild_cards = load_guild_cards(&state, &session).await;
    let Some(card) = guild_cards
        .iter()
        .find(|card| card.id == guild_id && card.manageable)
        .cloned()
    else {
        return Html(render_error_page(
            &state,
            Some(&session),
            "Guild Access Restricted",
            "You do not have dashboard access to that server.",
        ))
        .into_response();
    };
    if !card.bot_present {
        return Html(render_install_required_page(&state, &session, &card)).into_response();
    }

    let deployment = state
        .persistence
        .deployment_settings_or_default()
        .await
        .unwrap_or_default();
    let settings = state
        .persistence
        .guild_settings_or_default(guild_id)
        .await
        .unwrap_or_default();
    let resolved_states =
        resolve_module_states(&state.module_catalog, &deployment, Some(&settings));
    let resolved_command_states = resolve_command_states(
        &state.module_catalog,
        &state.command_catalog,
        &deployment,
        Some(&settings),
    );

    let module_modals = state
        .module_catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let current = settings
                .modules
                .get(entry.module.id)
                .cloned()
                .unwrap_or_default();
            let structured_fields = render_structured_fields(entry, &current.configuration);
            let runtime_notice = render_module_runtime_notice(entry.module.id);

            render_guild_module_modal(
                guild_id,
                entry,
                resolved,
                &runtime_notice,
                &current,
                &structured_fields,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let module_cards = render_module_summary_cards(
        "guild",
        &state.module_catalog,
        &deployment,
        Some(&settings),
        &resolved_states,
    );
    let command_cards = render_command_summary_cards(
        "guild",
        &state.command_catalog,
        &deployment,
        Some(&settings),
        &resolved_command_states,
    );
    let command_modals = render_guild_command_modals(
        guild_id,
        &state.command_catalog,
        &settings,
        &resolved_command_states,
    );
    let content = format!(
        "{overview}<section id=\"activity\" class=\"panel section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Runtime</p><h2>Guild Summary</h2></div><span class=\"pill pill-success\">Bot Connected</span></div><div class=\"grid two\"><article class=\"panel info-panel\"><h3>Server Info</h3><p>Guild ID <code>{guild_id}</code></p><p>Guild-specific settings override deployment defaults where enabled.</p></article><article class=\"panel info-panel\"><h3>Runtime Notes</h3>{runtime_notices}</article></div></section><section id=\"modules\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Modules</p><h2>Guild Modules</h2></div><input id=\"module-filter\" class=\"toolbar-search\" type=\"search\" placeholder=\"Search modules\" oninput=\"filterModuleCards(this.value)\" /></div><div class=\"module-grid compact-grid\">{module_cards}</div></section><section id=\"commands\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Commands</p><h2>Guild Commands</h2></div><input id=\"command-filter\" class=\"toolbar-search\" type=\"search\" placeholder=\"Search commands\" oninput=\"filterCommandCards(this.value)\" /></div>{command_tabs}<div class=\"module-grid command-grid compact-grid\">{command_cards}</div></section>{module_modals}{command_modals}<script>{script}</script>",
        overview = render_overview_section(
            &card.name,
            "Guild-scoped module and command controls for this server.",
            &[
                (
                    "Modules Enabled",
                    count_enabled_modules(&resolved_states).to_string()
                ),
                (
                    "Commands Enabled",
                    count_enabled_commands(&resolved_command_states).to_string()
                ),
                ("Guild ID", guild_id.to_string()),
            ],
        ),
        guild_id = guild_id,
        runtime_notices = render_runtime_notices(&state.module_catalog),
        module_cards = module_cards,
        command_tabs = render_command_category_tabs(&state.command_catalog),
        command_cards = command_cards,
        module_modals = module_modals,
        command_modals = command_modals,
        script = dashboard_script(),
    );

    Html(render_document(
        &state,
        Some(&session),
        &format!("Guild Settings: {}", card.name),
        "Guild-scoped module and command controls for this server.",
        Some(&format!("/guild/{guild_id}")),
        &content,
    ))
    .into_response()
}

async fn fetch_application_info(
    http: &reqwest::Client,
    config: &DashboardConfig,
) -> anyhow::Result<DiscordApplicationInfo> {
    let response = http
        .get(format!("{DISCORD_API_BASE}/oauth2/applications/@me"))
        .header("Authorization", format!("Bot {}", config.bot_token))
        .send()
        .await?
        .error_for_status()?;

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
    let mut sessions = state.sessions.write().await;
    sessions.retain(|_, value| !is_session_expired(value));
    sessions.get(&session_id).cloned()
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
    let token_response = state
        .http
        .post(format!("{DISCORD_API_BASE}/oauth2/token"))
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
        ])
        .send()
        .await?
        .error_for_status()?;

    let token_payload: DiscordTokenResponse = token_response.json().await?;
    let bearer = format!("Bearer {}", token_payload.access_token);

    let user_response = state
        .http
        .get(format!("{DISCORD_API_BASE}/users/@me"))
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .send()
        .await?
        .error_for_status()?;
    let user: DiscordOAuthUser = user_response.json().await?;

    let guilds_response = state
        .http
        .get(format!("{DISCORD_API_BASE}/users/@me/guilds"))
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .send()
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

    stream::iter(manageable.into_iter().map(|guild| async move {
        let bot_present = bot_is_in_guild(state, guild.id).await;
        GuildCard {
            id: guild.id,
            name: guild.name.clone(),
            icon_url: guild_icon_url(&guild),
            manageable: true,
            bot_present,
            manage_url: format!("/guild/{}", guild.id),
            invite_url: build_bot_invite_url(state, guild.id),
        }
    }))
    .buffer_unordered(8)
    .collect::<Vec<_>>()
    .await
}

fn session_can_manage_guild(session: &DashboardSession, guild_id: u64) -> bool {
    session
        .guilds
        .iter()
        .any(|guild| guild.id == guild_id && user_can_manage_guild(guild))
}

async fn refresh_session_guilds(
    state: &DashboardState,
    session_id: &str,
) -> Result<Option<DashboardSession>, anyhow::Error> {
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
    let guilds_response = state
        .http
        .get(format!("{DISCORD_API_BASE}/users/@me/guilds"))
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .send()
        .await?
        .error_for_status()?;
    let guilds: Vec<DashboardGuild> = guilds_response.json().await?;

    let mut sessions = state.sessions.write().await;
    let Some(session) = sessions.get_mut(session_id) else {
        return Ok(None);
    };
    session.guilds = guilds;
    Ok(Some(session.clone()))
}

fn user_can_manage_guild(guild: &DashboardGuild) -> bool {
    let Ok(bits) = guild.permissions.parse::<u64>() else {
        return false;
    };
    let administrator = 1 << 3;
    let manage_guild = 1 << 5;
    bits & administrator == administrator || bits & manage_guild == manage_guild
}

async fn bot_is_in_guild(state: &DashboardState, guild_id: u64) -> bool {
    match state
        .http
        .get(format!("{DISCORD_API_BASE}/guilds/{guild_id}"))
        .header("Authorization", format!("Bot {}", state.config.bot_token))
        .send()
        .await
    {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
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

fn render_landing_page(state: &DashboardState) -> String {
    let content = format!(
        "<section class=\"hero\"><div><p class=\"eyebrow\">Discord OAuth Dashboard</p><h1>Manage Dynamo like a real multi-server control panel.</h1><p class=\"lede\">Sign in with Discord, pick the servers you can manage, and adjust module and command behavior without touching the terminal.</p><div class=\"actions\"><a class=\"button button-primary\" href=\"/login\">Sign in with Discord</a><a class=\"button button-secondary\" href=\"/healthz\">Health Check</a></div></div><div class=\"hero-card\"><dl><div><dt>Modules</dt><dd>{module_count}</dd></div><div><dt>Leaf Commands</dt><dd>{command_count}</dd></div><div><dt>Runtime Notes</dt><dd>{notice_count}</dd></div></dl></div></section><section class=\"grid two\"><article class=\"panel\"><h2>Server Selector</h2><p>Dyno-like server cards split between servers you can manage now and servers that still need the bot installed.</p></article><article class=\"panel\"><h2>Shared Runtime Guard</h2><p>Dashboard state, runtime checks, and command sync all resolve from the same module and command enablement rules.</p></article></section>{runtime_notices}",
        module_count = state.module_catalog.entries.len(),
        command_count = state.command_catalog.entries.len(),
        notice_count = count_runtime_notices(&state.module_catalog),
        runtime_notices = render_runtime_notices(&state.module_catalog),
    );

    render_document(
        state,
        None,
        &format!("{} Dashboard", state.app_info.name),
        "OAuth-protected control plane for Dynamo.",
        None,
        &content,
    )
}

fn render_selector_page(
    state: &DashboardState,
    session: &DashboardSession,
    guild_cards: &[GuildCard],
) -> String {
    let manageable_now = guild_cards.iter().filter(|card| card.bot_present).count();
    let needs_install = guild_cards.len().saturating_sub(manageable_now);
    let connected_markup = guild_cards
        .iter()
        .filter(|card| card.bot_present)
        .map(render_guild_card)
        .collect::<Vec<_>>()
        .join("\n");
    let install_markup = guild_cards
        .iter()
        .filter(|card| !card.bot_present)
        .map(render_guild_card)
        .collect::<Vec<_>>()
        .join("\n");

    let content = format!(
        "<section class=\"hero compact dyno-hero\"><div><p class=\"eyebrow\">Server Listing</p><h1>Choose a server to manage.</h1><p class=\"lede\">Only guilds where your account has Manage Server or Administrator are shown. Connected servers can be configured immediately.</p><div class=\"actions\"><a class=\"button button-primary\" href=\"#connected-servers\">Connected Servers</a><a class=\"button button-secondary\" href=\"#install-required\">Needs Install</a></div></div><div class=\"hero-card\"><dl><div><dt>Manage Now</dt><dd>{manageable_now}</dd></div><div><dt>Needs Install</dt><dd>{needs_install}</dd></div><div><dt>Total Eligible</dt><dd>{total}</dd></div></dl></div></section><section class=\"panel toolbar-panel\"><div class=\"toolbar\"><div><p class=\"eyebrow\">Guild Search</p><h2>Server Listing</h2></div><input class=\"toolbar-search\" id=\"guild-filter\" type=\"search\" placeholder=\"Search guilds\" oninput=\"filterGuildCards(this.value)\" /></div></section><section id=\"connected-servers\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Connected</p><h2>Manageable Servers</h2></div><span class=\"pill pill-success\">{manageable_now}</span></div><div class=\"module-grid\">{connected_markup}</div></section><section id=\"install-required\" class=\"section-block\"><div class=\"section-heading\"><div><p class=\"eyebrow\">Install Required</p><h2>Servers Missing The Bot</h2></div><span class=\"pill pill-warn\">{needs_install}</span></div><div class=\"module-grid\">{install_markup}</div></section>",
        manageable_now = manageable_now,
        needs_install = needs_install,
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
    );

    render_document(
        state,
        Some(session),
        "Server Selector",
        "Pick a guild and move into module-level controls.",
        Some("/selector"),
        &content,
    )
}

fn render_guild_card(card: &GuildCard) -> String {
    let badge = if card.bot_present {
        "<span class=\"pill pill-success\">Connected</span>"
    } else {
        "<span class=\"pill pill-warn\">Install Required</span>"
    };
    let action = if card.bot_present {
        format!(
            "<a class=\"button button-primary card-action\" href=\"{}\">Manage Server</a>",
            card.manage_url
        )
    } else {
        format!(
            "<a class=\"button button-secondary card-action\" href=\"{}\">Invite Bot</a>",
            card.invite_url
        )
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
        description = if card.bot_present {
            "Open guild-scoped module and command settings."
        } else {
            "The bot is not in this server yet. Install it first, then return here."
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
        "<section class=\"hero compact\"><div><p class=\"eyebrow\">Dashboard</p><h1>{}</h1><p class=\"lede\">{}</p><div class=\"actions\"><a class=\"button button-primary\" href=\"/selector\">Server Selector</a><a class=\"button button-secondary\" href=\"/\">Home</a></div></div></section>",
        escape_html(title),
        message,
    );

    render_document(state, session, title, message, None, &content)
}

fn render_document(
    state: &DashboardState,
    session: Option<&DashboardSession>,
    title: &str,
    subtitle: &str,
    active_path: Option<&str>,
    content: &str,
) -> String {
    let nav = render_nav(state, session, active_path);
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
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" /><title>{title}</title><style>{styles}</style></head><body><div class=\"backdrop\"></div><div class=\"app-shell\"><aside class=\"sidebar\"><div class=\"sidebar-brand\">{brand_media}<div><p class=\"eyebrow\">Dynamo</p><h1>{app_name}</h1></div></div><nav class=\"sidebar-nav\">{nav}</nav><div class=\"sidebar-footer\"><span class=\"sidebar-footnote\">Rust dashboard control plane</span></div></aside><main class=\"content-shell\"><header class=\"content-topbar\"><div class=\"content-topbar-copy\"><p class=\"eyebrow\">Control Plane</p><h2>{page_title}</h2><p class=\"lede\">{subtitle}</p></div><div class=\"content-topbar-right\"><div class=\"stat-strip\"><div class=\"stat\"><span>Modules</span><strong>{module_count}</strong></div><div class=\"stat\"><span>Commands</span><strong>{command_count}</strong></div></div><div class=\"session-box\">{session_summary}</div></div></header><section class=\"content-body\">{content}</section></main></div><script>{ui_script}</script></body></html>",
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
) -> String {
    let default_dashboard = if session.is_some() { "/selector" } else { "/" };
    let mut items = vec![nav_link(
        "Dashboard",
        default_dashboard,
        active_path == Some("/") || active_path == Some("/selector"),
    )];
    if session.is_some() {
        items.push(nav_link("Modules", "#modules", false));
        items.push(nav_link("Commands", "#commands", false));
        items.push(nav_link(
            "Server Listing",
            "/selector",
            active_path == Some("/selector"),
        ));
        items.push(nav_link("Logs", "#activity", false));
        if session
            .map(|session| user_is_dashboard_admin(state, &session.user))
            .unwrap_or(false)
        {
            items.push(nav_link(
                "Deployment",
                "/deployment",
                active_path == Some("/deployment"),
            ));
        }
        items.push(nav_link("Logout", "/logout", false));
    } else {
        items.push(nav_link("Sign in", "/login", false));
    }

    items.join("")
}

fn nav_link(label: &str, href: &str, active: bool) -> String {
    format!(
        "<a class=\"nav-link{}\" href=\"{}\">{}</a>",
        if active { " active" } else { "" },
        href,
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
    r#"
@import url('https://fonts.googleapis.com/css2?family=Fira+Code:wght@500;600;700&family=Fira+Sans:wght@300;400;500;600;700&display=swap');
:root {
  --bg: #0c0f17;
  --sidebar: #0b0e15;
  --panel: #171b24;
  --panel-strong: #1f2430;
  --panel-border: rgba(255, 255, 255, 0.04);
  --text: #f8fafc;
  --muted: #7f8ba3;
  --accent: #dd2e53;
  --accent-strong: #ff4d6d;
  --accent-soft: rgba(221, 46, 83, 0.16);
  --success: #48e5b2;
  --danger: #f97316;
  --shadow: 0 18px 48px rgba(0, 0, 0, 0.28);
}
* { box-sizing: border-box; }
html, body { margin: 0; min-height: 100%; background: var(--bg); color: var(--text); font-family: 'Fira Sans', sans-serif; }
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
  display: grid; place-items: center; font-family: 'Fira Code', monospace; font-weight: 700;
  background: linear-gradient(135deg, rgba(221, 46, 83, 0.22), rgba(61, 84, 143, 0.18));
  border: 1px solid rgba(255,255,255,0.06);
}
.eyebrow { margin: 0 0 6px; color: var(--accent); font-size: 12px; letter-spacing: 0.16em; text-transform: uppercase; font-family: 'Fira Code', monospace; }
h1, h2, h3, legend { margin: 0; font-family: 'Fira Code', monospace; }
.sidebar-nav { display: grid; gap: 8px; }
.nav-link {
  color: var(--muted); text-decoration: none; padding: 13px 14px; border-radius: 14px;
  transition: background-color 180ms ease, color 180ms ease, border-color 180ms ease;
  border: 1px solid transparent; cursor: pointer; font-weight: 600;
}
.nav-link:hover, .nav-link.active { color: var(--text); background: rgba(221, 46, 83, 0.14); border-color: rgba(221,46,83,0.2); }
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
.button-primary { background: var(--accent); color: #fff6fa; }
.button-primary:hover { background: var(--accent-strong); }
.button-secondary { background: var(--panel-strong); color: var(--text); border-color: rgba(255,255,255,0.06); }
.grid { display: grid; gap: 20px; }
.grid.two { grid-template-columns: repeat(2, minmax(0, 1fr)); margin-bottom: 24px; }
.grid.three { grid-template-columns: repeat(3, minmax(0, 1fr)); }
.panel, section, article, details { padding: 16px; border-radius: 14px; margin-bottom: 16px; }
.guild-card p, .panel p { color: var(--muted); line-height: 1.6; }
.pill {
  display: inline-flex; align-items: center; padding: 6px 10px; border-radius: 999px;
  font-size: 12px; font-family: 'Fira Code', monospace; border: 1px solid rgba(255,255,255,0.08);
}
.pill-success { color: #bbf7d0; background: rgba(72, 229, 178, 0.14); }
.pill-warn { color: #fdba74; background: rgba(249, 115, 22, 0.12); }
.toolbar-panel, .section-block { margin-bottom: 18px; }
.toolbar { display: flex; justify-content: space-between; align-items: center; gap: 16px; }
.toolbar-search { max-width: 320px; margin: 0; }
.section-heading { display: flex; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 16px; }
.module-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 14px; }
.command-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); }
.summary-card-head, .detail-panel-head { display: flex; justify-content: space-between; align-items: start; gap: 12px; }
.detail-panel-status { display: flex; align-items: center; }
.summary-card h3, .detail-panel h2, .command-detail-card h3 { font-size: 0.98rem; line-height: 1.2; }
.summary-card p, .detail-panel p, .command-detail-card p { font-size: 0.92rem; }
.summary-card-subtitle { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; margin-top: 4px; }
.detail-panel p, .summary-card p, .info-panel p { color: var(--muted); }
.detail-meta { margin: 8px 0 0; }
.detail-stack { display: grid; gap: 18px; }
.command-detail-card { border: 1px solid rgba(255,255,255,0.06); background: var(--panel-strong); border-radius: 14px; padding: 16px; }
.summary-card-meta { display: inline-flex; align-items: center; gap: 8px; margin: 6px 10px 0 0; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; }
.summary-card-meta code { color: var(--text); background: rgba(255,255,255,0.04); padding: 3px 7px; border-radius: 8px; font-size: 12px; }
.guild-card-meta { display: flex; align-items: center; gap: 10px; margin: 14px 0 16px; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; }
.guild-card-meta code { color: var(--text); background: rgba(255,255,255,0.04); padding: 4px 8px; border-radius: 8px; }
.empty-state { min-height: 220px; display: flex; flex-direction: column; justify-content: center; }
.card-action { margin-top: 10px; }
.tab-row { display: flex; flex-wrap: wrap; gap: 8px; margin: 0 0 14px; }
.tab-button {
  appearance: none; border: 1px solid rgba(255,255,255,0.06); background: var(--panel-strong); color: var(--muted);
  padding: 8px 12px; border-radius: 10px; font: inherit; font-weight: 700; cursor: pointer; width: auto; margin: 0;
}
.tab-button.active, .tab-button:hover { color: var(--text); background: rgba(221, 46, 83, 0.16); border-color: rgba(221,46,83,0.24); }
.toggle-switch { position: relative; display: inline-flex; width: 44px; height: 24px; align-items: center; cursor: pointer; }
.toggle-switch input { position: absolute; inset: 0; opacity: 0; margin: 0; cursor: pointer; }
.toggle-slider { width: 44px; height: 24px; border-radius: 999px; background: #2a313e; border: 1px solid rgba(255,255,255,0.06); position: relative; transition: background-color 150ms ease; }
.toggle-slider::after { content: ''; position: absolute; top: 2px; left: 2px; width: 18px; height: 18px; border-radius: 50%; background: #aab3c5; transition: transform 150ms ease, background-color 150ms ease; }
.toggle-switch input:checked + .toggle-slider { background: rgba(72, 229, 178, 0.24); }
.toggle-switch input:checked + .toggle-slider::after { transform: translateX(20px); background: var(--success); }
.settings-modal-overlay { position: fixed; inset: 0; background: rgba(7, 9, 14, 0.74); display: grid; place-items: center; padding: 20px; z-index: 50; }
.settings-modal-overlay[hidden] { display: none !important; }
.settings-modal { width: min(560px, 100%); max-height: min(80vh, 860px); overflow: auto; background: #11151e; border: 1px solid rgba(255,255,255,0.08); border-radius: 18px; box-shadow: 0 30px 80px rgba(0,0,0,0.45); }
.settings-modal-head { display: flex; justify-content: space-between; align-items: center; gap: 12px; padding: 18px 18px 12px; position: sticky; top: 0; background: #11151e; }
.settings-modal-body { padding: 0 18px 18px; }
.modal-close { width: auto; min-width: 40px; padding: 8px 12px; font-size: 24px; line-height: 1; background: transparent; color: var(--text); }
form, .advanced-json-form { margin-top: 14px; }
label, small, legend { color: var(--text); }
small { color: var(--muted); }
input, textarea, select, button {
  width: 100%; margin-top: 8px; margin-bottom: 12px; border-radius: 10px; border: 1px solid rgba(255,255,255,0.06);
  background: #262b36; color: var(--text); padding: 12px 14px; font: inherit;
}
input[type='checkbox'] { width: auto; margin-right: 8px; }
button { width: auto; cursor: pointer; background: var(--accent-soft); color: #ffd5df; }
button:hover { background: rgba(221, 46, 83, 0.24); }
fieldset { border: 1px solid rgba(255,255,255,0.06); border-radius: 14px; padding: 16px; margin-top: 16px; }
details summary { cursor: pointer; color: var(--text); font-weight: 600; }
article { margin-top: 16px; }
a { color: #ff6b87; }
.runtime-notice { border-color: rgba(249,115,22,0.22); background: rgba(124, 45, 18, 0.22); }
.content-body > section[id], .content-body > section.section-block { scroll-margin-top: 24px; }
@media (max-width: 1100px) {
  .app-shell { grid-template-columns: 1fr; }
  .sidebar { position: relative; height: auto; }
  .content-topbar, .hero, .hero.compact, .grid.two, .grid.three, .module-grid, .command-grid { grid-template-columns: 1fr; }
  .toolbar { flex-direction: column; align-items: stretch; }
  .content-shell { padding: 20px; }
}
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { transition: none !important; animation: none !important; }
}
"#
}

fn dashboard_ui_script() -> &'static str {
    r#"
function filterGuildCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-guild-name]');
  for (const card of cards) {
    const guildName = card.getAttribute('data-guild-name') || '';
    card.style.display = guildName.includes(value) ? '' : 'none';
  }
}

function filterModuleCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-module-name]');
  for (const card of cards) {
    const moduleName = card.getAttribute('data-module-name') || '';
    card.style.display = moduleName.includes(value) ? '' : 'none';
  }
}

function filterCommandCards(query) {
  const value = (query || '').trim().toLowerCase();
  const cards = document.querySelectorAll('[data-command-name]');
  for (const card of cards) {
    const commandName = card.getAttribute('data-command-name') || '';
    const category = window.__activeCommandCategory || 'all';
    const categoryMatch = category === 'all' || card.getAttribute('data-command-category') === category;
    card.style.display = commandName.includes(value) && categoryMatch ? '' : 'none';
  }
}

function setCommandCategory(category, button) {
  window.__activeCommandCategory = category;
  document.querySelectorAll('.command-tab, .tab-button').forEach((item) => item.classList.remove('active'));
  if (button) {
    button.classList.add('active');
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
            "<p class=\"detail-meta\"><strong>Status:</strong> {}</p>{}<form onsubmit=\"return patchDeploymentModule(event, '{}')\"><label><input type=\"checkbox\" name=\"installed\" {}/> Installed</label><br/><label><input type=\"checkbox\" name=\"enabled\" {}/> Enabled</label><br/><button type=\"submit\">Save</button><span id=\"deployment-status-{}\" style=\"margin-left:8px\"></span></form>",
            render_deployment_status(resolved),
            runtime_notice,
            escape_html(entry.module.id),
            if current.installed { "checked" } else { "" },
            if current.enabled { "checked" } else { "" },
            escape_html(entry.module.id)
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
            "<p class=\"detail-meta\"><strong>Status:</strong> {}</p>{}<form onsubmit=\"return patchGuildModule(event, '{}', '{}')\"><label><input type=\"checkbox\" name=\"enabled\" {}/> Enabled in guild</label>{}<br/><button type=\"submit\">Save</button><span id=\"guild-status-{}\" style=\"margin-left:8px\"></span></form>",
            render_guild_status(resolved),
            runtime_notice,
            guild_id,
            escape_html(entry.module.id),
            if current.enabled { "checked" } else { "" },
            structured_fields,
            escape_html(entry.module.id),
        ),
    )
}

fn render_deployment_command_modals(
    command_catalog: &CommandCatalog,
    settings: &DeploymentSettings,
    resolved_states: &[ResolvedCommandState],
) -> String {
    command_catalog
        .entries
        .iter()
        .map(|entry| {
            let current = settings
                .commands
                .get(&entry.command.id)
                .cloned()
                .unwrap_or_default();
            let resolved = resolved_states
                .iter()
                .find(|state| state.command.id == entry.command.id);
            let structured_fields = render_command_structured_fields(entry, &current.configuration);
            render_settings_modal(
                &modal_id_for_command("deployment", &entry.command.id),
                &entry.command.display_name,
                &format!(
                    "<p class=\"detail-meta\"><strong>Status:</strong> {}</p><form onsubmit=\"return patchDeploymentCommand(event, '{}')\"><label><input type=\"checkbox\" name=\"installed\" {}/> Installed</label><br/><label><input type=\"checkbox\" name=\"enabled\" {}/> Enabled</label>{}<br/><button type=\"submit\">Save command settings</button><span id=\"deployment-command-status-{}\" style=\"margin-left:8px\"></span></form>",
                    resolved
                        .map(render_deployment_command_status)
                        .unwrap_or_else(|| "unknown".to_string()),
                    escape_html(&entry.command.id),
                    if current.installed { "checked" } else { "" },
                    if current.enabled { "checked" } else { "" },
                    structured_fields,
                    status_key(&entry.command.id),
                ),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_guild_command_modals(
    guild_id: u64,
    command_catalog: &CommandCatalog,
    settings: &dynamo_core::GuildSettings,
    resolved_states: &[ResolvedCommandState],
) -> String {
    command_catalog
        .entries
        .iter()
        .map(|entry| {
            let current = settings
                .commands
                .get(&entry.command.id)
                .cloned()
                .unwrap_or_default();
            let resolved = resolved_states
                .iter()
                .find(|state| state.command.id == entry.command.id);
            let structured_fields = render_command_structured_fields(entry, &current.configuration);
            render_settings_modal(
                &modal_id_for_command("guild", &entry.command.id),
                &entry.command.display_name,
                &format!(
                    "<p class=\"detail-meta\"><strong>Status:</strong> {}</p><form onsubmit=\"return patchGuildCommand(event, '{}', '{}')\"><label><input type=\"checkbox\" name=\"enabled\" {}/> Enabled in guild</label>{}<br/><button type=\"submit\">Save command settings</button><span id=\"guild-command-status-{}\" style=\"margin-left:8px\"></span></form>",
                    resolved
                        .map(render_guild_command_status)
                        .unwrap_or_else(|| "unknown".to_string()),
                    guild_id,
                    escape_html(&entry.command.id),
                    if current.enabled { "checked" } else { "" },
                    structured_fields,
                    status_key(&entry.command.id),
                ),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_settings_modal(modal_id: &str, title: &str, body: &str) -> String {
    format!(
        "<div id=\"{modal_id}\" class=\"settings-modal-overlay\" hidden><div class=\"settings-modal\"><div class=\"settings-modal-head\"><h3>{title}</h3><button class=\"modal-close\" type=\"button\" onclick=\"closeSettingsModal('{modal_id}')\">×</button></div><div class=\"settings-modal-body\">{body}</div></div></div>",
        modal_id = modal_id,
        title = escape_html(title),
        body = body,
    )
}

fn render_module_toggle(
    scope: &str,
    module_id: &str,
    _deployment: &DeploymentSettings,
    guild: Option<&dynamo_core::GuildSettings>,
    resolved: &ResolvedModuleState,
) -> String {
    match scope {
        "guild" => format!(
            "<label class=\"toggle-switch\"><input type=\"checkbox\" {} onchange=\"toggleGuildModule('{}', '{}', this.checked)\" /><span class=\"toggle-slider\"></span></label>",
            if resolved.effective_enabled {
                "checked"
            } else {
                ""
            },
            guild.map(|g| g.guild_id.to_string()).unwrap_or_default(),
            escape_html(module_id),
        ),
        _ => format!(
            "<label class=\"toggle-switch\"><input type=\"checkbox\" {} onchange=\"toggleDeploymentModule('{}', this.checked)\" /><span class=\"toggle-slider\"></span></label>",
            if resolved.effective_enabled {
                "checked"
            } else {
                ""
            },
            escape_html(module_id),
        ),
    }
}

fn render_command_toggle(
    scope: &str,
    command_id: &str,
    _deployment: &DeploymentSettings,
    guild: Option<&dynamo_core::GuildSettings>,
    resolved: &ResolvedCommandState,
) -> String {
    match scope {
        "guild" => format!(
            "<label class=\"toggle-switch\"><input type=\"checkbox\" {} onchange=\"toggleGuildCommand('{}', '{}', this.checked)\" /><span class=\"toggle-slider\"></span></label>",
            if resolved.effective_enabled {
                "checked"
            } else {
                ""
            },
            guild.map(|g| g.guild_id.to_string()).unwrap_or_default(),
            escape_html(command_id),
        ),
        _ => format!(
            "<label class=\"toggle-switch\"><input type=\"checkbox\" {} onchange=\"toggleDeploymentCommand('{}', this.checked)\" /><span class=\"toggle-slider\"></span></label>",
            if resolved.effective_enabled {
                "checked"
            } else {
                ""
            },
            escape_html(command_id),
        ),
    }
}

fn render_command_category_tabs(catalog: &CommandCatalog) -> String {
    let mut seen = HashSet::new();
    let mut tabs = vec![
        "<button class=\"tab-button active\" type=\"button\" onclick=\"setCommandCategory('all', this)\">All</button>".to_string(),
    ];

    for entry in &catalog.entries {
        let key = command_category_key(entry);
        let label = command_category_label(entry);
        if seen.insert(key.clone()) {
            tabs.push(format!(
                "<button class=\"tab-button command-tab\" type=\"button\" onclick=\"setCommandCategory('{key}', this)\">{label}</button>",
                key = escape_html(&key),
                label = escape_html(&label),
            ));
        }
    }

    format!("<div class=\"tab-row\">{}</div>", tabs.join(""))
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
    guild: Option<&dynamo_core::GuildSettings>,
    resolved_states: &[ResolvedModuleState],
) -> String {
    catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let toggle = render_module_toggle(scope, entry.module.id, deployment, guild, resolved);
            format!(
                "<article class=\"panel summary-card module-card\" data-module-name=\"{data_name}\"><div class=\"summary-card-head\"><h3>{name}</h3>{toggle}</div><p>{description}</p><div class=\"actions\"><button class=\"button button-secondary\" type=\"button\" onclick=\"openSettingsModal('{modal_id}')\">Settings</button></div></article>",
                data_name = escape_html(&entry.module.display_name.to_ascii_lowercase()),
                name = escape_html(entry.module.display_name),
                toggle = toggle,
                description = escape_html(entry.module.description),
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
    guild: Option<&dynamo_core::GuildSettings>,
    resolved_states: &[ResolvedCommandState],
) -> String {
    catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let toggle = render_command_toggle(scope, &entry.command.id, deployment, guild, resolved);
            format!(
                "<article class=\"panel summary-card command-card\" data-command-name=\"{command_name}\" data-command-category=\"{category_key}\"><div class=\"summary-card-head\"><h3>{display_name}</h3>{toggle}</div><p>{description}</p><div class=\"summary-card-subtitle\">{category_label}</div><div class=\"actions\"><button class=\"button button-secondary\" type=\"button\" onclick=\"openSettingsModal('{modal_id}')\">Settings</button></div></article>",
                command_name = escape_html(&entry.command.display_name.to_ascii_lowercase()),
                category_key = escape_html(&command_category_key(entry)),
                display_name = escape_html(&entry.command.display_name),
                toggle = toggle,
                description = escape_html(entry.command.description.as_deref().unwrap_or("No description provided.")),
                category_label = escape_html(&command_category_label(entry)),
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
    match name {
        "Game Info" => "Game Info",
        _ => name,
    }
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
    match module_id {
        "music" => Some(MUSIC_RUNTIME_NOTICE),
        _ => None,
    }
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
                "<fieldset><legend>{}</legend><p>{}</p>{}</fieldset>",
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
    let help_text = field
        .help_text
        .map(escape_html)
        .map(|text| format!("<small>{text}</small><br/>"))
        .unwrap_or_default();
    let required = if field.required { "required" } else { "" };

    match &field.kind {
        SettingsFieldKind::Toggle => {
            let checked = field_bool_value(configuration, field.key).unwrap_or(false);
            format!(
                "<label><input type=\"checkbox\" data-setting-key=\"{key}\" data-setting-kind=\"toggle\" {checked}/> {label}</label><br/>{help_text}",
                key = escape_html(field.key),
                label = escape_html(field.label),
                checked = if checked { "checked" } else { "" },
                help_text = help_text,
            )
        }
        SettingsFieldKind::Integer => {
            let value = field_string_value(configuration, field.key);
            format!(
                "<label>{label}</label><br/>{help_text}<input type=\"number\" data-setting-key=\"{key}\" data-setting-kind=\"integer\" value=\"{value}\" {required}/>",
                label = escape_html(field.label),
                help_text = help_text,
                key = escape_html(field.key),
                value = escape_html(&value.unwrap_or_default()),
                required = required,
            )
        }
        SettingsFieldKind::Text => {
            let value = field_string_value(configuration, field.key).unwrap_or_default();
            if value.len() > 40 || value.starts_with('[') || value.starts_with('{') {
                format!(
                    "<label>{label}</label><br/>{help_text}<textarea data-setting-key=\"{key}\" data-setting-kind=\"text\" rows=\"4\" cols=\"80\" {required}>{value}</textarea>",
                    label = escape_html(field.label),
                    help_text = help_text,
                    key = escape_html(field.key),
                    required = required,
                    value = escape_html(&value),
                )
            } else {
                format!(
                    "<label>{label}</label><br/>{help_text}<input type=\"text\" data-setting-key=\"{key}\" data-setting-kind=\"text\" value=\"{value}\" {required}/>",
                    label = escape_html(field.label),
                    help_text = help_text,
                    key = escape_html(field.key),
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
                "<label>{label}</label><br/>{help_text}<select data-setting-key=\"{key}\" data-setting-kind=\"select\" {required}>{options}</select>",
                label = escape_html(field.label),
                help_text = help_text,
                key = escape_html(field.key),
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
        "installed: {} | deployment: {} | guild: {} | effective: {}",
        yes_no(state.installed),
        yes_no(state.deployment_enabled),
        yes_no(state.guild_enabled),
        yes_no(state.effective_enabled),
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
        "module: {} | installed: {} | deployment: {} | guild: {} | effective: {}",
        yes_no(state.module_effective_enabled),
        yes_no(state.installed),
        yes_no(state.deployment_enabled),
        yes_no(state.guild_enabled),
        yes_no(state.effective_enabled),
    )
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
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "status": "error",
                    "message": format!("failed to load deployment settings: {error}")
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

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stdout)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_dashboard=info,dynamo_app=info".into()),
        )
        .try_init();
}

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
    if let Err(response) = require_api_admin(&state, &jar).await {
        return response;
    }
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

    match repo.upsert_module_settings(&module_id, next).await {
        Ok(settings) => Json(settings).into_response(),
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
    if let Err(response) = require_api_admin(&state, &jar).await {
        return response;
    }
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

    match repo.upsert_command_settings(&command_id, next).await {
        Ok(settings) => Json(settings).into_response(),
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
    match state.persistence.guild_settings_or_default(guild_id).await {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to load guild settings: {error}"
            ))),
        )
            .into_response(),
    }
}

async fn patch_guild_module_settings(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path((guild_id, module_id)): Path<(u64, String)>,
    Json(patch): Json<GuildModuleSettingsPatch>,
) -> impl IntoResponse {
    if let Err(response) = require_api_guild_access(&state, &jar, guild_id).await {
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

    let current_settings = match repo.get_or_create(guild_id).await {
        Ok(settings) => settings,
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

    match repo
        .upsert_module_settings(guild_id, &module_id, next)
        .await
    {
        Ok(settings) => Json(settings).into_response(),
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
    if let Err(response) = require_api_guild_access(&state, &jar, guild_id).await {
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

    let current_settings = match repo.get_or_create(guild_id).await {
        Ok(settings) => settings,
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

    match repo
        .upsert_command_settings(guild_id, &command_id, next)
        .await
    {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist guild command settings: {error}"
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

fn dashboard_script() -> &'static str {
    r#"
function openSettingsModal(modalId) {
  const modal = document.getElementById(modalId);
  if (!modal) return false;
  modal.hidden = false;
  document.body.style.overflow = 'hidden';
  return false;
}

function closeSettingsModal(modalId) {
  const modal = document.getElementById(modalId);
  if (!modal) return false;
  modal.hidden = true;
  document.body.style.overflow = '';
  return false;
}

async function toggleDeploymentModule(moduleId, enabled) {
  const response = await fetch(`/api/deployment-settings/${moduleId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
  if (!response.ok) {
    alert('Failed to update deployment module state.');
  }
}

async function patchDeploymentModule(event, moduleId) {
  event.preventDefault();
  const form = event.target;
  const body = {
    installed: form.installed.checked,
    enabled: form.enabled.checked,
  };
  const response = await fetch(`/api/deployment-settings/${moduleId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  const output = await response.json();
  document.getElementById(`deployment-status-${moduleId}`).textContent = response.ok ? 'Saved' : `Error: ${output.message ?? response.status}`;
  if (response.ok) {
    closeSettingsModal(`modal-deployment-module-${statusKey(moduleId)}`);
  }
  return false;
}

async function toggleDeploymentCommand(commandId, enabled) {
  const response = await fetch(`/api/deployment-command-settings/${encodeURIComponent(commandId)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
  if (!response.ok) {
    alert('Failed to update deployment command state.');
  }
}

async function patchDeploymentCommand(event, commandId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = collectConfiguration(form);
  } catch (error) {
    document.getElementById(`deployment-command-status-${statusKey(commandId)}`).textContent = `Error: ${error.message}`;
    return false;
  }

  const response = await fetch(`/api/deployment-command-settings/${encodeURIComponent(commandId)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      installed: form.installed.checked,
      enabled: form.enabled.checked,
      configuration,
    }),
  });
  const output = await response.json();
  document.getElementById(`deployment-command-status-${statusKey(commandId)}`).textContent = response.ok ? 'Saved' : `Error: ${output.message ?? response.status}`;
  if (response.ok) {
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
    document.getElementById(`guild-status-${moduleId}`).textContent = `Error: ${error.message}`;
    return false;
  }

  const body = {
    enabled: form.enabled.checked,
    configuration,
  };
  const response = await fetch(`/api/guild-settings/${guildId}/${moduleId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  const output = await response.json();
  document.getElementById(`guild-status-${moduleId}`).textContent = response.ok ? 'Saved' : `Error: ${output.message ?? response.status}`;
  if (response.ok) {
    closeSettingsModal(`modal-guild-module-${statusKey(moduleId)}`);
  }
  return false;
}

async function toggleGuildModule(guildId, moduleId, enabled) {
  const response = await fetch(`/api/guild-settings/${guildId}/${moduleId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
  if (!response.ok) {
    alert('Failed to update guild module state.');
  }
}

async function patchGuildCommand(event, guildId, commandId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = collectConfiguration(form);
  } catch (error) {
    document.getElementById(`guild-command-status-${statusKey(commandId)}`).textContent = `Error: ${error.message}`;
    return false;
  }

  const response = await fetch(`/api/guild-command-settings/${guildId}/${encodeURIComponent(commandId)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      enabled: form.enabled.checked,
      configuration,
    }),
  });
  const output = await response.json();
  document.getElementById(`guild-command-status-${statusKey(commandId)}`).textContent = response.ok ? 'Saved' : `Error: ${output.message ?? response.status}`;
  if (response.ok) {
    closeSettingsModal(`modal-guild-command-${statusKey(commandId)}`);
  }
  return false;
}

async function toggleGuildCommand(guildId, commandId, enabled) {
  const response = await fetch(`/api/guild-command-settings/${guildId}/${encodeURIComponent(commandId)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
  if (!response.ok) {
    alert('Failed to update guild command state.');
  }
}

"#
}

#[cfg(test)]
mod tests {
    use super::{
        DashboardGuild, escape_html, render_field, render_module_runtime_notice,
        sanitize_redirect_target, user_can_manage_guild,
    };
    use dynamo_core::{SettingsField, SettingsFieldKind};

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
    }

    #[test]
    fn music_module_renders_runtime_notice() {
        let rendered = render_module_runtime_notice("music");
        assert!(rendered.contains("Runtime notice"));
        assert!(rendered.contains("DAVE/E2EE"));
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
}
