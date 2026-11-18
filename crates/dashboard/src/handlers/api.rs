//! JSON API handlers for dashboard reads, settings mutations, command-sync requests,
//! and their authorization and audit helpers.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::OnceLock,
};

use chrono::Utc;
use dynamo_module_kit::SettingsFieldKind;
use dynamo_ops::{
    ANNOUNCEMENT_QUEUE_PROVIDER_ID, AnnouncementStatus, AnnouncementTone, OwnerAnnouncement,
    OwnerAnnouncementStore,
};

use crate::owner_auth::{
    require_elevated_api_owner, require_fresh_action_totp, verify_mutation_request_security,
};
use crate::render::entities::{command_rows, module_rows};
use crate::render::forms::value_at_path;
use crate::status::{EntityStatusView, Scope};
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
            Json(error_payload("dashboard login required")),
        )
            .into_response()
    })
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
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    if let Err(response) = require_elevated_api_owner(&state, &jar, &headers, false).await {
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
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path(module_id): Path<String>,
    Json(patch): Json<DeploymentModuleSettingsPatch>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
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
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path(command_id): Path<String>,
    Json(patch): Json<DeploymentCommandSettingsPatch>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
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
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path((guild_id, module_id)): Path<(u64, String)>,
    Json(patch): Json<GuildModuleSettingsPatch>,
) -> impl IntoResponse {
    let initial_session = match require_api_session(&state, &jar).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(response) = verify_mutation_request_security(&headers, &initial_session, None) {
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
            Json(error_payload("guild settings repository is not configured")),
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
        if let Err(message) =
            validate_module_configuration(&state.module_catalog, &module_id, &configuration)
        {
            return (StatusCode::BAD_REQUEST, Json(error_payload(message))).into_response();
        }
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
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path((guild_id, command_id)): Path<(u64, String)>,
    Json(patch): Json<GuildCommandSettingsPatch>,
) -> impl IntoResponse {
    let initial_session = match require_api_session(&state, &jar).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(response) = verify_mutation_request_security(&headers, &initial_session, None) {
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
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
        Ok(session) => session,
        Err(response) => return response,
    };

    if !state.config.register_globally {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_payload(
                "Deployment command sync is only available while DISCORD_REGISTER_GLOBALLY=true.",
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
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> impl IntoResponse {
    let initial_session = match require_api_session(&state, &jar).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(response) = verify_mutation_request_security(&headers, &initial_session, None) {
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

fn entity_views(
    state: &DashboardState,
    scope: Scope,
    deployment: &DeploymentSettings,
    guild: Option<&GuildSettings>,
    locale: Locale,
) -> Vec<EntityStatusView> {
    let modules = module_rows(
        &state.module_catalog,
        &resolve_module_states(&state.module_catalog, deployment, guild),
        scope,
        locale,
    );
    let commands = command_rows(
        &state.command_catalog,
        &resolve_command_states(
            &state.module_catalog,
            &state.command_catalog,
            deployment,
            guild,
        ),
        scope,
        locale,
    );
    modules
        .iter()
        .chain(commands.iter())
        .map(|row| row.status.to_view(row.kind, &row.id, locale))
        .collect()
}

/// Current status of every module and command in a guild, plus the command
/// sync panel, so the page can refresh in place after a change.
pub(crate) async fn get_guild_state(
    jar: CookieJar,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> impl IntoResponse {
    if let Err(response) = require_api_guild_access(&state, &jar, guild_id).await {
        return response;
    }
    let unavailable = || {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload("guild settings are unavailable".to_string())),
        )
            .into_response()
    };
    let Ok(deployment) = state.persistence.deployment_settings_or_default().await else {
        return unavailable();
    };
    let settings = match state.persistence.guild_settings(guild_id).await {
        Ok(settings) => settings.unwrap_or_else(|| GuildSettings::for_guild(guild_id)),
        Err(error) => {
            warn!(?error, guild_id, "failed to load guild state API");
            return unavailable();
        }
    };
    let sync = if state.config.register_globally {
        None
    } else {
        let store = load_command_sync_store(&state.persistence).await;
        let (fingerprint, _) =
            dynamo_app::application_command_fingerprint_for_scope(&deployment, Some(&settings));
        Some(
            build_command_sync_panel(
                SyncScopeKind::Guild(guild_id),
                &fingerprint,
                store.guild(guild_id),
                &state.config,
                locale,
            )
            .to_json(),
        )
    };
    Json(serde_json::json!({
        "entities": entity_views(&state, Scope::Guild(guild_id), &deployment, Some(&settings), locale),
        "sync": sync,
    }))
    .into_response()
}

pub(crate) async fn get_deployment_state(
    jar: CookieJar,
    headers: HeaderMap,
    locale: Locale,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    if let Err(response) = require_elevated_api_owner(&state, &jar, &headers, false).await {
        return response;
    }
    let Ok(deployment) = state.persistence.deployment_settings_or_default().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "deployment settings are unavailable".to_string(),
            )),
        )
            .into_response();
    };
    let sync = if state.config.register_globally {
        let store = load_command_sync_store(&state.persistence).await;
        let (fingerprint, _) =
            dynamo_app::application_command_fingerprint_for_scope(&deployment, None);
        Some(
            build_command_sync_panel(
                SyncScopeKind::Global,
                &fingerprint,
                Some(&store.global),
                &state.config,
                locale,
            )
            .to_json(),
        )
    } else {
        None
    };
    Json(serde_json::json!({
        "entities": entity_views(&state, Scope::Deployment, &deployment, None, locale),
        "sync": sync,
    }))
    .into_response()
}

const GUILD_RESOURCES_TTL: std::time::Duration = std::time::Duration::from_secs(60);

type GuildResourcesCache = tokio::sync::Mutex<HashMap<u64, (Instant, serde_json::Value)>>;

fn guild_resources_cache() -> &'static GuildResourcesCache {
    static CACHE: OnceLock<GuildResourcesCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

#[derive(Debug, Deserialize)]
struct DiscordChannel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "type")]
    kind: u8,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    position: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DiscordRole {
    id: String,
    name: String,
    #[serde(default)]
    color: u32,
    #[serde(default)]
    position: i64,
    #[serde(default)]
    managed: bool,
}

