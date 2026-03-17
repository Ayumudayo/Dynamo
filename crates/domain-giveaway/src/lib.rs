use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GiveawayStatus {
    #[default]
    Active,
    Paused,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GiveawayRecord {
    pub guild_id: u64,
    pub channel_id: u64,
    pub message_id: u64,
    pub prize: String,
    pub winner_count: u64,
    pub host_user_id: u64,
    pub allowed_role_ids: Vec<u64>,
    pub entries: Vec<u64>,
    pub winner_ids: Vec<u64>,
    pub status: GiveawayStatus,
    pub started_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub paused_at: Option<DateTime<Utc>>,
    pub button_label: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Default for GiveawayRecord {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            guild_id: 0,
            channel_id: 0,
            message_id: 0,
            prize: String::new(),
            winner_count: 1,
            host_user_id: 0,
            allowed_role_ids: Vec::new(),
            entries: Vec::new(),
            winner_ids: Vec::new(),
            status: GiveawayStatus::Active,
            started_at: now,
            ends_at: now,
            paused_at: None,
            button_label: "Enter Giveaway".to_string(),
            created_at: now,
            updated_at: now,
        }
    }
}
