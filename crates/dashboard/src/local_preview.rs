use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, RwLock as StdRwLock},
};

use chrono::Duration;
use data_encoding::BASE32_NOPAD;
use dynamo_observability::{catalog_startup_summary, init_tracing};
use dynamo_ops::{
    ANNOUNCEMENT_QUEUE_PROVIDER_ID, AnnouncementStatus, BOT_HEARTBEAT_PROVIDER_ID,
    BotHeartbeatSnapshot, COMMAND_SYNC_PROVIDER_ID, CommandSyncStateStore, DashboardAuditAction,
    DashboardAuditEntityType, DashboardAuditLogEntry, DashboardAuditLogPage,
    DashboardAuditLogQuery, DashboardAuditLogRepository, DashboardAuditScope,
    OwnerAnnouncementStore,
};
use dynamo_persistence_api::Persistence;
use dynamo_repositories::{
    DeploymentSettingsRepository, GuildSettingsRepository, ProviderStateRepository,
};
use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, DeploymentSettings, GuildCommandSettings,
    GuildModuleSettings, GuildSettings,
};
use tokio::sync::RwLock;

use crate::auth::random_token;
use crate::discord::{BotGuildSummary, build_dashboard_http_client};
use crate::owner_auth::DEMO_TOTP_SECRET_BASE32;
use crate::render::document::BotGuildPresence;
use crate::state::SESSION_TTL_HOURS;
use crate::{
    DISCORD_API_BASE, DashboardConfig, DashboardGuild, DashboardSession, DashboardState,
    DashboardUser, DiscordApplicationInfo, build_dashboard_router,
};

const DEFAULT_PREVIEW_PORT: u16 = 3000;
const PREVIEW_PORT_ENV: &str = "DASHBOARD_PREVIEW_PORT";

pub(crate) const PREVIEW_USER_ID: u64 = 900_000_000_000_000_001;
pub(crate) const PREVIEW_GUILD_LOUNGE: u64 = 100_000_000_000_000_001;
pub(crate) const PREVIEW_GUILD_RAID: u64 = 100_000_000_000_000_002;
pub(crate) const PREVIEW_GUILD_SANDBOX: u64 = 100_000_000_000_000_003;

#[derive(Debug, Default)]
pub(crate) struct LocalPreviewRuntime {
    left_guilds: StdRwLock<HashSet<u64>>,
}

impl LocalPreviewRuntime {
    pub(crate) fn new_session(&self) -> DashboardSession {
        DashboardSession {
            user: DashboardUser {
                id: PREVIEW_USER_ID,
                username: "preview_operator".to_string(),
                global_name: Some("Preview Operator".to_string()),
                avatar: None,
            },
            guilds: vec![
                DashboardGuild {
                    id: PREVIEW_GUILD_LOUNGE,
                    name: "Dynamo Lounge".to_string(),
                    icon: None,
                    permissions: "40".to_string(),
                },
                DashboardGuild {
                    id: PREVIEW_GUILD_RAID,
                    name: "FFXIV Raid Hub".to_string(),
                    icon: None,
                    permissions: "32".to_string(),
                },
                DashboardGuild {
                    id: PREVIEW_GUILD_SANDBOX,
                    name: "Sandbox Test Server".to_string(),
                    icon: None,
                    permissions: "32".to_string(),
                },
            ],
            access_token: String::new(),
            csrf_token: random_token(32),
            expires_at: chrono::Utc::now() + Duration::hours(SESSION_TTL_HOURS),
        }
    }

    pub(crate) fn bot_presence(&self, guild_id: u64) -> BotGuildPresence {
        if self
            .left_guilds
            .read()
            .expect("left_guilds lock")
            .contains(&guild_id)
        {
            return BotGuildPresence::Missing;
        }
        if matches!(guild_id, PREVIEW_GUILD_LOUNGE | PREVIEW_GUILD_RAID) {
            BotGuildPresence::Present
        } else {
            BotGuildPresence::Missing
        }
    }

