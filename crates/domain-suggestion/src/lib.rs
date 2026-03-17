use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SuggestionStatus {
    #[default]
    Pending,
    Approved,
    Rejected,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SuggestionStats {
    pub upvotes: u64,
    pub downvotes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionStatusUpdate {
    pub user_id: u64,
    pub status: SuggestionStatus,
    pub reason: Option<String>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionRecord {
    pub guild_id: u64,
    pub channel_id: u64,
    pub message_id: u64,
    pub user_id: u64,
    pub suggestion: String,
    pub status: SuggestionStatus,
    pub stats: SuggestionStats,
    #[serde(default)]
    pub status_updates: Vec<SuggestionStatusUpdate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