/// Text channel types the bot can post in (text, announcement) plus
/// categories used to group them.
const CHANNEL_TEXT: u8 = 0;
const CHANNEL_CATEGORY: u8 = 4;
const CHANNEL_ANNOUNCEMENT: u8 = 5;

async fn fetch_discord_list<T: serde::de::DeserializeOwned>(
    state: &DashboardState,
    path: &str,
) -> anyhow::Result<Vec<T>> {
    let request = state
        .http
        .get(format!("{}{path}", state.discord_api_base))
        .header("Authorization", format!("Bot {}", state.config.bot_token));
    let response = crate::discord::send_dashboard_http(state, request)
        .await?
        .error_for_status()?;
    Ok(response.json::<Vec<T>>().await?)
}

/// Channels and roles for the settings pickers. Cached briefly per guild to
/// stay well inside Discord rate limits.
pub(crate) async fn get_guild_resources(
    jar: CookieJar,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> impl IntoResponse {
    if let Err(response) = require_api_guild_access(&state, &jar, guild_id).await {
        return response;
    }
    #[cfg(feature = "perf-harness")]
    if state.perf_runtime.is_some() {
        return Json(serde_json::json!({
            "channels": [
                { "id": "9000000000000001001", "name": "welcome", "category": "Information" },
                { "id": "9000000000000001002", "name": "announcements", "category": "Information" },
                { "id": "9000000000000001003", "name": "mod-logs", "category": "Staff" }
            ],
            "roles": [
                { "id": "9000000000000002001", "name": "Admin", "color": "#5865f2" },
                { "id": "9000000000000002002", "name": "Moderator", "color": "#57f287" },
                { "id": "9000000000000002003", "name": "Support", "color": "#fee75c" }
            ]
        }))
        .into_response();
    }

    #[cfg(feature = "local-preview")]
    if let Some(preview) = state.local_preview.as_ref() {
        return Json(preview.guild_resources(guild_id)).into_response();
    }
    {
        let cache = guild_resources_cache().lock().await;
        if let Some((fetched_at, value)) = cache.get(&guild_id)
            && fetched_at.elapsed() < GUILD_RESOURCES_TTL
        {
            return Json(value.clone()).into_response();
        }
    }

    let channels_path = format!("/guilds/{guild_id}/channels");
    let roles_path = format!("/guilds/{guild_id}/roles");
    let channels = fetch_discord_list::<DiscordChannel>(&state, &channels_path);
    let roles = fetch_discord_list::<DiscordRole>(&state, &roles_path);
    let (channels, roles) = match tokio::join!(channels, roles) {
        (Ok(channels), Ok(roles)) => (channels, roles),
        (channels, roles) => {
            warn!(
                guild_id,
                channels_error = ?channels.err(),
                roles_error = ?roles.err(),
                "failed to load guild channels or roles"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(error_payload(
                    "guild channels and roles are unavailable".to_string(),
                )),
            )
                .into_response();
        }
    };

    let categories = channels
        .iter()
        .filter(|channel| channel.kind == CHANNEL_CATEGORY)
        .map(|channel| {
            (
                channel.id.clone(),
                (
                    channel.position.unwrap_or_default(),
                    channel.name.clone().unwrap_or_default(),
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut text_channels = channels
        .iter()
        .filter(|channel| matches!(channel.kind, CHANNEL_TEXT | CHANNEL_ANNOUNCEMENT))
        .map(|channel| {
            let category = channel
                .parent_id
                .as_ref()
                .and_then(|parent| categories.get(parent));
            (
                category.map(|(position, _)| *position).unwrap_or(-1),
                channel.position.unwrap_or_default(),
                serde_json::json!({
                    "id": channel.id,
                    "name": channel.name.clone().unwrap_or_default(),
                    "category": category.map(|(_, name)| name.clone()),
                }),
            )
        })
        .collect::<Vec<_>>();
    text_channels.sort_by_key(|(category, position, _)| (*category, *position));

    let guild_id_text = guild_id.to_string();
    let mut roles = roles
        .into_iter()
        .filter(|role| role.id != guild_id_text && !role.managed)
        .collect::<Vec<_>>();
    roles.sort_by_key(|role| std::cmp::Reverse(role.position));

    let value = serde_json::json!({
        "channels": text_channels.into_iter().map(|(_, _, value)| value).collect::<Vec<_>>(),
        "roles": roles
            .into_iter()
            .map(|role| serde_json::json!({
                "id": role.id,
                "name": role.name,
                "color": (role.color != 0).then(|| format!("#{:06x}", role.color)),
            }))
            .collect::<Vec<_>>(),
    });
    guild_resources_cache()
        .lock()
        .await
        .insert(guild_id, (Instant::now(), value.clone()));
    Json(value).into_response()
}

fn is_snowflake(raw: &str) -> bool {
    (17..=20).contains(&raw.len()) && raw.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_module_configuration(
    catalog: &ModuleCatalog,
    module_id: &str,
    configuration: &serde_json::Value,
) -> Result<(), String> {
    let Some(entry) = catalog
        .entries
        .iter()
        .find(|entry| entry.module.id == module_id)
    else {
        return Ok(());
    };
    for section in &entry.settings.sections {
        for field in &section.fields {
            let Some(value) = value_at_path(configuration, field.key) else {
                continue;
            };
            if value.is_null() {
                continue;
            }
            match &field.kind {
                SettingsFieldKind::InviteRanks => {
                    let Some(items) = value.as_array() else {
                        return Err(format!("{} must be an array", field.key));
                    };
                    for item in items {
                        let valid_invites = item
                            .get("invites")
                            .and_then(|v| v.as_u64())
                            .is_some_and(|n| n >= 1);
                        let valid_role = item
                            .get("role_id")
                            .and_then(|v| v.as_str())
                            .is_some_and(is_snowflake);
                        if !valid_invites || !valid_role {
                            return Err(format!(
                                "{} items require integer invites >= 1 and a Discord role ID",
                                field.key
                            ));
                        }
                    }
                }
                SettingsFieldKind::TicketCategories => {
                    let Some(items) = value.as_array() else {
                        return Err(format!("{} must be an array", field.key));
                    };
                    for item in items {
                        let valid_name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .is_some_and(|s| !s.is_empty() && s.len() <= 80);
                        let valid_roles = item
                            .get("staff_roles")
                            .and_then(|v| v.as_array())
                            .is_some_and(|roles| {
                                roles
                                    .iter()
                                    .all(|role| role.as_str().is_some_and(is_snowflake))
                            });
                        if !valid_name || !valid_roles {
                            return Err(format!(
                                "{} items require a non-empty topic name and valid staff_roles IDs",
                                field.key
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(crate) struct GuildAnnouncementChannelPatch {
    announcement_channel_id: Option<String>,
}

pub(crate) async fn patch_guild_announcement_channel(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
    Json(patch): Json<GuildAnnouncementChannelPatch>,
) -> impl IntoResponse {
    let initial_session = match require_api_session(&state, &jar).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(response) = verify_mutation_request_security(&headers, &initial_session, None) {
        return response;
    }

    let parsed_channel_id = match patch
        .announcement_channel_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(raw) => {
            if !is_snowflake(raw) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(error_payload("invalid Discord channel ID")),
                )
                    .into_response();
            }
            let Ok(channel_id) = raw.parse::<u64>() else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(error_payload("invalid Discord channel ID")),
                )
                    .into_response();
            };
            Some(channel_id)
        }
        None => None,
    };

    let Some(repo) = state.persistence.guild_settings.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload("guild settings repository is not configured")),
        )
            .into_response();
    };

    let previous = match repo.get(guild_id).await {
        Ok(existing) => existing.and_then(|s| s.announcement_channel_id),
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

    let session = match require_current_api_guild_access(&state, &jar, guild_id).await {
        Ok(session) => session,
        Err(response) => return response,
    };

    match repo
        .upsert_announcement_channel(guild_id, parsed_channel_id)
        .await
    {
        Ok(settings) => {
            if previous != settings.announcement_channel_id {
                let summary = match settings.announcement_channel_id {
                    Some(channel_id) => {
                        format!("Set guild operations announcement channel to #{channel_id}.")
                    }
                    None => "Cleared guild operations announcement channel.".to_string(),
                };
                record_dashboard_audit_events(
                    &state,
                    vec![build_audit_entry(
                        &session,
                        DashboardAuditScope::Guild,
                        Some(guild_id),
                        DashboardAuditEntityType::Operations,
                        "announcement_channel",
                        DashboardAuditAction::SaveSettings,
                        summary,
                    )],
                )
                .await;
            }
            Json(settings).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist announcement channel: {error}"
            ))),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct OwnerTotpActionPayload {
    totp_code: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OwnerBlockGuildPayload {
    blocked: bool,
    totp_code: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OwnerNoticesPayload {
    module_notices: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OwnerAnnouncementPayload {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    tone: Option<String>,
    body: String,
    totp_code: String,
}

pub(crate) async fn post_owner_sync_stale_guilds(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if state.config.register_globally {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_payload(
                "Batch guild sync is unavailable while global command registration is enabled.",
            )),
        )
            .into_response();
    }

    let bot_guilds = match crate::discord::fetch_bot_guilds(&state).await {
        Ok(guilds) => guilds,
        Err(error) => {
            warn!(?error, "failed to load bot guilds for batch sync");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(error_payload("bot guild list is temporarily unavailable")),
            )
                .into_response();
        }
    };
    let deployment = match state.persistence.deployment_settings_or_default().await {
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
    let all_guild_settings: HashMap<u64, GuildSettings> = state
        .persistence
        .list_all_guild_settings()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|settings| (settings.guild_id, settings))
        .collect();
    let blocked: HashSet<u64> = deployment.blocked_guild_ids.iter().copied().collect();
    let mut sync_state = load_command_sync_store(&state.persistence).await;
    let now = Utc::now();
    let mut queued = 0usize;
    for guild in bot_guilds {
        if blocked.contains(&guild.id) {
            continue;
        }
        let guild_settings = all_guild_settings.get(&guild.id);
        let (fingerprint, _) =
            dynamo_app::application_command_fingerprint_for_scope(&deployment, guild_settings);
        let scope_state = sync_state.guild_mut(guild.id);
        if !scope_state.has_pending_request() && !scope_state.is_in_sync_with(&fingerprint) {
            scope_state.request_sync(
                now,
                Some(session.user.id),
                Some(display_name(&session.user)),
            );
            queued += 1;
        }
    }
    if queued > 0
        && let Err(error) = save_command_sync_store(&state.persistence, &sync_state).await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist batch sync request: {error}"
            ))),
        )
            .into_response();
    }
    record_dashboard_audit_events(
        &state,
        vec![build_audit_entry(
            &session,
            DashboardAuditScope::Deployment,
            None,
            DashboardAuditEntityType::Operations,
            "batch_guild_sync",
            DashboardAuditAction::Sync,
            format!("Queued command sync for {queued} stale/failed server(s)."),
        )],
    )
    .await;
    Json(serde_json::json!({ "status": "ok", "queued": queued })).into_response()
}

pub(crate) async fn post_owner_guild_sync(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if state.config.register_globally {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_payload(
                "Guild command sync is unavailable while global command registration is enabled.",
            )),
        )
            .into_response();
    }
    let mut sync_state = load_command_sync_store(&state.persistence).await;
    sync_state.guild_mut(guild_id).request_sync(
        Utc::now(),
        Some(session.user.id),
        Some(display_name(&session.user)),
    );
    match save_command_sync_store(&state.persistence, &sync_state).await {
        Ok(()) => {
            record_dashboard_audit_events(
                &state,
                vec![build_audit_entry(
                    &session,
                    DashboardAuditScope::Deployment,
                    Some(guild_id),
                    DashboardAuditEntityType::Guild,
                    &guild_id.to_string(),
                    DashboardAuditAction::Sync,
                    format!("Owner queued command sync for server #{guild_id}."),
                )],
            )
            .await;
            Json(serde_json::json!({ "status": "ok" })).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist command sync request: {error}"
            ))),
        )
            .into_response(),
    }
}

