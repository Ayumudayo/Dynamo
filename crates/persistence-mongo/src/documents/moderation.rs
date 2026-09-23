use dynamo_domain_moderation::WarningLogRecord;
use mongodb::bson::DateTime as BsonDateTime;
use serde::{Deserialize, Serialize};

use crate::{Error, documents::parse_snowflake};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WarningLogDocument {
    pub(crate) guild_id: String,
    pub(crate) member_id: String,
    reason: Option<String>,
    admin_id: String,
    admin_tag: String,
    created_at: BsonDateTime,
}

impl WarningLogDocument {
    pub(crate) fn from_domain(value: WarningLogRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            member_id: value.member_id.to_string(),
            reason: value.reason,
            admin_id: value.admin_id.to_string(),
            admin_tag: value.admin_tag,
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
        }
    }

    pub(crate) fn into_domain(self) -> Result<WarningLogRecord, Error> {
        Ok(WarningLogRecord {
            guild_id: parse_snowflake(&self.guild_id, "warning log guild id")?,
            member_id: parse_snowflake(&self.member_id, "warning log member id")?,
            reason: self.reason,
            admin_id: parse_snowflake(&self.admin_id, "warning log admin id")?,
            admin_tag: self.admin_tag,
            created_at: self.created_at.to_system_time().into(),
        })
    }
}
