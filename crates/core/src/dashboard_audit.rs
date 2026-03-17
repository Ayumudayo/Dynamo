use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardAuditScope {
    Deployment,
    Guild,
}

impl DashboardAuditScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deployment => "deployment",
            Self::Guild => "guild",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardAuditEntityType {
    Module,
    Command,
}

impl DashboardAuditEntityType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Command => "command",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardAuditAction {
    Toggle,
    SaveSettings,
}

impl DashboardAuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::SaveSettings => "save_settings",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DashboardAuditLogEntry {
    pub id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub actor_user_id: u64,
    pub actor_username: String,
    pub scope: DashboardAuditScope,
    pub guild_id: Option<u64>,
    pub entity_type: DashboardAuditEntityType,
    pub entity_id: String,
    pub action: DashboardAuditAction,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DashboardAuditLogQuery {
    pub scope: DashboardAuditScope,
    pub guild_id: Option<u64>,
    pub entity_type: Option<DashboardAuditEntityType>,
    pub action: Option<DashboardAuditAction>,
    pub page: u64,
    pub page_size: u64,
}

impl DashboardAuditLogQuery {
    pub fn deployment() -> Self {
        Self {
            scope: DashboardAuditScope::Deployment,
            guild_id: None,
            entity_type: None,
            action: None,
            page: 1,
            page_size: 20,
        }
    }

    pub fn guild(guild_id: u64) -> Self {
        Self {
            scope: DashboardAuditScope::Guild,
            guild_id: Some(guild_id),
            entity_type: None,
            action: None,
            page: 1,
            page_size: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct DashboardAuditLogPage {
    pub entries: Vec<DashboardAuditLogEntry>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
}

impl DashboardAuditLogPage {
    pub fn empty(page: u64, page_size: u64) -> Self {
        Self {
            entries: Vec::new(),
            page,
            page_size,
            total: 0,
        }
    }

    pub fn has_prev(&self) -> bool {
        self.page > 1
    }

    pub fn has_next(&self) -> bool {
        self.page.saturating_mul(self.page_size) < self.total
    }
}