pub(crate) async fn post_owner_guild_reset(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
    Json(payload): Json<OwnerTotpActionPayload>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(response) = require_fresh_action_totp(&state, &payload.totp_code).await {
        return response;
    }
    let Some(repo) = state.persistence.guild_settings.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload("guild settings repository is not configured")),
        )
            .into_response();
    };
    match repo.reset_guild(guild_id).await {
        Ok(()) => {
            record_dashboard_audit_events(
                &state,
                vec![build_audit_entry(
                    &session,
                    DashboardAuditScope::Deployment,
                    Some(guild_id),
                    DashboardAuditEntityType::Guild,
                    &guild_id.to_string(),
                    DashboardAuditAction::Reset,
                    format!("Owner reset saved settings for server #{guild_id}."),
                )],
            )
            .await;
            Json(serde_json::json!({ "status": "ok" })).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to reset server settings: {error}"
            ))),
        )
            .into_response(),
    }
}

pub(crate) async fn post_owner_guild_leave(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
    Json(payload): Json<OwnerTotpActionPayload>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(response) = require_fresh_action_totp(&state, &payload.totp_code).await {
        return response;
    }
    match crate::discord::leave_bot_guild(&state, guild_id).await {
        Ok(()) => {
            record_dashboard_audit_events(
                &state,
                vec![build_audit_entry(
                    &session,
                    DashboardAuditScope::Deployment,
                    Some(guild_id),
                    DashboardAuditEntityType::Guild,
                    &guild_id.to_string(),
                    DashboardAuditAction::LeaveGuild,
                    format!("Owner removed bot from server #{guild_id}."),
                )],
            )
            .await;
            Json(serde_json::json!({ "status": "ok" })).into_response()
        }
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(error_payload(format!(
                "failed to leave server via Discord: {error}"
            ))),
        )
            .into_response(),
    }
}

