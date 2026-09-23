use dynamo_domain_invite::{InviteCounters, InviteMemberRecord};
use mongodb::bson::DateTime as BsonDateTime;
use serde::{Deserialize, Serialize};

use crate::{Error, documents::parse_snowflake};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InviteMemberDocument {
    pub(crate) guild_id: String,
    pub(crate) member_id: String,
    #[serde(default)]
    invite_data: InviteCounters,
    created_at: BsonDateTime,
    updated_at: BsonDateTime,
}

impl InviteMemberDocument {
    pub(crate) fn from_domain(value: InviteMemberRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            member_id: value.member_id,
            invite_data: value.invite_data,
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
            updated_at: BsonDateTime::from_millis(value.updated_at.timestamp_millis()),
        }
    }

    pub(crate) fn into_domain(self) -> Result<InviteMemberRecord, Error> {
        Ok(InviteMemberRecord {
            guild_id: parse_snowflake(&self.guild_id, "invite member guild id")?,
            member_id: self.member_id,
            invite_data: self.invite_data,
            created_at: self.created_at.to_system_time().into(),
            updated_at: self.updated_at.to_system_time().into(),
        })
    }
}
