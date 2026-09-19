use std::{
    collections::BTreeMap,
    collections::HashMap,
    env, fmt,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
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
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{RwLock, oneshot};

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

const SCHEMA_VERSION: u32 = 1;
const PERF_CONTROL_HEADER: &str = "x-dynamo-perf-control";
const FIXTURE_BYTES: &[u8] = include_bytes!("../../../tests/perf/fixtures/guild-detail-v1.json");

const ENV_REVISION: &str = "DYNAMO_PERF_REVISION";
const ENV_NONCE: &str = "DYNAMO_PERF_NONCE";
const ENV_FIXTURE_MODE: &str = "DYNAMO_PERF_FIXTURE_MODE";
const ENV_FIXTURE_VERSION: &str = "DYNAMO_PERF_FIXTURE_VERSION";
const ENV_FIXTURE_SHA256: &str = "DYNAMO_PERF_FIXTURE_SHA256";
const ENV_READY_FILE: &str = "DYNAMO_PERF_READY_FILE";

const FORBIDDEN_ENVIRONMENT: &[&str] = &[
    "DASHBOARD_HOST",
    "DASHBOARD_PORT",
    "DASHBOARD_BASE_URL",
    "PERF_BASE_URL",
    "DYNAMO_PERF_HOST",
    "DYNAMO_PERF_PORT",
    "DYNAMO_PERF_BASE_URL",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
enum FixtureMode {
    Public,
    GuildDetail,
    ReadOnly,
}

impl FixtureMode {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "Public" => Ok(Self::Public),
            "GuildDetail" => Ok(Self::GuildDetail),
            "ReadOnly" => Ok(Self::ReadOnly),
            _ => anyhow::bail!("{ENV_FIXTURE_MODE} must be Public, GuildDetail, or ReadOnly"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct FixtureIdentity {
    version: String,
    sha256: String,
}

#[derive(Debug, Clone)]
struct PerfHarnessConfig {
    revision: String,
    nonce: String,
    fixture_mode: FixtureMode,
    fixture: FixtureIdentity,
    ready_file: PathBuf,
}

impl PerfHarnessConfig {
    fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> anyhow::Result<Self> {
        for key in FORBIDDEN_ENVIRONMENT {
            ensure!(
                lookup(key).is_none(),
                "{key} is forbidden for the dashboard performance harness"
            );
        }

        let revision = required_value(&mut lookup, ENV_REVISION)?;
        ensure!(
            is_lower_hex(&revision, 40),
            "{ENV_REVISION} must be exactly 40 lowercase hexadecimal characters"
        );

        let nonce = required_value(&mut lookup, ENV_NONCE)?;
        ensure!(
            is_lower_hex(&nonce, 64),
            "{ENV_NONCE} must be exactly 64 lowercase hexadecimal characters"
        );

        let fixture_mode = FixtureMode::parse(&required_value(&mut lookup, ENV_FIXTURE_MODE)?)?;
        let fixture_version = required_value(&mut lookup, ENV_FIXTURE_VERSION)?;
        ensure!(
            is_safe_fixture_version(&fixture_version),
            "{ENV_FIXTURE_VERSION} is invalid"
        );
        let fixture_sha256 = required_value(&mut lookup, ENV_FIXTURE_SHA256)?;
        ensure!(
            is_lower_hex(&fixture_sha256, 64),
            "{ENV_FIXTURE_SHA256} must be exactly 64 lowercase hexadecimal characters"
        );
        ensure!(
            fixture_sha256 == fixture_bytes_sha256(),
            "{ENV_FIXTURE_SHA256} does not match the compiled fixture bytes"
        );

        let ready_file = PathBuf::from(required_value(&mut lookup, ENV_READY_FILE)?);
        validate_ready_path(&ready_file)?;

        Ok(Self {
            revision,
            nonce,
            fixture_mode,
            fixture: FixtureIdentity {
                version: fixture_version,
                sha256: fixture_sha256,
            },
            ready_file,
        })
    }
}

fn validate_compiled_revision(
    runtime_revision: &str,
    compiled_revision: Option<&str>,
) -> anyhow::Result<()> {
    let compiled_revision = compiled_revision
        .context("performance harness binary lacks DYNAMO_PERF_COMPILED_REVISION provenance")?;
    ensure!(
        is_lower_hex(compiled_revision, 40),
        "compiled performance harness revision is invalid"
    );
    ensure!(
        compiled_revision == runtime_revision,
        "compiled performance harness revision does not match DYNAMO_PERF_REVISION"
    );
    Ok(())
}

fn required_value(
    lookup: &mut impl FnMut(&str) -> Option<String>,
    key: &str,
) -> anyhow::Result<String> {
    let value = lookup(key).with_context(|| format!("{key} is required"))?;
    ensure!(!value.is_empty(), "{key} must not be empty");
    Ok(value)
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_safe_fixture_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn validate_ready_path(path: &Path) -> anyhow::Result<()> {
    ensure!(
        path.is_absolute(),
        "{ENV_READY_FILE} must be an absolute path"
    );
    ensure!(
        path.file_name().is_some(),
        "{ENV_READY_FILE} must name a file"
    );
    let parent = path
        .parent()
        .context("DYNAMO_PERF_READY_FILE must have a parent directory")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .context("DYNAMO_PERF_READY_FILE parent directory could not be inspected")?;
    ensure!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "{ENV_READY_FILE} parent must be an existing non-symlink directory"
    );
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("{ENV_READY_FILE} already exists"),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("DYNAMO_PERF_READY_FILE could not be inspected"),
    }
}

fn fixture_bytes_sha256() -> String {
    format!("{:x}", Sha256::digest(FIXTURE_BYTES))
}

fn random_control_secret() -> anyhow::Result<String> {
    let mut random = [0_u8; 32];
    OsRng
        .try_fill_bytes(&mut random)
        .context("failed to obtain operating-system randomness for harness control capability")?;
    let mut secret = String::with_capacity(69);
    secret.push_str("perf_");
    const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in random {
        secret.push(LOWER_HEX[usize::from(byte >> 4)] as char);
        secret.push(LOWER_HEX[usize::from(byte & 0x0f)] as char);
    }
    Ok(secret)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureData {
    schema_version: u32,
    fixture_version: String,
    application: FixtureApplication,
    session: FixtureSession,
    target: FixtureTarget,
    settings: FixtureSettings,
    fake_provider_responses: FixtureProviderResponses,
    route_payloads: FixtureRoutePayloads,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureApplication {
    id: String,
    name: String,
    icon: Option<String>,
    owner_user_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSession {
    id: String,
    expires_at: String,
    user: FixtureUser,
    guilds: Vec<FixtureGuild>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureUser {
    id: String,
    username: String,
    global_name: String,
    avatar: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureGuild {
    id: String,
    name: String,
    icon: Option<String>,
    permissions: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureTarget {
    guild_id: String,
    bot_present: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSettings {
    deployment: FixtureDeploymentSettings,
    guild: FixtureGuildSettings,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureDeploymentSettings {
    modules: BTreeMap<String, DeploymentModuleSettings>,
    commands: BTreeMap<String, DeploymentCommandSettings>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureGuildSettings {
    guild_id: String,
    modules: BTreeMap<String, GuildModuleSettings>,
    commands: BTreeMap<String, GuildCommandSettings>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureProviderResponses {
    bot_present: bool,
    guild_lookup_delay_ms: u64,
    outbound_http_attempts: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureRoutePayloads {
    public_padding_bytes: usize,
    guild_detail_padding_bytes: usize,
}

impl FixtureData {
    fn load(config: &PerfHarnessConfig) -> anyhow::Result<Self> {
        let fixture: Self = serde_json::from_slice(FIXTURE_BYTES)
            .context("compiled dashboard performance fixture is invalid JSON")?;
        ensure!(
            fixture.schema_version == SCHEMA_VERSION,
            "compiled dashboard performance fixture schema is unsupported"
        );
        ensure!(
            fixture.fixture_version == config.fixture.version,
            "compiled fixture version does not match DYNAMO_PERF_FIXTURE_VERSION"
        );
        let application_id = fixture
            .application
            .id
            .parse::<u64>()
            .context("fixture application id must be a u64")?;
        ensure!(
            application_id > 0,
            "fixture application id must be non-zero"
        );
        let user_id = fixture
            .session
            .user
            .id
            .parse::<u64>()
            .context("fixture user id must be a u64")?;
        ensure!(user_id > 0, "fixture user id must be non-zero");
        let owner_user_id = fixture
            .application
            .owner_user_id
            .parse::<u64>()
            .context("fixture application owner id must be a u64")?;
        ensure!(
            owner_user_id == user_id,
            "fixture application owner must be the fixture user"
        );
        ensure!(
            application_id != user_id,
            "fixture application and user ids must differ"
        );
        ensure!(
            fixture.application.icon.is_none() && fixture.session.user.avatar.is_none(),
            "fixture application icon and user avatar must be null"
        );
        ensure!(
            fixture.session.guilds.len() == 100,
            "fixture must contain exactly 100 guilds"
        );
        let target_guild_id = fixture
            .target
            .guild_id
            .parse::<u64>()
            .context("fixture target guild id must be a u64")?;
        ensure!(
            target_guild_id > 0,
            "fixture target guild id must be non-zero"
        );
        let mut guild_ids = std::collections::HashSet::with_capacity(100);
        let mut target_count = 0;
        for guild in &fixture.session.guilds {
            let guild_id = guild
                .id
                .parse::<u64>()
                .context("fixture guild id must be a u64")?;
            ensure!(guild_id > 0, "fixture guild id must be non-zero");
            ensure!(
                guild_id != application_id && guild_id != user_id,
                "fixture application, user, and guild ids must differ"
            );
            ensure!(
                guild_ids.insert(guild_id),
                "fixture guild ids must be unique"
            );
            target_count += usize::from(guild_id == target_guild_id);
            ensure!(guild.icon.is_none(), "fixture guild icons must be null");
            let permission_bits = guild
                .permissions
                .parse::<u64>()
                .context("fixture guild permissions must be a u64")?;
            ensure!(
                permission_bits & (1 << 5) != 0 || permission_bits & (1 << 3) != 0,
                "every fixture guild must grant manage-guild or administrator permission"
            );
            ensure!(
                !guild.name.is_empty(),
                "fixture guild names must not be empty"
            );
        }
        ensure!(
            target_count == 1,
            "fixture target guild must occur exactly once in the session guild list"
        );
        ensure!(
            !fixture.application.name.is_empty()
                && !fixture.session.user.username.is_empty()
                && !fixture.session.user.global_name.is_empty(),
            "fixture display names must not be empty"
        );
        ensure!(
            fixture.session.id.len() <= 128 && is_safe_cookie_value(&fixture.session.id),
            "fixture session id must be a safe non-empty cookie value"
        );
        fixture
            .session_expires_at()
            .context("fixture session expiry must be RFC 3339")?;
        ensure!(
            fixture.target.bot_present && fixture.fake_provider_responses.bot_present,
            "fixture bot presence responses must be consistently true"
        );
        ensure!(
            fixture.fake_provider_responses.guild_lookup_delay_ms <= 5_000,
            "fixture guild lookup delay exceeds the safety limit"
        );
        ensure!(
            fixture.fake_provider_responses.outbound_http_attempts == 0,
            "fixture must declare zero outbound HTTP attempts"
        );
        ensure!(
            fixture.settings.guild.guild_id == fixture.target.guild_id,
            "fixture guild settings id must equal the target guild id"
        );
        let expected_deployment_modules = BTreeMap::from([(
            "stock".to_string(),
            DeploymentModuleSettings {
                installed: true,
                enabled: true,
            },
        )]);
        let expected_deployment_commands = BTreeMap::from([(
            "etf".to_string(),
            DeploymentCommandSettings {
                installed: true,
                enabled: false,
                configuration: serde_json::json!({ "ticker_1": "DEPLOYMENT-CANARY" }),
            },
        )]);
        let expected_guild_modules = BTreeMap::from([(
            "stock".to_string(),
            GuildModuleSettings {
                enabled: false,
                configuration: serde_json::json!({
                    "default_symbol": "PERF-STOCK-CANARY",
                    "etf_tickers": ["SPY"],
                    "refresh_interval_seconds": 3,
                    "refresh_duration_seconds": 60,
                }),
            },
        )]);
        let expected_guild_commands = BTreeMap::from([(
            "etf".to_string(),
            GuildCommandSettings {
                enabled: true,
                configuration: serde_json::json!({ "ticker_1": "GUILD-ETF-CANARY" }),
            },
        )]);
        ensure!(
            fixture.settings.deployment.modules == expected_deployment_modules
                && fixture.settings.deployment.commands == expected_deployment_commands
                && fixture.settings.guild.modules == expected_guild_modules
                && fixture.settings.guild.commands == expected_guild_commands,
            "fixture settings must match the exact non-default dashboard canary"
        );
        // These values fingerprint the fixture ABI only. They are deliberately not carried into
        // DashboardState or injected into responses: measured bytes must come from the production
        // renderer itself.
        ensure!(
            fixture.route_payloads.public_padding_bytes == 0
                && fixture.route_payloads.guild_detail_padding_bytes == 4096,
            "fixture route payload provenance metadata is invalid"
        );
        Ok(fixture)
    }

    fn user_id(&self) -> u64 {
        self.session
            .user
            .id
            .parse()
            .expect("validated fixture user id")
    }

    fn guild_id(&self) -> u64 {
        self.target
            .guild_id
            .parse()
            .expect("validated fixture target guild id")
    }

    fn owner_user_id(&self) -> u64 {
        self.application
            .owner_user_id
            .parse()
            .expect("validated fixture owner user id")
    }

    fn session_expires_at(&self) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::parse_from_rfc3339(&self.session.expires_at)
            .map(|value| value.with_timezone(&chrono::Utc))
            .map_err(Into::into)
    }
}

fn is_safe_cookie_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub(super) struct PerfRuntime {
    revision: String,
    nonce: String,
    fixture_mode: FixtureMode,
    fixture: FixtureIdentity,
    guild_id: u64,
    guild_path: String,
    cookie_value: String,
    bot_present: bool,
    guild_lookup_delay_ms: u64,
    // Counts server egress attempts denied before reqwest::send, not successful network calls.
    outbound_calls: AtomicU64,
    browser_outbound_attempts: AtomicU64,
    denied_requests: AtomicU64,
    server_write_attempts: AtomicU64,
    repository_reads: AtomicU64,
    repository_mutations: AtomicU64,
    provider_guild_lookups: AtomicU64,
    shutdown_sender: Mutex<Option<oneshot::Sender<()>>>,
}

impl fmt::Debug for PerfRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PerfRuntime")
            .field("revision", &self.revision)
            .field("nonce", &self.nonce)
            .field("fixture_mode", &self.fixture_mode)
            .field("fixture", &self.fixture)
            .field("guild_id", &self.guild_id)
            .field("guild_path", &self.guild_path)
            .field("cookie_value", &"[redacted]")
            .field("bot_present", &self.bot_present)
            .field("guild_lookup_delay_ms", &self.guild_lookup_delay_ms)
            .field("outbound_calls", &self.outbound_calls)
            .field("browser_outbound_attempts", &self.browser_outbound_attempts)
            .field("denied_requests", &self.denied_requests)
            .field("server_write_attempts", &self.server_write_attempts)
            .field("repository_reads", &self.repository_reads)
            .field("repository_mutations", &self.repository_mutations)
            .field("provider_guild_lookups", &self.provider_guild_lookups)
            .field("shutdown_sender", &"[redacted]")
            .finish()
    }
}

impl PerfRuntime {
    fn new(
        config: &PerfHarnessConfig,
        fixture: &FixtureData,
        shutdown_sender: oneshot::Sender<()>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            revision: config.revision.clone(),
            nonce: config.nonce.clone(),
            fixture_mode: config.fixture_mode,
            fixture: config.fixture.clone(),
            guild_id: fixture.guild_id(),
            guild_path: format!("/guild/{}", fixture.guild_id()),
            cookie_value: random_control_secret()?,
            bot_present: fixture.target.bot_present,
            guild_lookup_delay_ms: fixture.fake_provider_responses.guild_lookup_delay_ms,
            outbound_calls: AtomicU64::new(0),
            browser_outbound_attempts: AtomicU64::new(0),
            denied_requests: AtomicU64::new(0),
            server_write_attempts: AtomicU64::new(0),
            repository_reads: AtomicU64::new(0),
            repository_mutations: AtomicU64::new(0),
            provider_guild_lookups: AtomicU64::new(0),
            shutdown_sender: Mutex::new(Some(shutdown_sender)),
        })
    }

    pub(super) async fn fixture_bot_present(&self) -> bool {
        self.provider_guild_lookups.fetch_add(1, Ordering::SeqCst);
        if self.guild_lookup_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(self.guild_lookup_delay_ms)).await;
        }
        self.bot_present
    }

    pub(super) fn deny_outbound(&self) {
        self.outbound_calls.fetch_add(1, Ordering::SeqCst);
    }

    fn increment_browser_outbound_attempts(&self) {
        self.browser_outbound_attempts
            .fetch_add(1, Ordering::SeqCst);
    }

    fn increment_server_write_attempts(&self) {
        self.server_write_attempts.fetch_add(1, Ordering::SeqCst);
    }

    fn increment_denied_requests(&self) {
        self.denied_requests.fetch_add(1, Ordering::SeqCst);
    }

    fn increment_repository_reads(&self) {
        self.repository_reads.fetch_add(1, Ordering::SeqCst);
    }

    fn increment_repository_mutations(&self) {
        self.repository_mutations.fetch_add(1, Ordering::SeqCst);
    }

    fn trigger_shutdown(&self) -> bool {
        self.shutdown_sender
            .lock()
            .expect("performance shutdown lock poisoned")
            .take()
            .is_some_and(|sender| sender.send(()).is_ok())
    }

    fn guild_path(&self) -> &str {
        &self.guild_path
    }
}

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
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        fs,
        path::PathBuf,
        sync::{Arc, Barrier},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use serde_json::Value;
    use tokio::sync::oneshot;
    use tower::ServiceExt;

    use super::*;

    fn unique_temp_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        env::temp_dir().join(format!(
            "dynamo-dashboard-perf-{label}-{}-{unique}",
            process::id()
        ))
    }

    fn valid_environment(ready_file: &Path) -> HashMap<String, String> {
        HashMap::from([
            (ENV_REVISION.to_string(), "a".repeat(40)),
            (ENV_NONCE.to_string(), "b".repeat(64)),
            (ENV_FIXTURE_MODE.to_string(), "GuildDetail".to_string()),
            (
                ENV_FIXTURE_VERSION.to_string(),
                "guild-detail-v1".to_string(),
            ),
            (ENV_FIXTURE_SHA256.to_string(), fixture_bytes_sha256()),
            (
                ENV_READY_FILE.to_string(),
                ready_file.to_string_lossy().into_owned(),
            ),
        ])
    }

    fn parse_config(values: &HashMap<String, String>) -> anyhow::Result<PerfHarnessConfig> {
        PerfHarnessConfig::from_lookup(|key| values.get(key).cloned())
    }

    fn fixture_app() -> (
        Router,
        Arc<PerfRuntime>,
        oneshot::Receiver<()>,
        PerfHarnessConfig,
    ) {
        fixture_app_for_mode(FixtureMode::ReadOnly)
    }

    fn fixture_app_for_mode(
        fixture_mode: FixtureMode,
    ) -> (
        Router,
        Arc<PerfRuntime>,
        oneshot::Receiver<()>,
        PerfHarnessConfig,
    ) {
        let config = PerfHarnessConfig {
            revision: "a".repeat(40),
            nonce: "b".repeat(64),
            fixture_mode,
            fixture: FixtureIdentity {
                version: "guild-detail-v1".to_string(),
                sha256: fixture_bytes_sha256(),
            },
            ready_file: env::temp_dir().join("unused-ready-file.json"),
        };
        let fixture = FixtureData::load(&config).expect("valid compiled fixture");
        let (sender, receiver) = oneshot::channel();
        let runtime =
            Arc::new(PerfRuntime::new(&config, &fixture, sender).expect("performance runtime"));
        let state =
            build_fixture_state(&fixture, runtime.clone(), 45678).expect("fixture dashboard state");
        (
            build_perf_router(state, runtime.clone()),
            runtime,
            receiver,
            config,
        )
    }

    fn request(method: &str, uri: &str, cookie: Option<(&str, &str)>) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some((name, value)) = cookie {
            builder = builder.header(header::COOKIE, format!("{name}={value}"));
        }
        builder.body(Body::empty()).expect("valid request")
    }

    async fn json_body(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("bounded response body");
        serde_json::from_slice(&bytes).expect("JSON response")
    }

    #[test]
    fn config_rejects_missing_malformed_and_caller_network_values() {
        let directory = unique_temp_directory("config-negative");
        fs::create_dir(&directory).expect("create temp directory");
        let ready_file = directory.join("ready.json");
        let valid = valid_environment(&ready_file);
        assert!(parse_config(&valid).is_ok());

        for key in [
            ENV_REVISION,
            ENV_NONCE,
            ENV_FIXTURE_MODE,
            ENV_FIXTURE_VERSION,
            ENV_FIXTURE_SHA256,
            ENV_READY_FILE,
        ] {
            let mut values = valid.clone();
            values.remove(key);
            assert!(parse_config(&values).is_err(), "missing {key} was accepted");
        }

        let invalid_values = [
            (ENV_REVISION, "A".repeat(40)),
            (ENV_NONCE, "g".repeat(64)),
            (ENV_FIXTURE_MODE, "Production".to_string()),
            (ENV_FIXTURE_VERSION, "Unsafe Version".to_string()),
            (ENV_FIXTURE_SHA256, "0".repeat(64)),
            (ENV_READY_FILE, "relative-ready.json".to_string()),
        ];
        for (key, value) in invalid_values {
            let mut values = valid.clone();
            values.insert(key.to_string(), value);
            assert!(parse_config(&values).is_err(), "invalid {key} was accepted");
        }

        for key in FORBIDDEN_ENVIRONMENT {
            let mut values = valid.clone();
            values.insert((*key).to_string(), "forbidden".to_string());
            assert!(
                parse_config(&values).is_err(),
                "forbidden {key} was accepted"
            );
        }
        fs::remove_dir_all(directory).expect("remove temp directory");
    }

    #[test]
    fn compiled_revision_provenance_is_required_and_must_match() {
        let revision = "a".repeat(40);
        assert!(validate_compiled_revision(&revision, None).is_err());
        assert!(validate_compiled_revision(&revision, Some(&"b".repeat(40))).is_err());
        assert!(validate_compiled_revision(&revision, Some("not-a-revision")).is_err());
        assert!(validate_compiled_revision(&revision, Some(&revision)).is_ok());
    }

    #[test]
    fn ready_file_is_create_new_and_contains_no_uri_or_real_credentials() {
        let directory = unique_temp_directory("ready");
        fs::create_dir(&directory).expect("create temp directory");
        let path = directory.join("ready.json");
        let fixture = FixtureIdentity {
            version: "guild-detail-v1".to_string(),
            sha256: fixture_bytes_sha256(),
        };
        let revision = "a".repeat(40);
        let nonce = "b".repeat(64);
        let cookie_value = format!("perf_{}", "c".repeat(64));
        let ready = ReadyFile {
            schema_version: SCHEMA_VERSION,
            host: "127.0.0.1",
            dynamic_port: true,
            port: 34567,
            pid: process::id(),
            revision: &revision,
            nonce: &nonce,
            fixture_mode: FixtureMode::ReadOnly,
            fixture: &fixture,
            guild_id: 100000000000000003,
            cookie_name: SESSION_COOKIE_NAME,
            cookie_value: &cookie_value,
        };
        write_ready_file(&path, &ready).expect("first ready write");
        let body = fs::read_to_string(&path).expect("read ready file");
        let parsed: Value = serde_json::from_str(&body).expect("ready JSON");
        let keys = parsed
            .as_object()
            .expect("ready object")
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(
            keys,
            HashSet::from_iter(
                [
                    "schema_version",
                    "host",
                    "dynamic_port",
                    "port",
                    "pid",
                    "revision",
                    "nonce",
                    "fixture_mode",
                    "fixture",
                    "guild_id",
                    "cookie_name",
                    "cookie_value",
                ]
                .map(str::to_string)
            )
        );
        assert!(!body.contains("mongodb://"));
        assert!(!body.contains("discord.com"));
        assert!(!body.contains("client_secret"));
        assert!(write_ready_file(&path, &ready).is_err());
        assert_eq!(
            fs::read_to_string(&path).expect("read preserved file"),
            body
        );
        fs::remove_dir_all(directory).expect("remove temp directory");
    }

    #[test]
    fn concurrent_ready_publish_has_one_complete_winner_and_no_temp_residue() {
        let directory = unique_temp_directory("ready-concurrent");
        fs::create_dir(&directory).expect("create temp directory");
        let path = Arc::new(directory.join("ready.json"));
        let bodies = Arc::new(
            (0..8)
                .map(|writer| {
                    format!(
                        "{{\"writer\":{writer},\"padding\":\"{}\"}}\n",
                        "x".repeat(64 * 1024)
                    )
                    .into_bytes()
                })
                .collect::<Vec<_>>(),
        );
        let barrier = Arc::new(Barrier::new(bodies.len()));
        let reader_path = path.clone();
        let reader_bodies = bodies.clone();
        let reader = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match fs::read(reader_path.as_ref()) {
                    Ok(body) => {
                        assert!(reader_bodies.iter().any(|candidate| candidate == &body));
                        serde_json::from_slice::<Value>(&body).expect("published JSON is complete");
                        break;
                    }
                    Err(error)
                        if error.kind() == ErrorKind::NotFound && Instant::now() < deadline =>
                    {
                        thread::yield_now();
                    }
                    Err(error) if error.kind() == ErrorKind::NotFound => {
                        panic!("timed out waiting for a ready-file publisher")
                    }
                    Err(error) => panic!("polling reader failed: {error}"),
                }
            }
        });

        let publishers = (0..bodies.len())
            .map(|index| {
                let body = bodies[index].clone();
                let barrier = barrier.clone();
                let path = path.clone();
                thread::spawn(move || {
                    barrier.wait();
                    publish_ready_bytes(path.as_ref(), &body).is_ok()
                })
            })
            .collect::<Vec<_>>();
        let winners = publishers
            .into_iter()
            .map(|publisher| publisher.join().expect("publisher thread"))
            .filter(|won| *won)
            .count();
        reader.join().expect("polling reader thread");

        assert_eq!(winners, 1);
        let published = fs::read(path.as_ref()).expect("published ready file");
        assert!(bodies.iter().any(|candidate| candidate == &published));
        serde_json::from_slice::<Value>(&published).expect("final ready JSON is complete");
        let residue = fs::read_dir(&directory)
            .expect("read ready directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.path() != path.as_path())
            .count();
        assert_eq!(residue, 0);
        fs::remove_dir_all(directory).expect("remove temp directory");
    }

    #[test]
    fn compiled_fixture_version_must_match_environment_version() {
        let mut config = PerfHarnessConfig {
            revision: "a".repeat(40),
            nonce: "b".repeat(64),
            fixture_mode: FixtureMode::GuildDetail,
            fixture: FixtureIdentity {
                version: "wrong-version".to_string(),
                sha256: fixture_bytes_sha256(),
            },
            ready_file: env::temp_dir().join("unused-ready-file.json"),
        };
        assert!(FixtureData::load(&config).is_err());
        config.fixture.version = "guild-detail-v1".to_string();
        let fixture = FixtureData::load(&config).expect("matching rich fixture");
        assert_eq!(fixture.session.guilds.len(), 100);
        assert_eq!(fixture.guild_id(), 9000000000000000101);
        assert_eq!(
            fixture.settings.deployment.modules.get("stock"),
            Some(&DeploymentModuleSettings {
                installed: true,
                enabled: true,
            })
        );
        assert_eq!(
            fixture.settings.deployment.commands.get("etf"),
            Some(&DeploymentCommandSettings {
                installed: true,
                enabled: false,
                configuration: serde_json::json!({ "ticker_1": "DEPLOYMENT-CANARY" }),
            })
        );
        assert_eq!(
            fixture.settings.guild.modules.get("stock"),
            Some(&GuildModuleSettings {
                enabled: false,
                configuration: serde_json::json!({
                    "default_symbol": "PERF-STOCK-CANARY",
                    "etf_tickers": ["SPY"],
                    "refresh_interval_seconds": 3,
                    "refresh_duration_seconds": 60,
                }),
            })
        );
        assert_eq!(
            fixture.settings.guild.commands.get("etf"),
            Some(&GuildCommandSettings {
                enabled: true,
                configuration: serde_json::json!({ "ticker_1": "GUILD-ETF-CANARY" }),
            })
        );
        assert_eq!(fixture.route_payloads.public_padding_bytes, 0);
        assert_eq!(fixture.route_payloads.guild_detail_padding_bytes, 4096);
    }

    #[tokio::test]
    async fn fixture_modes_enforce_distinct_read_allowlists_and_live_evidence() {
        let (public_app, public_runtime, _shutdown, _) = fixture_app_for_mode(FixtureMode::Public);
        assert_eq!(
            public_app
                .clone()
                .oneshot(request("GET", "/", None))
                .await
                .expect("public root")
                .status(),
            StatusCode::OK
        );
        for denied in ["/selector", public_runtime.guild_path()] {
            assert_eq!(
                public_app
                    .clone()
                    .oneshot(request("GET", denied, None))
                    .await
                    .expect("public denied route")
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        let public_counters = json_body(
            public_app
                .oneshot(request("GET", "/__perf/counters", None))
                .await
                .expect("public counters"),
        )
        .await;
        assert_eq!(public_counters["repository_reads"], 0);
        assert_eq!(public_counters["repository_mutations"], 0);
        assert_eq!(public_counters["provider_guild_lookups"], 0);

        let (guild_app, guild_runtime, _shutdown, _) =
            fixture_app_for_mode(FixtureMode::GuildDetail);
        for denied in ["/", "/selector"] {
            assert_eq!(
                guild_app
                    .clone()
                    .oneshot(request("GET", denied, None))
                    .await
                    .expect("guild-detail denied route")
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            guild_app
                .clone()
                .oneshot(request(
                    "GET",
                    guild_runtime.guild_path(),
                    Some((SESSION_COOKIE_NAME, &guild_runtime.cookie_value)),
                ))
                .await
                .expect("guild-detail target")
                .status(),
            StatusCode::OK
        );
        let guild_counters = json_body(
            guild_app
                .oneshot(request("GET", "/__perf/counters", None))
                .await
                .expect("guild counters"),
        )
        .await;
        assert_eq!(guild_counters["repository_reads"], 3);
        assert_eq!(guild_counters["repository_mutations"], 0);
        assert_eq!(guild_counters["provider_guild_lookups"], 1);

        let (readonly_app, readonly_runtime, _shutdown, _) =
            fixture_app_for_mode(FixtureMode::ReadOnly);
        for allowed in ["/", "/selector", readonly_runtime.guild_path()] {
            let cookie = (allowed != "/")
                .then_some((SESSION_COOKIE_NAME, readonly_runtime.cookie_value.as_str()));
            assert_eq!(
                readonly_app
                    .clone()
                    .oneshot(request("GET", allowed, cookie))
                    .await
                    .expect("read-only allowed route")
                    .status(),
                StatusCode::OK
            );
        }
        assert_eq!(
            readonly_app
                .oneshot(request("GET", "/deployment", None))
                .await
                .expect("read-only denied route")
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn instance_has_exact_node_abi_and_allowed_pages_are_read_only() {
        let (app, runtime, _shutdown, _) = fixture_app();
        let instance_response = app
            .clone()
            .oneshot(request("GET", "/__perf/instance", None))
            .await
            .expect("instance response");
        assert_eq!(instance_response.status(), StatusCode::OK);
        let instance = json_body(instance_response).await;
        assert!(!instance.to_string().contains(&runtime.cookie_value));
        let keys = instance
            .as_object()
            .expect("instance object")
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(
            keys,
            HashSet::from_iter(
                [
                    "schema_version",
                    "revision",
                    "nonce",
                    "pid",
                    "fixture_mode",
                    "fixture",
                    "outbound_calls",
                    "browser_outbound_attempts",
                ]
                .map(str::to_string)
            )
        );

        let public = app
            .clone()
            .oneshot(request("GET", "/", None))
            .await
            .expect("public page");
        assert_eq!(public.status(), StatusCode::OK);

        let selector = app
            .clone()
            .oneshot(request(
                "GET",
                "/selector",
                Some((SESSION_COOKIE_NAME, &runtime.cookie_value)),
            ))
            .await
            .expect("selector page");
        assert_eq!(selector.status(), StatusCode::OK);
        let selector_html = to_bytes(selector.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded selector page");
        let selector_html = String::from_utf8(selector_html.to_vec()).expect("UTF-8 selector page");
        assert!(selector_html.contains(runtime.guild_path()));
        assert!(!selector_html.contains("cdn.discordapp.com"));

        let guild = app
            .clone()
            .oneshot(request(
                "GET",
                runtime.guild_path(),
                Some((SESSION_COOKIE_NAME, &runtime.cookie_value)),
            ))
            .await
            .expect("guild page");
        assert_eq!(guild.status(), StatusCode::OK);
        let guild_html = to_bytes(guild.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded guild page");
        let guild_html = String::from_utf8(guild_html.to_vec()).expect("UTF-8 guild page");
        assert!(!guild_html.contains("cdn.discordapp.com"));
        assert!(
            guild_html.contains(
                "Installed: On | Deployment: On | Local guild: Off | Effective: Off | Blocked by local guild setting"
            )
        );
        assert!(
            guild_html.contains(
                "Parent module: Off | Installed: On | Deployment: Off | Local guild: On | Effective: Off | Blocked by parent module"
            )
        );
        assert!(guild_html.contains("value=\"PERF-STOCK-CANARY\""));
        assert!(guild_html.contains("value=\"GUILD-ETF-CANARY\""));

        for query in [
            "tab=overview",
            "tab=modules",
            "tab=commands",
            "tab=logs&log_entity=module&log_action=toggle&log_page=2",
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    "GET",
                    &format!("{}?{query}", runtime.guild_path()),
                    Some((SESSION_COOKIE_NAME, &runtime.cookie_value)),
                ))
                .await
                .expect("canonical guild query response");
            assert_eq!(response.status(), StatusCode::OK, "query {query}");
        }

        let font = app
            .clone()
            .oneshot(request("GET", FIRA_SANS_REGULAR_PATH, None))
            .await
            .expect("font response");
        assert_eq!(font.status(), StatusCode::OK);
        assert_eq!(font.headers()[header::CONTENT_TYPE], "font/woff2");

        let counters = app
            .oneshot(request("GET", "/__perf/counters", None))
            .await
            .expect("counter response");
        let counters = json_body(counters).await;
        let counter_keys = counters
            .as_object()
            .expect("counter object")
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(
            counter_keys,
            HashSet::from_iter(
                [
                    "schema_version",
                    "outbound_calls",
                    "browser_outbound_attempts",
                    "denied_requests",
                    "server_write_attempts",
                    "repository_reads",
                    "repository_mutations",
                    "provider_guild_lookups",
                ]
                .map(str::to_string)
            )
        );
        assert_eq!(counters["schema_version"], SCHEMA_VERSION);
        assert_eq!(counters["denied_requests"], 0);
        assert_eq!(counters["server_write_attempts"], 0);
        assert_eq!(counters["repository_reads"], 16);
        assert_eq!(counters["repository_mutations"], 0);
        assert_eq!(counters["provider_guild_lookups"], 600);
        assert_eq!(counters["outbound_calls"], 0);
        assert_eq!(counters["browser_outbound_attempts"], 0);
    }

    #[tokio::test]
    async fn outer_guard_blocks_auth_and_business_writes_before_handlers() {
        let (app, _runtime, _shutdown, _) = fixture_app();
        for (method, path) in [
            ("GET", "/login"),
            ("GET", "/logout"),
            ("GET", "/?unexpected=query"),
            ("GET", "/guild/9000000000000000101?tab=logs&tab=modules"),
            ("GET", "/guild/9000000000000000101?tab=logs&unknown=value"),
            ("GET", "/guild/9000000000000000101?tab=logs&log_page=01"),
            ("PATCH", "/api/guild-settings/100000000000000003/info"),
        ] {
            let response = app
                .clone()
                .oneshot(request(method, path, None))
                .await
                .expect("guard response");
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let counters = app
            .oneshot(request("GET", "/__perf/counters", None))
            .await
            .expect("counter response");
        let counters = json_body(counters).await;
        assert_eq!(counters["denied_requests"], 7);
        assert_eq!(counters["server_write_attempts"], 1);
        assert_eq!(counters["repository_mutations"], 0);
    }

    #[tokio::test]
    async fn browser_control_uses_fresh_secret_and_proves_counter_is_live() {
        let (app, runtime, _shutdown, _) = fixture_app();
        let (_other_app, other_runtime, _other_shutdown, _) = fixture_app();
        let secret_hex = runtime
            .cookie_value
            .strip_prefix("perf_")
            .expect("control secret prefix");
        assert!(is_lower_hex(secret_hex, 64));
        assert_ne!(runtime.cookie_value, other_runtime.cookie_value);
        assert_ne!(runtime.cookie_value, runtime.nonce);

        let public_nonce = Request::builder()
            .method("POST")
            .uri("/__perf/browser-outbound-attempt")
            .header("x-dynamo-perf-nonce", &runtime.nonce)
            .body(Body::empty())
            .expect("old control request");
        let public_nonce = app
            .clone()
            .oneshot(public_nonce)
            .await
            .expect("public nonce response");
        assert_eq!(public_nonce.status(), StatusCode::FORBIDDEN);

        let wrong_secret = Request::builder()
            .method("POST")
            .uri("/__perf/browser-outbound-attempt")
            .header(PERF_CONTROL_HEADER, "wrong")
            .body(Body::empty())
            .expect("wrong control request");
        let wrong_secret = app
            .clone()
            .oneshot(wrong_secret)
            .await
            .expect("wrong secret response");
        assert_eq!(wrong_secret.status(), StatusCode::FORBIDDEN);

        let queried = Request::builder()
            .method("POST")
            .uri("/__perf/browser-outbound-attempt?unexpected=query")
            .header(PERF_CONTROL_HEADER, &runtime.cookie_value)
            .body(Body::empty())
            .expect("queried control request");
        let queried = app
            .clone()
            .oneshot(queried)
            .await
            .expect("queried control response");
        assert_eq!(queried.status(), StatusCode::FORBIDDEN);

        let correct = Request::builder()
            .method("POST")
            .uri("/__perf/browser-outbound-attempt")
            .header(PERF_CONTROL_HEADER, &runtime.cookie_value)
            .body(Body::empty())
            .expect("control request");
        let correct = app
            .clone()
            .oneshot(correct)
            .await
            .expect("correct nonce response");
        assert_eq!(correct.status(), StatusCode::NO_CONTENT);

        let counters = app
            .oneshot(request("GET", "/__perf/counters", None))
            .await
            .expect("counter response");
        let counters = json_body(counters).await;
        assert_eq!(counters["denied_requests"], 3);
        assert_eq!(counters["server_write_attempts"], 3);
        assert_eq!(counters["browser_outbound_attempts"], 1);
    }

    #[test]
    fn runtime_debug_redacts_control_capability_and_shutdown_sender() {
        let (_app, runtime, _shutdown, _) = fixture_app();
        let control_secret = runtime.cookie_value.clone();
        let debug = format!("{runtime:?}");

        assert!(!debug.contains(&control_secret));
        assert!(debug.contains("cookie_value: \"[redacted]\""));
        assert!(debug.contains("shutdown_sender: \"[redacted]\""));
    }

    #[tokio::test]
    async fn authenticated_shutdown_completes_graceful_signal() {
        let (app, runtime, mut shutdown, _) = fixture_app();
        let wrong = Request::builder()
            .method("POST")
            .uri("/__perf/shutdown")
            .header(PERF_CONTROL_HEADER, "wrong")
            .body(Body::empty())
            .expect("shutdown request");
        assert_eq!(
            app.clone()
                .oneshot(wrong)
                .await
                .expect("wrong shutdown response")
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(shutdown.try_recv().is_err());

        let correct = Request::builder()
            .method("POST")
            .uri("/__perf/shutdown")
            .header(PERF_CONTROL_HEADER, &runtime.cookie_value)
            .body(Body::empty())
            .expect("shutdown request");
        assert_eq!(
            app.oneshot(correct)
                .await
                .expect("shutdown response")
                .status(),
            StatusCode::NO_CONTENT
        );
        shutdown.await.expect("graceful shutdown signal");
    }

    #[tokio::test]
    async fn in_memory_repositories_prove_fixture_reads_and_deny_bypass_mutation() {
        let config = PerfHarnessConfig {
            revision: "a".repeat(40),
            nonce: "b".repeat(64),
            fixture_mode: FixtureMode::ReadOnly,
            fixture: FixtureIdentity {
                version: "guild-detail-v1".to_string(),
                sha256: fixture_bytes_sha256(),
            },
            ready_file: env::temp_dir().join("unused-ready-file.json"),
        };
        let fixture = FixtureData::load(&config).expect("valid compiled fixture");
        let (sender, _receiver) = oneshot::channel();
        let runtime =
            Arc::new(PerfRuntime::new(&config, &fixture, sender).expect("performance runtime"));
        let state = build_fixture_state(&fixture, runtime.clone(), 45678).expect("fixture state");

        let deployment = state
            .persistence
            .deployment_settings
            .as_ref()
            .expect("fixture deployment repository")
            .get()
            .await
            .expect("fixture deployment read");
        assert_eq!(
            deployment.modules.get("stock"),
            Some(&DeploymentModuleSettings {
                installed: true,
                enabled: true,
            })
        );
        assert!(!deployment.commands["etf"].enabled);
        assert_eq!(
            deployment.commands["etf"].configuration,
            serde_json::json!({ "ticker_1": "DEPLOYMENT-CANARY" })
        );
        let guild = state
            .persistence
            .guild_settings
            .as_ref()
            .expect("fixture guild repository")
            .get(fixture.guild_id())
            .await
            .expect("fixture guild read")
            .expect("fixture guild exists");
        assert_eq!(guild.guild_id, fixture.guild_id());
        assert!(!guild.modules["stock"].enabled);
        assert_eq!(
            guild.modules["stock"].configuration,
            serde_json::json!({
                "default_symbol": "PERF-STOCK-CANARY",
                "etf_tickers": ["SPY"],
                "refresh_interval_seconds": 3,
                "refresh_duration_seconds": 60,
            })
        );
        assert!(guild.commands["etf"].enabled);
        assert_eq!(
            guild.commands["etf"].configuration,
            serde_json::json!({ "ticker_1": "GUILD-ETF-CANARY" })
        );
        assert_eq!(runtime.repository_reads.load(Ordering::SeqCst), 2);
        assert_eq!(runtime.repository_mutations.load(Ordering::SeqCst), 0);

        let mutation = state
            .persistence
            .deployment_settings
            .as_ref()
            .expect("fixture deployment repository")
            .upsert_module_settings("info", DeploymentModuleSettings::default())
            .await;
        assert!(mutation.is_err());
        assert_eq!(runtime.repository_mutations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn outbound_http_seam_counts_and_denies_before_send() {
        let config = PerfHarnessConfig {
            revision: "a".repeat(40),
            nonce: "b".repeat(64),
            fixture_mode: FixtureMode::ReadOnly,
            fixture: FixtureIdentity {
                version: "guild-detail-v1".to_string(),
                sha256: fixture_bytes_sha256(),
            },
            ready_file: env::temp_dir().join("unused-ready-file.json"),
        };
        let fixture = FixtureData::load(&config).expect("valid compiled fixture");
        let (sender, _receiver) = oneshot::channel();
        let runtime =
            Arc::new(PerfRuntime::new(&config, &fixture, sender).expect("performance runtime"));
        let state = build_fixture_state(&fixture, runtime.clone(), 45678).expect("fixture state");
        let request = state.http.get("https://example.invalid/must-not-send");
        let error = super::super::send_dashboard_http(&state, request)
            .await
            .expect_err("harness outbound must fail");
        assert!(error.to_string().contains("disabled"));
        assert_eq!(runtime.outbound_calls.load(Ordering::SeqCst), 1);
    }
}
