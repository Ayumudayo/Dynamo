use dynamo_domain_stats::{
    CommandUsageStats, MemberStatsRecord, MessageContextUsageStats, VoiceStatsRecord,
};
use mongodb::bson::DateTime as BsonDateTime;
use serde::{Deserialize, Serialize};

use crate::{Error, documents::parse_snowflake};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MemberStatsDocument {
    #[serde(rename = "_id", default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<mongodb::bson::Bson>,
    #[serde(default)]
    pub(crate) revision: u64,
    pub(crate) guild_id: String,
    pub(crate) member_id: String,
    messages: u64,
    voice: VoiceStatsRecord,
    commands: CommandUsageStats,
    contexts: MessageContextUsageStats,
    xp: u64,
    level: u32,
    created_at: BsonDateTime,
    updated_at: BsonDateTime,
}

impl MemberStatsDocument {
    pub(crate) fn from_domain(value: MemberStatsRecord) -> Self {
        Self {
            id: None,
            revision: 0,
            guild_id: value.guild_id.to_string(),
            member_id: value.member_id.to_string(),
            messages: value.messages,
            voice: value.voice,
            commands: value.commands,
            contexts: value.contexts,
            xp: value.xp,
            level: value.level,
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
            updated_at: BsonDateTime::from_millis(value.updated_at.timestamp_millis()),
        }
    }

    pub(crate) fn into_domain(self) -> Result<MemberStatsRecord, Error> {
        Ok(MemberStatsRecord {
            guild_id: parse_snowflake(&self.guild_id, "member stats guild id")?,
            member_id: parse_snowflake(&self.member_id, "member stats member id")?,
            messages: self.messages,
            voice: self.voice,
            commands: self.commands,
            contexts: self.contexts,
            xp: self.xp,
            level: self.level,
            created_at: self.created_at.to_system_time().into(),
            updated_at: self.updated_at.to_system_time().into(),
        })
    }
}