    pub(crate) async fn bot_guilds(&self) -> Vec<BotGuildSummary> {
        let left = self.left_guilds.read().expect("left_guilds lock");
        [
            (PREVIEW_GUILD_LOUNGE, "Dynamo Lounge"),
            (PREVIEW_GUILD_RAID, "FFXIV Raid Hub"),
        ]
        .into_iter()
        .filter(|(id, _)| !left.contains(id))
        .map(|(id, name)| BotGuildSummary {
            id,
            name: name.to_string(),
            icon: None,
            owner: true,
        })
        .collect()
    }

    pub(crate) async fn leave_guild(&self, guild_id: u64) -> anyhow::Result<()> {
        self.left_guilds
            .write()
            .expect("left_guilds lock")
            .insert(guild_id);
        Ok(())
    }

    pub(crate) fn guild_resources(&self, _guild_id: u64) -> serde_json::Value {
        serde_json::json!({
            "channels": [
                { "id": "9000000000000001001", "name": "announcements", "category": "Information" },
                { "id": "9000000000000001002", "name": "welcome", "category": "Information" },
                { "id": "9000000000000001003", "name": "general", "category": "Community" },
                { "id": "9000000000000001004", "name": "suggestions", "category": "Community" },
                { "id": "9000000000000001005", "name": "mod-logs", "category": "Staff" },
                { "id": "9000000000000001006", "name": "ticket-logs", "category": "Staff" }
            ],
            "roles": [
                { "id": "9000000000000002001", "name": "Admin", "color": "#5865f2" },
                { "id": "9000000000000002002", "name": "Moderator", "color": "#57f287" },
                { "id": "9000000000000002003", "name": "Support", "color": "#fee75c" },
                { "id": "9000000000000002004", "name": "Raid Leader", "color": "#eb459e" },
                { "id": "9000000000000002005", "name": "Member", "color": "#99aab5" }
            ]
        })
    }
}

struct PreviewStore {
    deployment: DeploymentSettings,
    guilds: BTreeMap<u64, GuildSettings>,
    providers: HashMap<String, serde_json::Value>,
    audit_logs: Vec<DashboardAuditLogEntry>,
    next_audit_id: usize,
}

