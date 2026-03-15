use std::{env, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    response::{Html, IntoResponse},
    routing::get,
};
use dynamo_core::ModuleCatalog;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = DashboardConfig::from_env()?;
    let registry = dynamo_app::module_registry();
    let state = Arc::new(DashboardState {
        module_catalog: registry.catalog().clone(),
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/api/modules", get(list_modules))
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
}

async fn index(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let items = state
        .module_catalog
        .entries
        .iter()
        .map(|entry| {
            format!(
                "<li><strong>{}</strong> ({})<br/>{}</li>",
                entry.module.display_name, entry.module.id, entry.module.description
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

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_dashboard=info,dynamo_app=info".into()),
        )
        .try_init();
}
