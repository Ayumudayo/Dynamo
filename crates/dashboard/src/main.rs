use std::{env, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    response::{Html, IntoResponse},
    routing::get,
};
use dynamo_core::{
    DeploymentSettings, DeploymentSettingsRepository, ModuleCatalog, resolve_module_states,
};
use dynamo_persistence_mongo::{MongoPersistence, MongoPersistenceConfig};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = DashboardConfig::from_env()?;
    let registry = dynamo_app::module_registry();
    let deployment_store = connect_deployment_store().await?;
    let state = Arc::new(DashboardState {
        module_catalog: registry.catalog().clone(),
        deployment_store,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/api/modules", get(list_modules))
        .route(
            "/api/module-states/default",
            get(list_default_module_states),
        )
        .route("/api/module-states/live", get(list_live_module_states))
        .with_state(state);

    let address = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(address).await?;

    info!(address = %address, "Dashboard companion listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug, Clone)]
struct DashboardConfig {
    host: std::net::IpAddr,
    port: u16,
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

        Ok(Self { host, port })
    }
}

#[derive(Clone)]
struct DashboardState {
    module_catalog: ModuleCatalog,
    deployment_store: Option<Arc<MongoPersistence>>,
}

async fn index(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let default_states =
        resolve_module_states(&state.module_catalog, &DeploymentSettings::default(), None);
    let items = state
        .module_catalog
        .entries
        .iter()
        .zip(default_states.iter())
        .map(|(entry, state)| {
            format!(
                "<li><strong>{}</strong> ({}) - default: {}<br/>{}</li>",
                entry.module.display_name,
                entry.module.id,
                if state.effective_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                entry.module.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Dynamo Dashboard</title></head><body><h1>Dynamo Dashboard Companion</h1><p>Loaded modules: {}</p><ul>{}</ul></body></html>",
        state.module_catalog.entries.len(),
        items
    ))
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn list_modules(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    Json(state.module_catalog.clone())
}

async fn list_default_module_states(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    Json(resolve_module_states(
        &state.module_catalog,
        &DeploymentSettings::default(),
        None,
    ))
}

async fn list_live_module_states(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let deployment_settings = match &state.deployment_store {
        Some(store) => match store.get().await {
            Ok(settings) => settings,
            Err(error) => {
                return Json(serde_json::json!({
                    "status": "error",
                    "message": format!("failed to load deployment settings: {error}")
                }))
                .into_response();
            }
        },
        None => DeploymentSettings::default(),
    };

    Json(serde_json::json!({
        "status": "ok",
        "states": resolve_module_states(&state.module_catalog, &deployment_settings, None)
    }))
    .into_response()
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_dashboard=info,dynamo_app=info".into()),
        )
        .try_init();
}

async fn connect_deployment_store() -> anyhow::Result<Option<Arc<MongoPersistence>>> {
    let Some(config) = MongoPersistenceConfig::try_from_env()? else {
        info!(
            "MongoDB configuration not found; dashboard live module state endpoint will use defaults"
        );
        return Ok(None);
    };

    let store = MongoPersistence::connect(config).await?;
    Ok(Some(Arc::new(store)))
}