pub(crate) async fn post_owner_guild_block(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Path(guild_id): Path<u64>,
    Json(payload): Json<OwnerBlockGuildPayload>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if let Err(response) = require_fresh_action_totp(&state, &payload.totp_code).await {
        return response;
    }
    let Some(repo) = state.persistence.deployment_settings.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "deployment settings repository is not configured",
            )),
        )
            .into_response();
    };
    let current = match repo.get().await {
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
    let mut blocked = current.blocked_guild_ids;
    if payload.blocked {
        blocked.push(guild_id);
        blocked.sort_unstable();
        blocked.dedup();
    } else {
        blocked.retain(|id| *id != guild_id);
    }
    match repo
        .upsert_operational_settings(blocked, current.module_notices)
        .await
    {
        Ok(_) => {
            let (action, summary) = if payload.blocked {
                (
                    DashboardAuditAction::BlockGuild,
                    format!("Owner blocked server #{guild_id}."),
                )
            } else {
                (
                    DashboardAuditAction::UnblockGuild,
                    format!("Owner unblocked server #{guild_id}."),
                )
            };
            record_dashboard_audit_events(
                &state,
                vec![build_audit_entry(
                    &session,
                    DashboardAuditScope::Deployment,
                    Some(guild_id),
                    DashboardAuditEntityType::Guild,
                    &guild_id.to_string(),
                    action,
                    summary,
                )],
            )
            .await;
            Json(serde_json::json!({ "status": "ok", "blocked": payload.blocked })).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to persist server block status: {error}"
            ))),
        )
            .into_response(),
    }
}