impl PreviewStore {
    fn seeded() -> Self {
        let mut deployment = DeploymentSettings::default();
        deployment.modules.insert(
            "giveaway".to_string(),
            DeploymentModuleSettings {
                installed: true,
                enabled: false,
            },
        );

        let mut lounge = GuildSettings::for_guild(PREVIEW_GUILD_LOUNGE);
        lounge.announcement_channel_id = Some(9_000_000_000_000_001_001);
        lounge.modules.insert(
            "stock".to_string(),
            GuildModuleSettings {
                enabled: false,
                configuration: serde_json::json!({
                    "default_symbol": "NVDA",
                    "etf_tickers": ["SPY", "QQQ", "SOXX"],
                    "refresh_interval_seconds": 5,
                    "refresh_duration_seconds": 60,
                }),
            },
        );
        lounge.modules.insert(
            "greeting".to_string(),
            GuildModuleSettings {
                enabled: true,
                configuration: serde_json::json!({
                    "welcome": {
                        "enabled": true,
                        "channel_id": "9000000000000001002",
                        "content": "Welcome {member:mention}!",
                        "embed": {
                            "description": "Glad to have you in **{server}**!",
                            "footer": "Member #{count}",
                            "color": "#DD2E53",
                        }
                    }
                }),
            },
        );
        lounge.modules.insert(
            "invite".to_string(),
            GuildModuleSettings {
                enabled: true,
                configuration: serde_json::json!({
                    "ranks": [
                        { "invites": 5, "role_id": "9000000000000002005" }
                    ]
                }),
            },
        );
        lounge.modules.insert(
            "ticket".to_string(),
            GuildModuleSettings {
                enabled: true,
                configuration: serde_json::json!({
                    "setup_title": "Support Desk",
                    "setup_description": "Open a ticket using the menu below and our staff team will reply soon.",
                    "setup_footer": "Dynamo Lounge Support",
                    "categories": [
                        {
                            "name": "General Support",
                            "staff_roles": ["9000000000000002003"]
                        }
                    ]
                }),
            },
        );
        lounge.modules.insert(
            "suggestion".to_string(),
            GuildModuleSettings {
                enabled: true,
                configuration: serde_json::json!({
                    "channel_id": "9000000000000001004",
                    "staff_role_ids": ["9000000000000002001", "9000000000000002002"],
                }),
            },
        );
        lounge.modules.insert(
            "giveaway".to_string(),
            GuildModuleSettings {
                enabled: true,
                configuration: serde_json::json!({}),
            },
        );

        let raid = GuildSettings::for_guild(PREVIEW_GUILD_RAID);

        let now = chrono::Utc::now();
        let mut sync_store = CommandSyncStateStore::default();
        let (raid_fingerprint, raid_count) =
            dynamo_app::application_command_fingerprint_for_scope(&deployment, Some(&raid));
        sync_store.guild_mut(PREVIEW_GUILD_RAID).mark_success(
            now - Duration::minutes(12),
            raid_fingerprint,
            raid_count,
        );

        let heartbeat = BotHeartbeatSnapshot {
            bot_username: Some("Dynamo Preview#0001".to_string()),
            started_at: now - Duration::hours(3),
            updated_at: now - Duration::seconds(12),
            connected_guilds: 2,
            loaded_modules: 15,
            loaded_commands: 24,
            register_globally: false,
            tossinvest_wired: true,
            exchange_refresh_active: true,
            last_exchange_refresh_at: Some(now - Duration::minutes(4)),
            last_exchange_error: None,
            google_translate_configured: true,
        };

        let mut providers = HashMap::new();
        if let Ok(value) = serde_json::to_value(sync_store) {
            providers.insert(COMMAND_SYNC_PROVIDER_ID.to_string(), value);
        }
        if let Ok(value) = serde_json::to_value(heartbeat) {
            providers.insert(BOT_HEARTBEAT_PROVIDER_ID.to_string(), value);
        }

        let audit_logs = vec![
            DashboardAuditLogEntry {
                id: Some("preview-log-1".to_string()),
                timestamp: now - Duration::minutes(90),
                actor_user_id: PREVIEW_USER_ID,
                actor_username: "Preview Operator".to_string(),
                scope: DashboardAuditScope::Guild,
                guild_id: Some(PREVIEW_GUILD_LOUNGE),
                entity_type: DashboardAuditEntityType::Module,
                entity_id: "stock".to_string(),
                action: DashboardAuditAction::Toggle,
                summary: "Updated guild module stock: enabled=false.".to_string(),
            },
            DashboardAuditLogEntry {
                id: Some("preview-log-2".to_string()),
                timestamp: now - Duration::minutes(25),
                actor_user_id: PREVIEW_USER_ID,
                actor_username: "Preview Operator".to_string(),
                scope: DashboardAuditScope::Guild,
                guild_id: Some(PREVIEW_GUILD_LOUNGE),
                entity_type: DashboardAuditEntityType::Module,
                entity_id: "suggestion".to_string(),
                action: DashboardAuditAction::SaveSettings,
                summary: "Saved guild settings for module suggestion.".to_string(),
            },
            DashboardAuditLogEntry {
                id: Some("preview-log-3".to_string()),
                timestamp: now - Duration::minutes(5),
                actor_user_id: PREVIEW_USER_ID,
                actor_username: "Preview Operator".to_string(),
                scope: DashboardAuditScope::Guild,
                guild_id: Some(PREVIEW_GUILD_LOUNGE),
                entity_type: DashboardAuditEntityType::Module,
                entity_id: "greeting".to_string(),
                action: DashboardAuditAction::SaveSettings,
                summary: "Saved guild settings for module greeting.".to_string(),
            },
        ];

        Self {
            deployment,
            guilds: BTreeMap::from([(PREVIEW_GUILD_LOUNGE, lounge), (PREVIEW_GUILD_RAID, raid)]),
            providers,
            next_audit_id: audit_logs.len() + 1,
            audit_logs,
        }
    }
}

#[derive(Clone)]
struct PreviewRepositories {
    store: Arc<RwLock<PreviewStore>>,
}

impl PreviewRepositories {
    fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(PreviewStore::seeded())),
        }
    }
}

#[async_trait::async_trait]
impl DeploymentSettingsRepository for PreviewRepositories {
    async fn get(&self) -> anyhow::Result<DeploymentSettings> {
        Ok(self.store.read().await.deployment.clone())
    }

