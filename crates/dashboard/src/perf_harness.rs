use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    process,
    sync::{Arc, atomic::Ordering},
};

use anyhow::{Context, ensure};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use tokio::sync::{RwLock, oneshot};

mod config;
mod fixture;
mod runtime;
#[cfg(test)]
use config::{
    ENV_FIXTURE_MODE, ENV_FIXTURE_SHA256, ENV_FIXTURE_VERSION, ENV_NONCE, ENV_READY_FILE,
    ENV_REVISION, FORBIDDEN_ENVIRONMENT, fixture_bytes_sha256, is_lower_hex,
};
use config::{
    FixtureIdentity, FixtureMode, PerfHarnessConfig, random_control_secret,
    validate_compiled_revision, validate_ready_path,
};
use fixture::FixtureData;
pub(crate) use runtime::PerfRuntime;
#[cfg(test)]
use std::env;

use super::{
    DashboardConfig, DashboardGuild, DashboardSession, DashboardState, DashboardUser,
    DiscordApplicationInfo, FIRA_CODE_VARIABLE_PATH, FIRA_SANS_BOLD_PATH, FIRA_SANS_LIGHT_PATH,
    FIRA_SANS_MEDIUM_PATH, FIRA_SANS_REGULAR_PATH, FIRA_SANS_SEMIBOLD_PATH, SESSION_COOKIE_NAME,
};
use dynamo_ops::{
    DashboardAuditLogEntry, DashboardAuditLogPage, DashboardAuditLogQuery,
    DashboardAuditLogRepository,
};
use dynamo_repositories::{
    DeploymentSettingsRepository, GuildSettingsRepository, ProviderStateRepository,
};
use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};

const PERF_CONTROL_HEADER: &str = "x-dynamo-perf-control";
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone)]
struct PerfRepositories {
    runtime: Arc<PerfRuntime>,
    deployment: DeploymentSettings,
    guild: GuildSettings,
}

impl PerfRepositories {
    fn new(runtime: Arc<PerfRuntime>, fixture: &FixtureData) -> Self {
        Self {
            runtime,
            deployment: DeploymentSettings {
                modules: fixture.settings.deployment.modules.clone(),
                commands: fixture.settings.deployment.commands.clone(),
            },
            guild: GuildSettings {
                guild_id: fixture.guild_id(),
                modules: fixture.settings.guild.modules.clone(),
                commands: fixture.settings.guild.commands.clone(),
            },
        }
    }

    fn deny_mutation<T>(&self) -> anyhow::Result<T> {
        self.runtime.increment_repository_mutations();
        anyhow::bail!("performance harness repository mutation denied")
    }
}

#[async_trait::async_trait]
impl DeploymentSettingsRepository for PerfRepositories {
    async fn get(&self) -> anyhow::Result<DeploymentSettings> {
        self.runtime.increment_repository_reads();
        Ok(self.deployment.clone())
    }

    async fn upsert_module_settings(
        &self,
        _module_id: &str,
        _settings: DeploymentModuleSettings,
    ) -> anyhow::Result<DeploymentSettings> {
        self.deny_mutation()
    }

    async fn upsert_command_settings(
        &self,
        _command_id: &str,
        _settings: DeploymentCommandSettings,
    ) -> anyhow::Result<DeploymentSettings> {
        self.deny_mutation()
    }
}

#[async_trait::async_trait]
impl GuildSettingsRepository for PerfRepositories {
    async fn get(&self, guild_id: u64) -> anyhow::Result<Option<GuildSettings>> {
        self.runtime.increment_repository_reads();
        ensure!(
            guild_id == self.guild.guild_id,
            "performance fixture repository only contains the target guild"
        );
        Ok(Some(self.guild.clone()))
    }

    async fn upsert_module_settings(
        &self,
        _guild_id: u64,
        _module_id: &str,
        _settings: GuildModuleSettings,
    ) -> anyhow::Result<GuildSettings> {
        self.deny_mutation()
    }

    async fn upsert_command_settings(
        &self,
        _guild_id: u64,
        _command_id: &str,
        _settings: GuildCommandSettings,
    ) -> anyhow::Result<GuildSettings> {
        self.deny_mutation()
    }
}

#[async_trait::async_trait]
impl ProviderStateRepository for PerfRepositories {
    async fn load_json(&self, _provider_id: &str) -> anyhow::Result<Option<serde_json::Value>> {
        self.runtime.increment_repository_reads();
        Ok(None)
    }

    async fn save_json(&self, _provider_id: &str, _value: serde_json::Value) -> anyhow::Result<()> {
        self.deny_mutation()
    }
}