pub(crate) async fn patch_owner_notices(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Json(payload): Json<OwnerNoticesPayload>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(repo) = state.persistence.deployment_settings.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload(
                "deployment settings repository is not configured",
            )),
        )
            .into_response();
    };
    let current = match repo.get().await {
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
    let mut cleaned = BTreeMap::new();
    for (key, value) in payload.module_notices {
        let valid_key = key == "__global" || module_exists(&state.module_catalog, &key);
        if !valid_key {
            return (
                StatusCode::BAD_REQUEST,
                Json(error_payload(format!("unknown notice target key: {key}"))),
            )
                .into_response();
        }
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > 500 {
            return (
                StatusCode::BAD_REQUEST,
                Json(error_payload(
                    "dashboard notice text must be at most 500 characters",
                )),
            )
                .into_response();
        }
        cleaned.insert(key, trimmed.to_string());
    }
    match repo
        .upsert_operational_settings(current.blocked_guild_ids, cleaned)
        .await
    {
        Ok(settings) => {
            record_dashboard_audit_events(
                &state,
                vec![build_audit_entry(
                    &session,
                    DashboardAuditScope::Deployment,
                    None,
                    DashboardAuditEntityType::Operations,
                    "dashboard_notices",
                    DashboardAuditAction::SaveSettings,
                    format!(
                        "Updated {} dashboard operations notice banner(s).",
                        settings.module_notices.len()
                    ),
                )],
            )
            .await;
            Json(serde_json::json!({ "status": "ok", "settings": settings })).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!("failed to persist notices: {error}"))),
        )
            .into_response(),
    }
}