    async fn upsert_module_settings(
        &self,
        module_id: &str,
        settings: DeploymentModuleSettings,
    ) -> anyhow::Result<DeploymentSettings> {
        let mut store = self.store.write().await;
        store
            .deployment
            .modules
            .insert(module_id.to_string(), settings);
        Ok(store.deployment.clone())
    }

    async fn upsert_command_settings(
        &self,
        command_id: &str,
        settings: DeploymentCommandSettings,
    ) -> anyhow::Result<DeploymentSettings> {
        let mut store = self.store.write().await;
        store
            .deployment
            .commands
            .insert(command_id.to_string(), settings);
        Ok(store.deployment.clone())
    }

    async fn upsert_operational_settings(
        &self,
        blocked_guild_ids: Vec<u64>,
        module_notices: BTreeMap<String, String>,
    ) -> anyhow::Result<DeploymentSettings> {
        let mut store = self.store.write().await;
        store.deployment.blocked_guild_ids = blocked_guild_ids;
        store.deployment.module_notices = module_notices;
        Ok(store.deployment.clone())
    }
}

#[async_trait::async_trait]
impl GuildSettingsRepository for PreviewRepositories {
    async fn get(&self, guild_id: u64) -> anyhow::Result<Option<GuildSettings>> {
        let store = self.store.read().await;
        Ok(Some(
            store
                .guilds
                .get(&guild_id)
                .cloned()
                .unwrap_or_else(|| GuildSettings::for_guild(guild_id)),
        ))
    }

    async fn list_all(&self) -> anyhow::Result<Vec<GuildSettings>> {
        Ok(self.store.read().await.guilds.values().cloned().collect())
    }

    async fn upsert_module_settings(
        &self,
        guild_id: u64,
        module_id: &str,
        settings: GuildModuleSettings,
    ) -> anyhow::Result<GuildSettings> {
        let mut store = self.store.write().await;
        let guild = store
            .guilds
            .entry(guild_id)
            .or_insert_with(|| GuildSettings::for_guild(guild_id));
        guild.modules.insert(module_id.to_string(), settings);
        Ok(guild.clone())
    }

    async fn upsert_command_settings(
        &self,
        guild_id: u64,
        command_id: &str,
        settings: GuildCommandSettings,
    ) -> anyhow::Result<GuildSettings> {
        let mut store = self.store.write().await;
        let guild = store
            .guilds
            .entry(guild_id)
            .or_insert_with(|| GuildSettings::for_guild(guild_id));
        guild.commands.insert(command_id.to_string(), settings);
        Ok(guild.clone())
    }

    async fn upsert_announcement_channel(
        &self,
        guild_id: u64,
        channel_id: Option<u64>,
    ) -> anyhow::Result<GuildSettings> {
        let mut store = self.store.write().await;
        let guild = store
            .guilds
            .entry(guild_id)
            .or_insert_with(|| GuildSettings::for_guild(guild_id));
        guild.announcement_channel_id = channel_id;
        Ok(guild.clone())
    }

    async fn reset_guild(&self, guild_id: u64) -> anyhow::Result<()> {
        let mut store = self.store.write().await;
        store
            .guilds
            .insert(guild_id, GuildSettings::for_guild(guild_id));
        Ok(())
    }
}

#[async_trait::async_trait]
impl ProviderStateRepository for PreviewRepositories {
    async fn load_json(&self, provider_id: &str) -> anyhow::Result<Option<serde_json::Value>> {
        Ok(self.store.read().await.providers.get(provider_id).cloned())
    }

