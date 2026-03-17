use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarningLogRecord {
    pub guild_id: u64,
    pub member_id: u64,
    pub reason: Option<String>,
    pub admin_id: u64,
    pub admin_tag: String,
    pub created_at: DateTime<Utc>,
}
