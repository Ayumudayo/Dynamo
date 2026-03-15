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
    CommandCatalog, CommandCatalogEntry, DeploymentModuleSettings, DeploymentSettings,
    GuildModuleSettings, ModuleCatalog, ModuleCatalogEntry, Persistence, ResolvedCommandState,
    ResolvedModuleState, SettingsField, SettingsFieldKind, SettingsSchema, resolve_command_states,
    resolve_module_states,
};
use serde::Deserialize;
use serde_json::Value;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = DashboardConfig::from_env()?;
    let registry = dynamo_app::module_registry();
    let persistence = dynamo_app::persistence_from_env().await?;
    let module_catalog = registry.catalog().clone();
    let command_catalog = registry.command_catalog().clone();
    let loaded_modules = module_catalog
        .entries
        .iter()
        .map(|entry| entry.module.id)
        .collect::<Vec<_>>()
        .join(", ");
    if let Some(database_name) = persistence.database_name.as_deref() {
        info!(database = %database_name, "Dashboard persistence initialized");
    }
    info!(
        host = %config.host,
        port = config.port,
        module_count = module_catalog.entries.len(),
        command_count = command_catalog.entries.len(),
        modules = %loaded_modules,
        "Dashboard companion configured"
    );
    let state = Arc::new(DashboardState {
        module_catalog,
        command_catalog,
        persistence,
    });

    let app = Router::new()
        .route("/", get(index))
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
        .with_state(state);

    let address = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(address).await?;

    info!(address = %address, url = %format!("http://{address}/"), "Dashboard companion listening");
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
    command_catalog: CommandCatalog,
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
        .map(|(entry, resolved)| {
            format!(
                "<li><strong>{}</strong> ({}) {}<br/>{}</li>",
                escape_html(entry.module.display_name),
                escape_html(entry.module.id),
                render_effective_badge(resolved),
                escape_html(entry.module.description),
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
    let resolved_states = resolve_module_states(&state.module_catalog, &settings, None);
    let resolved_command_states = resolve_command_states(
        &state.module_catalog,
        &state.command_catalog,
        &settings,
        None,
    );

    let sections = state
        .module_catalog
        .entries
        .iter()
        .zip(resolved_states.iter())
        .map(|(entry, resolved)| {
            let current = settings
                .modules
                .get(entry.module.id)
                .cloned()
                .unwrap_or(DeploymentModuleSettings {
                    installed: true,
                    enabled: entry.module.enabled_by_default,
                });
            let command_sections = render_deployment_command_sections(
                &state.command_catalog,
                &settings,
                &resolved_command_states,
                entry.module.id,
            );

            format!(
                "<section><h2>{name}</h2><p>{description}</p><p><strong>Status:</strong> {status}</p><form onsubmit=\"return patchDeploymentModule(event, '{module_id}')\"><label><input type=\"checkbox\" name=\"installed\" {installed}/> Installed</label><br/><label><input type=\"checkbox\" name=\"enabled\" {enabled}/> Enabled</label><br/><button type=\"submit\">Save</button><span id=\"deployment-status-{module_id}\" style=\"margin-left:8px\"></span></form>{command_sections}</section>",
                name = escape_html(entry.module.display_name),
                description = escape_html(entry.module.description),
                status = render_deployment_status(resolved),
                module_id = escape_html(entry.module.id),
                installed = if current.installed { "checked" } else { "" },
                enabled = if current.enabled { "checked" } else { "" },
                command_sections = command_sections,
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

    let sections = state
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
            let configuration_pretty = pretty_configuration(&current.configuration);
            let structured_fields =
                render_structured_fields(entry, &current.configuration, guild_id, entry.module.id);
            let advanced_form = render_advanced_json_form(
                guild_id,
                entry.module.id,
                &configuration_pretty,
            );
            let command_sections = render_guild_command_sections(
                &state.command_catalog,
                &settings,
                &resolved_command_states,
                guild_id,
                entry.module.id,
            );

            format!(
                "<section><h2>{name}</h2><p>{description}</p><p><strong>Status:</strong> {status}</p><form onsubmit=\"return patchGuildModule(event, {guild_id}, '{module_id}')\"><label><input type=\"checkbox\" name=\"enabled\" {enabled}/> Enabled in guild</label>{structured_fields}<br/><button type=\"submit\">Save structured settings</button><span id=\"guild-status-{module_id}\" style=\"margin-left:8px\"></span></form>{advanced_form}{command_sections}</section>",
                guild_id = guild_id,
                name = escape_html(entry.module.display_name),
                description = escape_html(entry.module.description),
                status = render_guild_status(resolved),
                module_id = escape_html(entry.module.id),
                enabled = if current.enabled { "checked" } else { "" },
                structured_fields = structured_fields,
                advanced_form = advanced_form,
                command_sections = command_sections,
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

fn render_structured_fields(
    entry: &ModuleCatalogEntry,
    configuration: &Value,
    guild_id: u64,
    module_id: &str,
) -> String {
    render_settings_sections(
        &entry.settings,
        configuration,
        &format!(
            "<p>No structured settings for this module. Use the advanced JSON editor below.</p><input type=\"hidden\" data-setting-key=\"__empty\" data-setting-kind=\"text\" value=\"\" form=\"structured-{guild_id}-{module_id}\" />"
        ),
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

fn render_deployment_command_sections(
    command_catalog: &CommandCatalog,
    settings: &DeploymentSettings,
    resolved_states: &[ResolvedCommandState],
    module_id: &str,
) -> String {
    let commands = command_catalog
        .entries
        .iter()
        .filter(|entry| entry.command.module_id == module_id)
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
            let advanced_form = render_deployment_command_json_form(
                &entry.command.id,
                &pretty_configuration(&current.configuration),
            );

            format!(
                "<article style=\"margin:12px 0 0 16px; padding:12px; border:1px solid #ddd\"><h3>{name}</h3><p><strong>Status:</strong> {status}</p><form onsubmit=\"return patchDeploymentCommand(event, '{command_id}')\"><label><input type=\"checkbox\" name=\"installed\" {installed}/> Installed</label><br/><label><input type=\"checkbox\" name=\"enabled\" {enabled}/> Enabled</label>{structured_fields}<br/><button type=\"submit\">Save command settings</button><span id=\"deployment-command-status-{command_key}\" style=\"margin-left:8px\"></span></form>{advanced_form}</article>",
                name = escape_html(&entry.command.display_name),
                status = resolved
                    .map(render_deployment_command_status)
                    .unwrap_or_else(|| "unknown".to_string()),
                command_id = escape_html(&entry.command.id),
                command_key = status_key(&entry.command.id),
                installed = if current.installed { "checked" } else { "" },
                enabled = if current.enabled { "checked" } else { "" },
                structured_fields = structured_fields,
                advanced_form = advanced_form,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    if commands.is_empty() {
        String::new()
    } else {
        format!("<details><summary>Command Settings</summary>{commands}</details>")
    }
}

fn render_guild_command_sections(
    command_catalog: &CommandCatalog,
    settings: &dynamo_core::GuildSettings,
    resolved_states: &[ResolvedCommandState],
    guild_id: u64,
    module_id: &str,
) -> String {
    let commands = command_catalog
        .entries
        .iter()
        .filter(|entry| entry.command.module_id == module_id)
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
            let advanced_form = render_guild_command_json_form(
                guild_id,
                &entry.command.id,
                &pretty_configuration(&current.configuration),
            );

            format!(
                "<article style=\"margin:12px 0 0 16px; padding:12px; border:1px solid #ddd\"><h3>{name}</h3><p><strong>Status:</strong> {status}</p><form onsubmit=\"return patchGuildCommand(event, {guild_id}, '{command_id}')\"><label><input type=\"checkbox\" name=\"enabled\" {enabled}/> Enabled in guild</label>{structured_fields}<br/><button type=\"submit\">Save command settings</button><span id=\"guild-command-status-{command_key}\" style=\"margin-left:8px\"></span></form>{advanced_form}</article>",
                name = escape_html(&entry.command.display_name),
                status = resolved
                    .map(render_guild_command_status)
                    .unwrap_or_else(|| "unknown".to_string()),
                guild_id = guild_id,
                command_id = escape_html(&entry.command.id),
                command_key = status_key(&entry.command.id),
                enabled = if current.enabled { "checked" } else { "" },
                structured_fields = structured_fields,
                advanced_form = advanced_form,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    if commands.is_empty() {
        String::new()
    } else {
        format!("<details><summary>Command Settings</summary>{commands}</details>")
    }
}

fn render_command_structured_fields(entry: &CommandCatalogEntry, configuration: &Value) -> String {
    render_settings_sections(
        &entry.settings,
        configuration,
        "<p>No structured settings for this command. Use the advanced JSON editor below.</p>",
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

fn render_advanced_json_form(guild_id: u64, module_id: &str, configuration_pretty: &str) -> String {
    format!(
        "<details><summary>Advanced JSON</summary><form onsubmit=\"return patchGuildModuleJson(event, {guild_id}, '{module_id}')\"><label>Configuration JSON</label><br/><textarea name=\"configuration\" rows=\"8\" cols=\"80\">{configuration}</textarea><br/><button type=\"submit\">Save JSON</button><span id=\"guild-json-status-{module_id}\" style=\"margin-left:8px\"></span></form></details>",
        guild_id = guild_id,
        module_id = escape_html(module_id),
        configuration = escape_html(configuration_pretty),
    )
}

fn render_deployment_command_json_form(command_id: &str, configuration_pretty: &str) -> String {
    format!(
        "<details><summary>Advanced JSON</summary><form onsubmit=\"return patchDeploymentCommandJson(event, '{command_id}')\"><label>Configuration JSON</label><br/><textarea name=\"configuration\" rows=\"8\" cols=\"80\">{configuration}</textarea><br/><button type=\"submit\">Save JSON</button><span id=\"deployment-command-json-status-{command_key}\" style=\"margin-left:8px\"></span></form></details>",
        command_id = escape_html(command_id),
        command_key = status_key(command_id),
        configuration = escape_html(configuration_pretty),
    )
}

fn render_guild_command_json_form(
    guild_id: u64,
    command_id: &str,
    configuration_pretty: &str,
) -> String {
    format!(
        "<details><summary>Advanced JSON</summary><form onsubmit=\"return patchGuildCommandJson(event, {guild_id}, '{command_id}')\"><label>Configuration JSON</label><br/><textarea name=\"configuration\" rows=\"8\" cols=\"80\">{configuration}</textarea><br/><button type=\"submit\">Save JSON</button><span id=\"guild-command-json-status-{command_key}\" style=\"margin-left:8px\"></span></form></details>",
        guild_id = guild_id,
        command_id = escape_html(command_id),
        command_key = status_key(command_id),
        configuration = escape_html(configuration_pretty),
    )
}

fn render_effective_badge(state: &ResolvedModuleState) -> String {
    if state.effective_enabled {
        "<span style=\"color:#0a7f40\">enabled</span>".to_string()
    } else {
        "<span style=\"color:#a40000\">disabled</span>".to_string()
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

fn pretty_configuration(configuration: &Value) -> String {
    if configuration.is_null() {
        "{}".to_string()
    } else {
        serde_json::to_string_pretty(configuration).unwrap_or_else(|_| "{}".to_string())
    }
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

async fn patch_deployment_command_settings(
    State(state): State<Arc<DashboardState>>,
    Path(command_id): Path<String>,
    Json(patch): Json<DeploymentCommandSettingsPatch>,
) -> impl IntoResponse {
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

async fn patch_guild_command_settings(
    State(state): State<Arc<DashboardState>>,
    Path((guild_id, command_id)): Path<(u64, String)>,
    Json(patch): Json<GuildCommandSettingsPatch>,
) -> impl IntoResponse {
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
  return false;
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
  return false;
}

async function patchGuildModuleJson(event, guildId, moduleId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = JSON.parse(form.configuration.value || '{}');
  } catch (error) {
    document.getElementById(`guild-json-status-${moduleId}`).textContent = 'Error: invalid JSON';
    return false;
  }

  const response = await fetch(`/api/guild-settings/${guildId}/${moduleId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ configuration }),
  });
  const output = await response.json();
  document.getElementById(`guild-json-status-${moduleId}`).textContent = response.ok ? 'Saved' : `Error: ${output.message ?? response.status}`;
  return false;
}

async function patchDeploymentCommandJson(event, commandId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = JSON.parse(form.configuration.value || '{}');
  } catch (error) {
    document.getElementById(`deployment-command-json-status-${statusKey(commandId)}`).textContent = 'Error: invalid JSON';
    return false;
  }

  const response = await fetch(`/api/deployment-command-settings/${encodeURIComponent(commandId)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ configuration }),
  });
  const output = await response.json();
  document.getElementById(`deployment-command-json-status-${statusKey(commandId)}`).textContent = response.ok ? 'Saved' : `Error: ${output.message ?? response.status}`;
  return false;
}

async function patchGuildCommandJson(event, guildId, commandId) {
  event.preventDefault();
  const form = event.target;
  let configuration;
  try {
    configuration = JSON.parse(form.configuration.value || '{}');
  } catch (error) {
    document.getElementById(`guild-command-json-status-${statusKey(commandId)}`).textContent = 'Error: invalid JSON';
    return false;
  }

  const response = await fetch(`/api/guild-command-settings/${guildId}/${encodeURIComponent(commandId)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ configuration }),
  });
  const output = await response.json();
  document.getElementById(`guild-command-json-status-${statusKey(commandId)}`).textContent = response.ok ? 'Saved' : `Error: ${output.message ?? response.status}`;
  return false;
}
"#
}

#[cfg(test)]
mod tests {
    use super::{escape_html, render_advanced_json_form, render_field};
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
    fn advanced_json_form_includes_textarea_and_submit() {
        let rendered = render_advanced_json_form(1, "stock", "{\n  \"foo\": \"bar\"\n}");
        assert!(rendered.contains("textarea"));
        assert!(rendered.contains("patchGuildModuleJson"));
        assert!(rendered.contains("guild-json-status-stock"));
    }
}