#[async_trait::async_trait]
impl DashboardAuditLogRepository for PerfRepositories {
    async fn append(
        &self,
        _entry: DashboardAuditLogEntry,
    ) -> anyhow::Result<DashboardAuditLogEntry> {
        self.deny_mutation()
    }

    async fn list(&self, query: DashboardAuditLogQuery) -> anyhow::Result<DashboardAuditLogPage> {
        self.runtime.increment_repository_reads();
        Ok(DashboardAuditLogPage::empty(query.page, query.page_size))
    }
}

#[derive(Serialize)]
struct ReadyFile<'a> {
    schema_version: u32,
    host: &'static str,
    dynamic_port: bool,
    port: u16,
    pid: u32,
    revision: &'a str,
    nonce: &'a str,
    fixture_mode: FixtureMode,
    fixture: &'a FixtureIdentity,
    guild_id: u64,
    cookie_name: &'static str,
    cookie_value: &'a str,
}

fn write_ready_file(path: &Path, ready: &ReadyFile<'_>) -> anyhow::Result<()> {
    validate_ready_path(path)?;
    let mut body =
        serde_json::to_vec(ready).context("failed to serialize performance ready file")?;
    body.push(b'\n');
    publish_ready_bytes(path, &body)
}

struct TemporaryReadyFile {
    path: PathBuf,
}

impl Drop for TemporaryReadyFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn publish_ready_bytes(path: &Path, body: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().context("ready file parent is required")?;
    let final_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("ready file name must be valid Unicode")?;
    let mut created = None;
    for _ in 0..16 {
        let unique = random_control_secret()?;
        let candidate = parent.join(format!(".{final_name}.{}.{}.tmp", process::id(), unique));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                created = Some((file, candidate));
                break;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).context("failed to create same-directory ready temp file");
            }
        }
    }
    let (mut file, temporary_path) =
        created.context("could not reserve a unique ready temp file")?;
    let _cleanup = TemporaryReadyFile {
        path: temporary_path.clone(),
    };
    file.write_all(body)
        .context("failed to write performance ready temp file")?;
    file.flush()
        .context("failed to flush performance ready temp file")?;
    file.sync_all()
        .context("failed to sync performance ready temp file")?;
    let temp_readback =
        fs::read(&temporary_path).context("failed to read back performance ready temp file")?;
    ensure!(
        temp_readback == body,
        "performance ready temp file readback mismatch"
    );
    validate_ready_path(path)?;
    fs::hard_link(&temporary_path, path)
        .context("failed to publish performance ready file without replacement")?;
    let published_metadata =
        fs::symlink_metadata(path).context("failed to inspect published performance ready file")?;
    ensure!(
        published_metadata.is_file() && !published_metadata.file_type().is_symlink(),
        "published performance ready file is not a regular non-symlink file"
    );
    let published_readback =
        fs::read(path).context("failed to read back published performance ready file")?;
    ensure!(
        published_readback == body,
        "published performance ready file readback mismatch"
    );
    Ok(())
}

#[derive(Debug, Serialize)]
struct InstanceSnapshot<'a> {
    schema_version: u32,
    revision: &'a str,
    nonce: &'a str,
    pid: u32,
    fixture_mode: FixtureMode,
    fixture: &'a FixtureIdentity,
    outbound_calls: u64,
    browser_outbound_attempts: u64,
}

#[derive(Debug, Serialize)]
struct CounterSnapshot {
    schema_version: u32,
    denied_requests: u64,
    server_write_attempts: u64,
    repository_reads: u64,
    repository_mutations: u64,
    provider_guild_lookups: u64,
    outbound_calls: u64,
    browser_outbound_attempts: u64,
}

fn instance_snapshot(runtime: &PerfRuntime) -> InstanceSnapshot<'_> {
    InstanceSnapshot {
        schema_version: SCHEMA_VERSION,
        revision: &runtime.revision,
        nonce: &runtime.nonce,
        pid: process::id(),
        fixture_mode: runtime.fixture_mode,
        fixture: &runtime.fixture,
        outbound_calls: runtime.outbound_calls.load(Ordering::SeqCst),
        browser_outbound_attempts: runtime.browser_outbound_attempts.load(Ordering::SeqCst),
    }
}

