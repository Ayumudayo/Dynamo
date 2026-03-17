use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InviteCounters {
    pub inviter: Option<String>,
    pub code: Option<String>,
    pub tracked: u64,
    pub fake: u64,
    pub left: u64,
    pub added: u64,
}

impl InviteCounters {
    pub fn effective(&self) -> i64 {
        self.tracked as i64 + self.added as i64 - self.fake as i64 - self.left as i64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteMemberRecord {
    pub guild_id: u64,
    pub member_id: String,
    pub invite_data: InviteCounters,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteLeaderboardEntry {
    pub member_id: String,
    pub invites: i64,
}
