use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VoiceStatsRecord {
    pub connections: u64,
    pub time_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CommandUsageStats {
    pub prefix: u64,
    pub slash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MessageContextUsageStats {
    pub message: u64,
    pub user: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberStatsRecord {
    pub guild_id: u64,
    pub member_id: u64,
    pub messages: u64,
    pub voice: VoiceStatsRecord,
    pub commands: CommandUsageStats,
    pub contexts: MessageContextUsageStats,
    pub xp: u64,
    pub level: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
