use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const ANNOUNCEMENT_QUEUE_PROVIDER_ID: &str = "owner_announcement_queue";
pub const MAX_ANNOUNCEMENT_HISTORY: usize = 20;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementTone {
    #[default]
    Info,
    Warning,
    Critical,
}

impl AnnouncementTone {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }

    pub fn from_str_opt(value: &str) -> Option<Self> {
        match value.trim() {
            "info" => Some(Self::Info),
            "warning" => Some(Self::Warning),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    pub fn embed_color(self) -> u32 {
        match self {
            Self::Info => 0x5865F2,
            Self::Warning => 0xFEE75C,
            Self::Critical => 0xED4245,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementStatus {
    #[default]
    Queued,
    Delivered,
    PartialFailure,
    NoSubscribers,
}

impl AnnouncementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Delivered => "delivered",
            Self::PartialFailure => "partial_failure",
            Self::NoSubscribers => "no_subscribers",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OwnerAnnouncement {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub actor_user_id: u64,
    pub actor_username: String,
    pub tone: AnnouncementTone,
    pub title: String,
    pub body: String,
    pub status: AnnouncementStatus,
    pub targeted_guilds: usize,
    pub delivered_guilds: usize,
    pub failed_guilds: usize,
    pub last_error: Option<String>,
}

impl Default for OwnerAnnouncement {
    fn default() -> Self {
        Self {
            id: String::new(),
            created_at: Utc::now(),
            processed_at: None,
            actor_user_id: 0,
            actor_username: String::new(),
            tone: AnnouncementTone::Info,
            title: String::new(),
            body: String::new(),
            status: AnnouncementStatus::Queued,
            targeted_guilds: 0,
            delivered_guilds: 0,
            failed_guilds: 0,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct OwnerAnnouncementStore {
    pub entries: Vec<OwnerAnnouncement>,
}

impl OwnerAnnouncementStore {
    pub fn enqueue(&mut self, item: OwnerAnnouncement) {
        self.entries.insert(0, item);
        self.entries.truncate(MAX_ANNOUNCEMENT_HISTORY);
    }
}