    async fn save_json(&self, provider_id: &str, value: serde_json::Value) -> anyhow::Result<()> {
        let mut store = self.store.write().await;
        if provider_id == COMMAND_SYNC_PROVIDER_ID
            && let Ok(mut sync_store) =
                serde_json::from_value::<CommandSyncStateStore>(value.clone())
        {
            let now = chrono::Utc::now();
            if sync_store.global.has_pending_request() {
                let (fingerprint, count) =
                    dynamo_app::application_command_fingerprint_for_scope(&store.deployment, None);
                sync_store.global.mark_success(now, fingerprint, count);
            }
            for guild_id in sync_store.pending_guild_ids() {
                let guild = store
                    .guilds
                    .get(&guild_id)
                    .cloned()
                    .unwrap_or_else(|| GuildSettings::for_guild(guild_id));
                let (fingerprint, count) = dynamo_app::application_command_fingerprint_for_scope(
                    &store.deployment,
                    Some(&guild),
                );
                sync_store
                    .guild_mut(guild_id)
                    .mark_success(now, fingerprint, count);
            }
            store
                .providers
                .insert(provider_id.to_string(), serde_json::to_value(sync_store)?);
            return Ok(());
        }
        if provider_id == ANNOUNCEMENT_QUEUE_PROVIDER_ID
            && let Ok(mut queue) = serde_json::from_value::<OwnerAnnouncementStore>(value.clone())
        {
            let now = chrono::Utc::now();
            let targeted = store
                .guilds
                .values()
                .filter(|guild| {
                    guild.announcement_channel_id.is_some()
                        && !store.deployment.blocked_guild_ids.contains(&guild.guild_id)
                })
                .count();
            for entry in &mut queue.entries {
                if entry.status == AnnouncementStatus::Queued {
                    entry.processed_at = Some(now);
                    entry.targeted_guilds = targeted;
                    entry.delivered_guilds = targeted;
                    entry.failed_guilds = 0;
                    entry.status = if targeted == 0 {
                        AnnouncementStatus::NoSubscribers
                    } else {
                        AnnouncementStatus::Delivered
                    };
                }
            }
            store
                .providers
                .insert(provider_id.to_string(), serde_json::to_value(queue)?);
            return Ok(());
        }
        store.providers.insert(provider_id.to_string(), value);
        Ok(())
    }
}

#[async_trait::async_trait]
impl DashboardAuditLogRepository for PreviewRepositories {
    async fn append(
        &self,
        mut entry: DashboardAuditLogEntry,
    ) -> anyhow::Result<DashboardAuditLogEntry> {
        let mut store = self.store.write().await;
        let id = store.next_audit_id;
        store.next_audit_id += 1;
        entry.id = Some(format!("preview-log-{id}"));
        store.audit_logs.push(entry.clone());
        Ok(entry)
    }

