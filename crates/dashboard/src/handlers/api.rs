//! JSON API handlers for dashboard reads, settings mutations, command-sync requests,
//! and their authorization and audit helpers.

use crate::*;

pub(crate) async fn list_modules(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    if let Err(response) = require_api_session(&state, &jar).await {
        return response;
    }
    Json(state.module_catalog.clone()).into_response()
}

pub(crate) async fn list_default_module_states(
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

pub(crate) async fn list_live_module_states(
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
pub(crate) async fn require_api_session(
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
pub(crate) struct DeploymentModuleSettingsPatch {
    installed: Option<bool>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DeploymentCommandSettingsPatch {
    installed: Option<bool>,
    enabled: Option<bool>,
    configuration: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GuildModuleSettingsPatch {
    enabled: Option<bool>,
    configuration: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GuildCommandSettingsPatch {
    enabled: Option<bool>,
    configuration: Option<serde_json::Value>,
}

pub(crate) async fn get_deployment_settings(
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

pub(crate) async fn patch_deployment_module_settings(
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

pub(crate) async fn patch_deployment_command_settings(
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

pub(crate) async fn get_guild_settings(
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

pub(crate) async fn patch_guild_module_settings(
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

pub(crate) async fn patch_guild_command_settings(
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

pub(crate) async fn post_deployment_command_sync(
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

pub(crate) async fn post_guild_command_sync(
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

pub(crate) fn error_payload(message: String) -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "message": message
    })
}
