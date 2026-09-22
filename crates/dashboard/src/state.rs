use std::{collections::HashMap, env, sync::Arc, time::Duration};

use dynamo_module_kit::{CommandCatalog, ModuleCatalog};
use dynamo_persistence_api::Persistence;
use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::RwLock;

pub(crate) const SESSION_COOKIE_NAME: &str = "dynamo_dashboard_session";
pub(crate) const SESSION_TTL_HOURS: i64 = 24 * 14;
pub(crate) const OAUTH_STATE_TTL_MINUTES: i64 = 15;
pub(crate) const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DEFAULT_INVITE_PERMISSIONS: u64 = 2_146_958_847;
pub(crate) const DASHBOARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const DASHBOARD_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub(crate) struct DashboardConfig {
    pub(crate) host: std::net::IpAddr,
    pub(crate) port: u16,
    pub(crate) public_base_url: String,
    pub(crate) bot_token: String,
    pub(crate) client_secret: String,
    pub(crate) invite_permissions: u64,
    pub(crate) admin_user_ids: Vec<u64>,
    pub(crate) register_globally: bool,
    pub(crate) command_sync_interval_seconds: u64,
}

impl DashboardConfig {
    pub(crate) fn from_env() -> anyhow::Result<Self> {
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

        let public_base_url = env::var("DASHBOARD_BASE_URL")
            .unwrap_or_else(|_| format!("http://{host}:{port}"))
            .trim_end_matches('/')
            .to_string();

        let bot_token = env::var("DISCORD_TOKEN")
            .or_else(|_| env::var("BOT_TOKEN"))
            .map_err(|_| anyhow::anyhow!("DISCORD_TOKEN or BOT_TOKEN must be set"))?;

        let client_secret = env::var("DISCORD_CLIENT_SECRET")
            .or_else(|_| env::var("BOT_SECRET"))
            .map_err(|_| {
                anyhow::anyhow!(
                    "DISCORD_CLIENT_SECRET or BOT_SECRET must be set for dashboard OAuth"
                )
            })?;

        let invite_permissions = env::var("DISCORD_BOT_INVITE_PERMISSIONS")
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|error| {
                anyhow::anyhow!("DISCORD_BOT_INVITE_PERMISSIONS must be a valid u64: {error}")
            })?
            .unwrap_or(DEFAULT_INVITE_PERMISSIONS);

        let admin_user_ids = parse_u64_list_env("DASHBOARD_ADMIN_USER_IDS")?;
        let dev_guild_id = env::var("DISCORD_DEV_GUILD_ID")
            .or_else(|_| env::var("GUILD_ID"))
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|error| {
                anyhow::anyhow!("DISCORD_DEV_GUILD_ID or GUILD_ID must be a valid u64: {error}")
            })?;
        let register_globally = match env::var("DISCORD_REGISTER_GLOBALLY") {
            Ok(value) => parse_bool_value("DISCORD_REGISTER_GLOBALLY", &value)?,
            Err(env::VarError::NotPresent) => dev_guild_id.is_none(),
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "DISCORD_REGISTER_GLOBALLY could not be read: {error}"
                ));
            }
        };
        let command_sync_interval_seconds =
            parse_u64_env("DISCORD_COMMAND_SYNC_INTERVAL_SECONDS", 15)?;

        Ok(Self {
            host,
            port,
            public_base_url,
            bot_token,
            client_secret,
            invite_permissions,
            admin_user_ids,
            register_globally,
            command_sync_interval_seconds,
        })
    }
}

#[derive(Clone)]
pub(crate) struct DashboardState {
    pub(crate) config: DashboardConfig,
    pub(crate) http: reqwest::Client,
    pub(crate) discord_api_base: String,
    pub(crate) app_info: DiscordApplicationInfo,
    pub(crate) module_catalog: ModuleCatalog,
    pub(crate) command_catalog: CommandCatalog,
    pub(crate) persistence: Persistence,
    pub(crate) sessions: Arc<RwLock<HashMap<String, DashboardSession>>>,
    pub(crate) oauth_states: Arc<RwLock<HashMap<String, PendingOauthState>>>,
    #[cfg(feature = "perf-harness")]
    pub(crate) perf_runtime: Option<Arc<crate::DashboardPerfRuntime>>,
}

#[derive(Debug, Clone)]
pub(crate) struct DiscordApplicationInfo {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) icon: Option<String>,
    pub(crate) owner_user_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct DashboardSession {
    pub(crate) user: DashboardUser,
    pub(crate) guilds: Vec<DashboardGuild>,
    pub(crate) access_token: String,
    pub(crate) expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingOauthState {
    pub(crate) redirect_to: String,
    pub(crate) created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DashboardUser {
    pub(crate) id: u64,
    pub(crate) username: String,
    pub(crate) global_name: Option<String>,
    pub(crate) avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DashboardGuild {
    #[serde(deserialize_with = "deserialize_u64_from_discord_id")]
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) icon: Option<String>,
    #[serde(default, alias = "permissions_new")]
    pub(crate) permissions: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(crate) struct DashboardPageQuery {
    pub(crate) tab: Option<String>,
    pub(crate) log_entity: Option<String>,
    pub(crate) log_action: Option<String>,
    pub(crate) log_page: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LoginQuery {
    pub(crate) redirect: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordCallbackQuery {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordApplicationResponse {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) icon: Option<String>,
    pub(crate) owner: Option<DiscordOwner>,
    pub(crate) team: Option<DiscordTeam>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordOwner {
    pub(crate) id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordTeam {
    pub(crate) owner_user_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordTokenResponse {
    pub(crate) access_token: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscordOAuthUser {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) global_name: Option<String>,
    pub(crate) avatar: Option<String>,
}

fn parse_u64_list_env(key: &str) -> Result<Vec<u64>, anyhow::Error> {
    let Some(raw) = env::var(key).ok() else {
        return Ok(Vec::new());
    };

    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| anyhow::anyhow!("{key} must contain valid u64 values: {error}"))
        })
        .collect()
}

fn parse_bool_value(key: &str, value: &str) -> Result<bool, anyhow::Error> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("{key} must be one of true/false/1/0/yes/no/on/off"),
    }
}

fn parse_u64_env(key: &str, default: u64) -> Result<u64, anyhow::Error> {
    match env::var(key) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|error| anyhow::anyhow!("{key} must be a valid u64: {error}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(anyhow::anyhow!("{key} could not be read: {error}")),
    }
}

fn deserialize_u64_from_discord_id<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DiscordId {
        String(String),
        Number(u64),
    }

    match DiscordId::deserialize(deserializer)? {
        DiscordId::String(value) => value.parse::<u64>().map_err(serde::de::Error::custom),
        DiscordId::Number(value) => Ok(value),
    }
}

#[cfg(test)]
mod tests {
    use super::DashboardGuild;

    #[test]
    fn discord_guild_id_deserializes_from_string() {
        let guild: DashboardGuild = serde_json::from_value(serde_json::json!({
            "id": "110340875107733504",
            "name": "Test Guild",
            "icon": null,
            "permissions": "32"
        }))
        .expect("dashboard guild");

        assert_eq!(guild.id, 110340875107733504);
    }
}