fn counter_snapshot(runtime: &PerfRuntime) -> CounterSnapshot {
    CounterSnapshot {
        schema_version: SCHEMA_VERSION,
        denied_requests: runtime.denied_requests.load(Ordering::SeqCst),
        server_write_attempts: runtime.server_write_attempts.load(Ordering::SeqCst),
        repository_reads: runtime.repository_reads.load(Ordering::SeqCst),
        repository_mutations: runtime.repository_mutations.load(Ordering::SeqCst),
        provider_guild_lookups: runtime.provider_guild_lookups.load(Ordering::SeqCst),
        outbound_calls: runtime.outbound_calls.load(Ordering::SeqCst),
        browser_outbound_attempts: runtime.browser_outbound_attempts.load(Ordering::SeqCst),
    }
}

async fn perf_instance(State(state): State<Arc<DashboardState>>) -> Response {
    match state.perf_runtime.as_deref() {
        Some(runtime) => Json(instance_snapshot(runtime)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn perf_counters(State(state): State<Arc<DashboardState>>) -> Response {
    match state.perf_runtime.as_deref() {
        Some(runtime) => Json(counter_snapshot(runtime)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn record_browser_outbound_attempt(State(state): State<Arc<DashboardState>>) -> Response {
    let Some(runtime) = state.perf_runtime.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    runtime.increment_browser_outbound_attempts();
    StatusCode::NO_CONTENT.into_response()
}

async fn shutdown_harness(State(state): State<Arc<DashboardState>>) -> Response {
    let Some(runtime) = state.perf_runtime.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if runtime.trigger_shutdown() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::CONFLICT.into_response()
    }
}

async fn enforce_read_only_harness(
    State(runtime): State<Arc<PerfRuntime>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method();
    let allowed_read = (method == Method::GET || method == Method::HEAD)
        && is_allowed_read_request(&runtime, request.uri());
    let path = request.uri().path();
    let authenticated_control = method == Method::POST
        && request.uri().query().is_none()
        && matches!(
            path,
            "/__perf/browser-outbound-attempt" | "/__perf/shutdown"
        )
        && request
            .headers()
            .get(PERF_CONTROL_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == runtime.cookie_value);

    if allowed_read || authenticated_control {
        return next.run(request).await;
    }

    runtime.increment_denied_requests();
    if method != Method::GET && method != Method::HEAD {
        runtime.increment_server_write_attempts();
    }
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": "performance harness route denied" })),
    )
        .into_response()
}

fn is_allowed_read_request(runtime: &PerfRuntime, uri: &axum::http::Uri) -> bool {
    if runtime.fixture_mode != FixtureMode::Public && uri.path() == runtime.guild_path() {
        return uri.query().is_none_or(is_allowed_guild_query);
    }
    uri.query().is_none() && is_allowed_read_path(runtime, uri.path())
}

fn is_allowed_guild_query(query: &str) -> bool {
    let mut fields = query.split('&');
    let Some(tab_field) = fields.next() else {
        return false;
    };
    let Some(tab) = tab_field.strip_prefix("tab=") else {
        return false;
    };
    if matches!(tab, "overview" | "modules" | "commands") {
        return fields.next().is_none();
    }
    if tab != "logs" {
        return false;
    }

    let mut last_rank = 0;
    for field in fields {
        let Some((key, value)) = field.split_once('=') else {
            return false;
        };
        let rank = match key {
            "log_entity" if matches!(value, "module" | "command") => 1,
            "log_action" if matches!(value, "toggle" | "save_settings") => 2,
            "log_page" if is_canonical_log_page(value) => 3,
            _ => return false,
        };
        if rank <= last_rank {
            return false;
        }
        last_rank = rank;
    }
    true
}

fn is_canonical_log_page(value: &str) -> bool {
    value
        .parse::<u64>()
        .ok()
        .filter(|page| (1..=10_000).contains(page))
        .is_some_and(|page| page.to_string() == value)
}

fn is_allowed_read_path(runtime: &PerfRuntime, path: &str) -> bool {
    matches!(path, "/healthz" | "/__perf/instance" | "/__perf/counters")
        || matches!(
            path,
            FIRA_SANS_LIGHT_PATH
                | FIRA_SANS_REGULAR_PATH
                | FIRA_SANS_MEDIUM_PATH
                | FIRA_SANS_SEMIBOLD_PATH
                | FIRA_SANS_BOLD_PATH
                | FIRA_CODE_VARIABLE_PATH
        )
        || match runtime.fixture_mode {
            FixtureMode::Public => path == "/",
            FixtureMode::GuildDetail => path == runtime.guild_path(),
            FixtureMode::ReadOnly => {
                matches!(path, "/" | "/selector") || path == runtime.guild_path()
            }
        }
}

fn build_fixture_state(
    fixture: &FixtureData,
    runtime: Arc<PerfRuntime>,
    port: u16,
) -> anyhow::Result<Arc<DashboardState>> {
    let registry = dynamo_app::module_registry();
    let user_id = fixture.user_id();
    let session = DashboardSession {
        user: DashboardUser {
            id: user_id,
            username: fixture.session.user.username.clone(),
            global_name: Some(fixture.session.user.global_name.clone()),
            avatar: None,
        },
        guilds: fixture
            .session
            .guilds
            .iter()
            .map(|guild| DashboardGuild {
                id: guild.id.parse().expect("validated fixture guild id"),
                name: guild.name.clone(),
                icon: None,
                permissions: guild.permissions.clone(),
            })
            .collect(),
        access_token: String::new(),
        expires_at: fixture.session_expires_at()?,
    };
    let sessions = HashMap::from([(runtime.cookie_value.clone(), session)]);
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .context("failed to create disabled performance HTTP client")?;
    let repositories = Arc::new(PerfRepositories::new(runtime.clone(), fixture));
    let guild_settings: Arc<dyn GuildSettingsRepository> = repositories.clone();
    let deployment_settings: Arc<dyn DeploymentSettingsRepository> = repositories.clone();
    let provider_state: Arc<dyn ProviderStateRepository> = repositories.clone();
    let audit_logs: Arc<dyn DashboardAuditLogRepository> = repositories;
    let persistence = dynamo_persistence_api::Persistence::new(
        Some("dynamo_perf_in_memory".to_string()),
        Some(guild_settings),
        Some(deployment_settings),
        Some(provider_state),
        None,
        None,
        None,
        None,
        None,
        Some(audit_logs),
    );

    Ok(Arc::new(DashboardState {
        config: DashboardConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            public_base_url: format!("http://127.0.0.1:{port}"),
            bot_token: String::new(),
            client_secret: String::new(),
            invite_permissions: 0,
            admin_user_ids: vec![user_id],
            register_globally: false,
            command_sync_interval_seconds: 15,
        },
        http,
        discord_api_base: "https://discord.com/api/v10".to_string(),
        app_info: DiscordApplicationInfo {
            id: fixture.application.id.clone(),
            name: fixture.application.name.clone(),
            icon: None,
            owner_user_id: Some(fixture.owner_user_id()),
        },
        module_catalog: registry.catalog().clone(),
        command_catalog: registry.command_catalog().clone(),
        persistence,
        sessions: Arc::new(RwLock::new(sessions)),
        oauth_states: Arc::new(RwLock::new(HashMap::new())),
        perf_runtime: Some(runtime),
    }))
}

fn build_perf_router(state: Arc<DashboardState>, runtime: Arc<PerfRuntime>) -> Router {
    super::build_dashboard_routes()
        .route("/__perf/instance", get(perf_instance))
        .route("/__perf/counters", get(perf_counters))
        .route(
            "/__perf/browser-outbound-attempt",
            post(record_browser_outbound_attempt),
        )
        .route("/__perf/shutdown", post(shutdown_harness))
        .with_state(state)
        .layer(middleware::from_fn(super::log_request))
        .layer(middleware::from_fn_with_state(
            runtime,
            enforce_read_only_harness,
        ))
}

pub async fn run_perf_harness() -> anyhow::Result<()> {
    let config = PerfHarnessConfig::from_env()?;
    validate_compiled_revision(
        &config.revision,
        option_env!("DYNAMO_PERF_COMPILED_REVISION"),
    )?;
    let fixture = FixtureData::load(&config)?;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("failed to bind dashboard performance harness to 127.0.0.1:0")?;
    let port = listener
        .local_addr()
        .context("failed to read dashboard performance listener address")?
        .port();
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let runtime = Arc::new(PerfRuntime::new(&config, &fixture, shutdown_sender)?);
    let state = build_fixture_state(&fixture, runtime.clone(), port)?;
    let app = build_perf_router(state, runtime.clone());
    let ready = ReadyFile {
        schema_version: SCHEMA_VERSION,
        host: "127.0.0.1",
        dynamic_port: true,
        port,
        pid: process::id(),
        revision: &runtime.revision,
        nonce: &runtime.nonce,
        fixture_mode: runtime.fixture_mode,
        fixture: &runtime.fixture,
        guild_id: runtime.guild_id,
        cookie_name: SESSION_COOKIE_NAME,
        cookie_value: &runtime.cookie_value,
    };
    write_ready_file(&config.ready_file, &ready)?;

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_receiver.await;
        })
        .await
        .context("dashboard performance harness server failed")?;
    Ok(())
}

#[cfg(test)]
mod tests;