    async fn list(&self, query: DashboardAuditLogQuery) -> anyhow::Result<DashboardAuditLogPage> {
        let store = self.store.read().await;
        let page = query.page.max(1);
        let page_size = query.page_size.max(1);
        let mut filtered = store
            .audit_logs
            .iter()
            .filter(|entry| {
                query.scope.is_none_or(|scope| entry.scope == scope)
                    && query
                        .guild_id
                        .is_none_or(|guild| entry.guild_id == Some(guild))
                    && query
                        .entity_type
                        .is_none_or(|kind| entry.entity_type == kind)
                    && query.action.is_none_or(|action| entry.action == action)
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| right.timestamp.cmp(&left.timestamp));
        let total = filtered.len() as u64;
        let start = ((page - 1) * page_size) as usize;
        let entries = filtered
            .into_iter()
            .skip(start)
            .take(page_size as usize)
            .collect();
        Ok(DashboardAuditLogPage {
            entries,
            page,
            page_size,
            total,
        })
    }
}

pub(crate) fn build_preview_state(port: u16) -> anyhow::Result<Arc<DashboardState>> {
    let registry = dynamo_app::module_registry();
    let module_catalog = registry.catalog().clone();
    let command_catalog = registry.command_catalog().clone();
    let repositories = Arc::new(PreviewRepositories::new());
    let guild_settings: Arc<dyn GuildSettingsRepository> = repositories.clone();
    let deployment_settings: Arc<dyn DeploymentSettingsRepository> = repositories.clone();
    let provider_state: Arc<dyn ProviderStateRepository> = repositories.clone();
    let audit_logs: Arc<dyn DashboardAuditLogRepository> = repositories;
    let persistence = Persistence::new(
        Some("dynamo_local_preview".to_string()),
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
    let demo_totp_secret = BASE32_NOPAD.decode(DEMO_TOTP_SECRET_BASE32.as_bytes()).ok();
    Ok(Arc::new(DashboardState {
        config: DashboardConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            public_base_url: format!("http://127.0.0.1:{port}"),
            bot_token: String::new(),
            client_secret: String::new(),
            invite_permissions: 0,
            owner_totp_secret: demo_totp_secret,
            register_globally: false,
            command_sync_interval_seconds: 15,
        },
        http: build_dashboard_http_client()?,
        discord_api_base: DISCORD_API_BASE.to_string(),
        app_info: DiscordApplicationInfo {
            id: "preview-app".to_string(),
            name: "Dynamo Preview".to_string(),
            icon: None,
            owner_user_id: Some(PREVIEW_USER_ID),
        },
        module_catalog,
        command_catalog,
        persistence,
        sessions: Default::default(),
        oauth_states: Default::default(),
        owner_security: Default::default(),
        #[cfg(feature = "perf-harness")]
        perf_runtime: None,
        local_preview: Some(Arc::new(LocalPreviewRuntime::default())),
    }))
}

fn preview_port_from_env() -> anyhow::Result<u16> {
    match env::var(PREVIEW_PORT_ENV) {
        Ok(value) => value
            .trim()
            .parse::<u16>()
            .map_err(|error| anyhow::anyhow!("{PREVIEW_PORT_ENV} must be a valid u16: {error}")),
        Err(env::VarError::NotPresent) => Ok(DEFAULT_PREVIEW_PORT),
        Err(error) => Err(anyhow::anyhow!(
            "{PREVIEW_PORT_ENV} could not be read: {error}"
        )),
    }
}

pub async fn run_local_preview() -> anyhow::Result<()> {
    init_tracing();
    let port = preview_port_from_env()?;
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    let actual_port = listener.local_addr()?.port();
    let state = build_preview_state(actual_port)?;
    let catalog_summary = catalog_startup_summary(&state.module_catalog, &state.command_catalog);
    let app = build_dashboard_router(state);

    println!("Dynamo Dashboard Local Preview (offline in-memory mode)");
    println!(
        "  Loaded {} modules and {} commands",
        catalog_summary.module_count, catalog_summary.discovered_leaf_command_count
    );
    println!("  Landing:           http://127.0.0.1:{actual_port}/");
    println!("  Server selector:   http://127.0.0.1:{actual_port}/selector");
    println!("  Dynamo Lounge:     http://127.0.0.1:{actual_port}/guild/{PREVIEW_GUILD_LOUNGE}");
    println!("  FFXIV Raid Hub:    http://127.0.0.1:{actual_port}/guild/{PREVIEW_GUILD_RAID}");
    println!("  Deployment:        http://127.0.0.1:{actual_port}/deployment");
    println!("Press Ctrl+C to stop.");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    use super::*;

    fn preview_router() -> (axum::Router, Arc<DashboardState>) {
        let state = build_preview_state(3000).expect("preview state");
        (build_dashboard_router(state.clone()), state)
    }

    async fn sign_in_cookie_and_csrf(
        app: &axum::Router,
        state: &DashboardState,
    ) -> (String, String) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login?redirect=%2Fselector")
                    .body(Body::empty())
                    .expect("login request"),
            )
            .await
            .expect("login response");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/selector");
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .expect("session cookie header")
            .split(';')
            .next()
            .expect("session cookie pair")
            .to_string();
        let session_id = cookie.split_once('=').expect("cookie pair").1;
        let csrf_token = state
            .sessions
            .read()
            .await
            .get(session_id)
            .expect("session in state")
            .csrf_token
            .clone();
        (cookie, csrf_token)
    }

    #[tokio::test]
    async fn preview_login_creates_session_and_redirects_to_target() {
        let (app, state) = preview_router();
        let unauthenticated = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/selector")
                    .body(Body::empty())
                    .expect("selector request"),
            )
            .await
            .expect("selector redirect");
        assert_eq!(unauthenticated.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            unauthenticated.headers()[header::LOCATION],
            "/login?redirect=%2Fselector"
        );

        let (cookie, _) = sign_in_cookie_and_csrf(&app, &state).await;
        let selector = app
            .oneshot(
                Request::builder()
                    .uri("/selector")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .expect("authenticated selector"),
            )
            .await
            .expect("selector page");
        assert_eq!(selector.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(selector.into_body(), 2 * 1024 * 1024)
                .await
                .expect("selector body")
                .to_vec(),
        )
        .expect("UTF-8 selector HTML");
        assert!(body.contains("Dynamo Lounge"));
        assert!(body.contains("FFXIV Raid Hub"));
        assert!(body.contains("Sandbox Test Server"));
    }

    #[tokio::test]
    async fn preview_supports_mutations_audit_logs_resources_and_simulated_sync() {
        let (app, state) = preview_router();
        let (cookie, csrf_token) = sign_in_cookie_and_csrf(&app, &state).await;

        let resources = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/guild-resources/{PREVIEW_GUILD_LOUNGE}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("resources request"),
            )
            .await
            .expect("resources response");
        assert_eq!(resources.status(), StatusCode::OK);
        let resources_json: serde_json::Value = serde_json::from_slice(
            &to_bytes(resources.into_body(), 1024 * 1024)
                .await
                .expect("resources body"),
        )
        .expect("resources JSON");
        assert!(
            !resources_json["channels"]
                .as_array()
                .expect("channels")
                .is_empty()
        );
        assert!(
            !resources_json["roles"]
                .as_array()
                .expect("roles")
                .is_empty()
        );

        let patch = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/guild-settings/{PREVIEW_GUILD_LOUNGE}/stock"))
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &csrf_token)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "enabled": true,
                            "configuration": { "default_symbol": "AAPL" }
                        })
                        .to_string(),
                    ))
                    .expect("patch request"),
            )
            .await
            .expect("patch response");
        assert_eq!(patch.status(), StatusCode::OK);

        let logs = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/guild/{PREVIEW_GUILD_LOUNGE}?tab=logs"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("logs request"),
            )
            .await
            .expect("logs response");
        assert_eq!(logs.status(), StatusCode::OK);
        let logs_html = String::from_utf8(
            to_bytes(logs.into_body(), 2 * 1024 * 1024)
                .await
                .expect("logs body")
                .to_vec(),
        )
        .expect("UTF-8 logs HTML");
        assert!(logs_html.contains("Stock"));
        assert!(logs_html.contains("Enabled"));

        let before_sync = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/guild-state/{PREVIEW_GUILD_LOUNGE}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("state before sync"),
            )
            .await
            .expect("state before sync response");
        let before_json: serde_json::Value = serde_json::from_slice(
            &to_bytes(before_sync.into_body(), 1024 * 1024)
                .await
                .expect("before sync body"),
        )
        .expect("before sync JSON");
        assert_eq!(before_json["sync"]["tone"], "warn");

        let sync = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/guild-command-sync/{PREVIEW_GUILD_LOUNGE}"))
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &csrf_token)
                    .body(Body::empty())
                    .expect("sync request"),
            )
            .await
            .expect("sync response");
        assert_eq!(sync.status(), StatusCode::OK);

        let after_sync = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/guild-state/{PREVIEW_GUILD_LOUNGE}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("state after sync"),
            )
            .await
            .expect("state after sync response");
        let after_json: serde_json::Value = serde_json::from_slice(
            &to_bytes(after_sync.into_body(), 1024 * 1024)
                .await
                .expect("after sync body"),
        )
        .expect("after sync JSON");
        assert_eq!(after_json["sync"]["tone"], "success");
    }

    #[tokio::test]
    async fn preview_auto_selects_korean_or_english_from_accept_language() {
        let (app, _) = preview_router();

        for (accept_language, cookie, expected_lang) in [
            ("ko-KR,ko;q=0.9,en-US;q=0.8", None, "lang=\"ko\""),
            ("ja-JP,ja;q=0.9", None, "lang=\"en\""),
            ("en-US,en;q=0.9", Some("dynamo_lang=ko"), "lang=\"ko\""),
        ] {
            let mut builder = Request::builder()
                .uri("/")
                .header(header::ACCEPT_LANGUAGE, accept_language);
            if let Some(cookie) = cookie {
                builder = builder.header(header::COOKIE, cookie);
            }
            let response = app
                .clone()
                .oneshot(builder.body(Body::empty()).expect("landing request"))
                .await
                .expect("landing response");
            assert_eq!(response.status(), StatusCode::OK);
            let html = String::from_utf8(
                to_bytes(response.into_body(), 2 * 1024 * 1024)
                    .await
                    .expect("landing body")
                    .to_vec(),
            )
            .expect("UTF-8 landing HTML");
            assert!(
                html.contains(expected_lang),
                "expected {expected_lang} for Accept-Language={accept_language}, cookie={cookie:?}"
            );
        }
    }
}
