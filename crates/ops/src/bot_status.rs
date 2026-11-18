use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const BOT_HEARTBEAT_PROVIDER_ID: &str = "bot_runtime_heartbeat";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BotHeartbeatSnapshot {
    pub bot_username: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub connected_guilds: usize,
    pub loaded_modules: usize,
    pub loaded_commands: usize,
    pub register_globally: bool,
    pub tossinvest_wired: bool,
    pub exchange_refresh_active: bool,
    pub last_exchange_refresh_at: Option<DateTime<Utc>>,
    pub last_exchange_error: Option<String>,
    pub google_translate_configured: bool,
}

impl Default for BotHeartbeatSnapshot {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            bot_username: None,
            started_at: now,
            updated_at: now,
            connected_guilds: 0,
            loaded_modules: 0,
            loaded_commands: 0,
            register_globally: false,
            tossinvest_wired: false,
            exchange_refresh_active: false,
            last_exchange_refresh_at: None,
            last_exchange_error: None,
            google_translate_configured: false,
        }
    }
}