pub(crate) async fn post_owner_announcement(
    jar: CookieJar,
    headers: HeaderMap,
    State(state): State<Arc<DashboardState>>,
    Json(payload): Json<OwnerAnnouncementPayload>,
) -> impl IntoResponse {
    let session = match require_elevated_api_owner(&state, &jar, &headers, true).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let body = payload.body.trim();
    if body.is_empty() || body.chars().count() > 3500 {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_payload(
                "announcement message must be 1 to 3500 characters",
            )),
        )
            .into_response();
    }
    let tone = match payload.tone.as_deref() {
        Some(raw) => match AnnouncementTone::from_str_opt(raw) {
            Some(tone) => tone,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(error_payload("invalid announcement tone")),
                )
                    .into_response();
            }
        },
        None => AnnouncementTone::Info,
    };
    let title = payload.title.unwrap_or_default().trim().to_string();
    if title.chars().count() > 120 {
        return (
            StatusCode::BAD_REQUEST,
            Json(error_payload(
                "announcement title must be at most 120 characters",
            )),
        )
            .into_response();
    }
    if let Err(response) = require_fresh_action_totp(&state, &payload.totp_code).await {
        return response;
    }
    if state.persistence.provider_state.is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(error_payload("provider state repository is not configured")),
        )
            .into_response();
    }

    let mut store: OwnerAnnouncementStore = state
        .persistence
        .load_provider_state(ANNOUNCEMENT_QUEUE_PROVIDER_ID)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();

    let now = Utc::now();
    store.enqueue(OwnerAnnouncement {
        id: format!("ann-{}-{}", now.timestamp_millis(), random_token(6)),
        created_at: now,
        processed_at: None,
        actor_user_id: session.user.id,
        actor_username: dashboard_actor_label(&session),
        tone,
        title,
        body: body.to_string(),
        status: AnnouncementStatus::Queued,
        targeted_guilds: 0,
        delivered_guilds: 0,
        failed_guilds: 0,
        last_error: None,
    });
    let serialized = match serde_json::to_value(&store) {
        Ok(value) => value,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_payload(format!(
                    "failed to encode announcement queue: {error}"
                ))),
            )
                .into_response();
        }
    };
    match state
        .persistence
        .save_provider_state(ANNOUNCEMENT_QUEUE_PROVIDER_ID, serialized)
        .await
    {
        Ok(()) => {
            record_dashboard_audit_events(
                &state,
                vec![build_audit_entry(
                    &session,
                    DashboardAuditScope::Deployment,
                    None,
                    DashboardAuditEntityType::Operations,
                    "broadcast_announcement",
                    DashboardAuditAction::SendAnnouncement,
                    format!(
                        "Queued {} operations broadcast announcement.",
                        tone.as_str()
                    ),
                )],
            )
            .await;
            Json(serde_json::json!({ "status": "ok" })).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(error_payload(format!(
                "failed to queue announcement: {error}"
            ))),
        )
            .into_response(),
    }
}

fn command_exists(catalog: &CommandCatalog, command_id: &str) -> bool {
    catalog.find_by_id(command_id).is_some()
}

pub(crate) fn error_payload(message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "message": message.into()
    })
}
