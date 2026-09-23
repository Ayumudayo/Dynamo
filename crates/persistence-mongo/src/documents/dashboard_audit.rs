use dynamo_ops::{
    DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry, DashboardAuditScope,
};
use mongodb::bson::{DateTime as BsonDateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};

use crate::{Error, documents::parse_snowflake};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DashboardAuditLogDocument {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<ObjectId>,
    timestamp: BsonDateTime,
    actor_user_id: String,
    actor_username: String,
    scope: DashboardAuditScope,
    #[serde(default)]
    guild_id: Option<String>,
    entity_type: DashboardAuditEntityType,
    entity_id: String,
    action: DashboardAuditAction,
    summary: String,
}

impl DashboardAuditLogDocument {
    pub(crate) fn from_domain(value: DashboardAuditLogEntry) -> Self {
        Self {
            id: value.id.and_then(|value| ObjectId::parse_str(&value).ok()),
            timestamp: BsonDateTime::from_millis(value.timestamp.timestamp_millis()),
            actor_user_id: value.actor_user_id.to_string(),
            actor_username: value.actor_username,
            scope: value.scope,
            guild_id: value.guild_id.map(|value| value.to_string()),
            entity_type: value.entity_type,
            entity_id: value.entity_id,
            action: value.action,
            summary: value.summary,
        }
    }

    pub(crate) fn into_domain(self) -> Result<DashboardAuditLogEntry, Error> {
        Ok(DashboardAuditLogEntry {
            id: self.id.map(|value| value.to_hex()),
            timestamp: self.timestamp.to_system_time().into(),
            actor_user_id: parse_snowflake(&self.actor_user_id, "dashboard audit actor user id")?,
            actor_username: self.actor_username,
            scope: self.scope,
            guild_id: self
                .guild_id
                .map(|value| parse_snowflake(&value, "dashboard audit guild id"))
                .transpose()?,
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            action: self.action,
            summary: self.summary,
        })
    }
}
