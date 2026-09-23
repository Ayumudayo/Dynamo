use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    process,
    sync::Arc,
};

use anyhow::Context;
use axum::{
    Router, middleware,
    routing::{get, post},
};
use tokio::sync::{RwLock, oneshot};

use super::{
    SCHEMA_VERSION,
    config::{PerfHarnessConfig, validate_compiled_revision},
    control::{perf_counters, perf_instance, record_browser_outbound_attempt, shutdown_harness},
    fixture::FixtureData,
    ready::{ReadyFile, write_ready_file},
    repositories::PerfRepositories,
    runtime::PerfRuntime,
    security::enforce_read_only_harness,
};
use crate::{
    DashboardConfig, DashboardGuild, DashboardSession, DashboardState, DashboardUser,
    DiscordApplicationInfo, SESSION_COOKIE_NAME,
};
use dynamo_ops::DashboardAuditLogRepository;
use dynamo_repositories::{
    DeploymentSettingsRepository, GuildSettingsRepository, ProviderStateRepository,
};

pub(super) fn build_fixture_state(
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

pub(super) fn build_perf_router(state: Arc<DashboardState>, runtime: Arc<PerfRuntime>) -> Router {
    super::super::build_dashboard_routes()
        .route("/__perf/instance", get(perf_instance))
        .route("/__perf/counters", get(perf_counters))
        .route(
            "/__perf/browser-outbound-attempt",
            post(record_browser_outbound_attempt),
        )
        .route("/__perf/shutdown", post(shutdown_harness))
        .with_state(state)
        .layer(middleware::from_fn(super::super::log_request))
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
