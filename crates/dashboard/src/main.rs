use std::{env, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::Path,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, patch},
};
use dynamo_core::{
    DeploymentModuleSettings, DeploymentSettings, GuildModuleSettings, ModuleCatalog, Persistence,
    resolve_module_states,
};
use serde::Deserialize;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = DashboardConfig::from_env()?;
    let registry = dynamo_app::module_registry();
    let persistence = dynamo_app::persistence_from_env().await?;
    let state = Arc::new(DashboardState {
        module_catalog: registry.catalog().clone(),
        persistence,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/deployment", get(deployment_page))
        .route("/guild/:guild_id", get(guild_page))
        .route("/healthz", get(healthz))
        .route("/api/modules", get(list_modules))
        .route(
            "/api/module-states/default",
            get(list_default_module_states),
        )
        .route("/api/module-states/live", get(list_live_module_states))
        .route("/api/deployment-settings", get(get_deployment_settings))
        .route(
            "/api/deployment-settings/:module_id",
            patch(patch_deployment_module_settings),
        )
        .route("/api/guild-settings/:guild_id", get(get_guild_settings))
        .route(
            "/api/guild-settings/:guild_id/:module_id",
            patch(patch_guild_module_settings),
        )
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
    persistence: Persistence,
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
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Dynamo Dashboard</title></head><body><h1>Dynamo Dashboard Companion</h1><p>Loaded modules: {}</p><p><a href=\"/deployment\">Manage deployment settings</a></p><form action=\"/guild/\" method=\"get\" onsubmit=\"event.preventDefault(); window.location='/guild/' + document.getElementById('guild-id').value;\"><label for=\"guild-id\">Guild ID:</label><input id=\"guild-id\" name=\"guild-id\" /><button type=\"submit\">Open guild settings</button></form><ul>{}</ul></body></html>",
        state.module_catalog.entries.len(),
        items
    ))
}

async fn deployment_page(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let settings = state
        .persistence
        .deployment_settings_or_default()
        .await
        .unwrap_or_default();

    let sections = state
        .module_catalog
        .entries
        .iter()
        .map(|entry| {
            let current = settings
                .modules
                .get(entry.module.id)
                .cloned()
                .unwrap_or(DeploymentModuleSettings {
                    installed: true,
                    enabled: entry.module.enabled_by_default,
                });

            format!(
                "<section><h2>{name}</h2><p>{description}</p><form onsubmit=\"return patchDeploymentModule(event, '{module_id}')\"><label><input type=\"checkbox\" name=\"installed\" {installed}/> Installed</label><br/><label><input type=\"checkbox\" name=\"enabled\" {enabled}/> Enabled</label><br/><button type=\"submit\">Save</button><span id=\"deployment-status-{module_id}\" style=\"margin-left:8px\"></span></form></section>",
                name = entry.module.display_name,
                description = entry.module.description,
                module_id = entry.module.id,
                installed = if current.installed { "checked" } else { "" },
                enabled = if current.enabled { "checked" } else { "" },
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Deployment Settings</title></head><body><h1>Deployment Settings</h1><p><a href=\"/\">Back</a></p>{sections}<script>{script}</script></body></html>",
        sections = sections,
        script = dashboard_script()
    ))
}

async fn guild_page(
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> Html<String> {
    let settings = state
        .persistence
        .guild_settings_or_default(guild_id)
        .await
        .unwrap_or_default();

    let sections = state
        .module_catalog
        .entries
        .iter()
        .map(|entry| {
            let current = settings
                .modules
                .get(entry.module.id)
                .cloned()
                .unwrap_or_default();
            let configuration = if current.configuration.is_null() {
                "{}".to_string()
            } else {
                serde_json::to_string_pretty(&current.configuration)
                    .unwrap_or_else(|_| "{}".to_string())
            };

            format!(
                "<section><h2>{name}</h2><p>{description}</p><form onsubmit=\"return patchGuildModule(event, {guild_id}, '{module_id}')\"><label><input type=\"checkbox\" name=\"enabled\" {enabled}/> Enabled in guild</label><br/><label>Configuration JSON</label><br/><textarea name=\"configuration\" rows=\"8\" cols=\"80\">{configuration}</textarea><br/><button type=\"submit\">Save</button><span id=\"guild-status-{module_id}\" style=\"margin-left:8px\"></span></form></section>",
                guild_id = guild_id,
                name = entry.module.display_name,
                description = entry.module.description,
                module_id = entry.module.id,
                enabled = if current.enabled { "checked" } else { "" },
                configuration = configuration,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Guild Settings</title></head><body><h1>Guild Settings for {guild_id}</h1><p><a href=\"/\">Back</a> | <a href=\"/deployment\">Deployment settings</a></p>{sections}<script>{script}</script></body></html>",
        guild_id = guild_id,
        sections = sections,
        script = dashboard_script()
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
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dynamo_dashboard=info,dynamo_app=info".into()),
        )
        .try_init();
}

#[derive(Debug, Deserialize)]
struct DeploymentModuleSettingsPatch {
    installed: Option<bool>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct GuildModuleSettingsPatch {
    enabled: Option<bool>,
    configuration: Option<serde_json::Value>,
}

async fn get_deployment_settings(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
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
    State(state): State<Arc<DashboardState>>,
    Path(module_id): Path<String>,
    Json(patch): Json<DeploymentModuleSettingsPatch>,
) -> impl IntoResponse {
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

async fn get_guild_settings(
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> impl IntoResponse {
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
    State(state): State<Arc<DashboardState>>,
    Path((guild_id, module_id)): Path<(u64, String)>,
    Json(patch): Json<GuildModuleSettingsPatch>,
) -> impl IntoResponse {
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

fn module_exists(catalog: &ModuleCatalog, module_id: &str) -> bool {
    catalog
        .entries
        .iter()
        .any(|entry| entry.module.id == module_id)
}

fn error_payload(message: String) -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "message": message
    })
}

fn dashboard_script() -> &'static str {
    r#"
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
  return false;
}

async function patchGuildModule(event, guildId, moduleId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = JSON.parse(form.configuration.value || '{}');
  } catch (error) {
    document.getElementById(`guild-status-${moduleId}`).textContent = `Error: invalid JSON`;
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
  return false;
}
"#
}
