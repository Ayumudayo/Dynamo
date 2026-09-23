use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, ensure};
use serde::Deserialize;

use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, GuildCommandSettings, GuildModuleSettings,
};

use super::config::{FIXTURE_BYTES, PerfHarnessConfig};

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureData {
    schema_version: u32,
    fixture_version: String,
    pub(super) application: FixtureApplication,
    pub(super) session: FixtureSession,
    pub(super) target: FixtureTarget,
    pub(super) settings: FixtureSettings,
    pub(super) fake_provider_responses: FixtureProviderResponses,
    pub(super) route_payloads: FixtureRoutePayloads,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureApplication {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) icon: Option<String>,
    pub(super) owner_user_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureSession {
    pub(super) id: String,
    pub(super) expires_at: String,
    pub(super) user: FixtureUser,
    pub(super) guilds: Vec<FixtureGuild>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureUser {
    pub(super) id: String,
    pub(super) username: String,
    pub(super) global_name: String,
    pub(super) avatar: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureGuild {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) icon: Option<String>,
    pub(super) permissions: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureTarget {
    pub(super) guild_id: String,
    pub(super) bot_present: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureSettings {
    pub(super) deployment: FixtureDeploymentSettings,
    pub(super) guild: FixtureGuildSettings,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureDeploymentSettings {
    pub(super) modules: BTreeMap<String, DeploymentModuleSettings>,
    pub(super) commands: BTreeMap<String, DeploymentCommandSettings>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureGuildSettings {
    pub(super) guild_id: String,
    pub(super) modules: BTreeMap<String, GuildModuleSettings>,
    pub(super) commands: BTreeMap<String, GuildCommandSettings>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureProviderResponses {
    pub(super) bot_present: bool,
    pub(super) guild_lookup_delay_ms: u64,
    pub(super) outbound_http_attempts: u64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureRoutePayloads {
    pub(super) public_padding_bytes: usize,
    pub(super) guild_detail_padding_bytes: usize,
}

impl FixtureData {
    pub(super) fn load(config: &PerfHarnessConfig) -> anyhow::Result<Self> {
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
        let mut guild_ids = HashSet::with_capacity(100);
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
                configuration: serde_json::json!({ "default_symbol": "PERF-STOCK-CANARY", "etf_tickers": ["SPY"], "refresh_interval_seconds": 3, "refresh_duration_seconds": 60 }),
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
        ensure!(
            fixture.route_payloads.public_padding_bytes == 0
                && fixture.route_payloads.guild_detail_padding_bytes == 4096,
            "fixture route payload provenance metadata is invalid"
        );
        Ok(fixture)
    }

    pub(super) fn user_id(&self) -> u64 {
        self.session
            .user
            .id
            .parse()
            .expect("validated fixture user id")
    }
    pub(super) fn guild_id(&self) -> u64 {
        self.target
            .guild_id
            .parse()
            .expect("validated fixture target guild id")
    }
    pub(super) fn owner_user_id(&self) -> u64 {
        self.application
            .owner_user_id
            .parse()
            .expect("validated fixture owner user id")
    }
    pub(super) fn session_expires_at(&self) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
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
