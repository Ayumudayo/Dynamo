use dynamo_domain_suggestion::{
    SuggestionRecord, SuggestionStats, SuggestionStatus, SuggestionStatusUpdate,
};
use mongodb::bson::DateTime as BsonDateTime;
use serde::{Deserialize, Serialize};

use crate::{Error, documents::parse_snowflake};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SuggestionDocument {
    pub(crate) guild_id: String,
    channel_id: String,
    pub(crate) message_id: String,
    user_id: String,
    suggestion: String,
    status: SuggestionStatus,
    stats: SuggestionStats,
    #[serde(default)]
    status_updates: Vec<SuggestionStatusUpdateDocument>,
    created_at: BsonDateTime,
    updated_at: BsonDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SuggestionStatusUpdateDocument {
    user_id: String,
    status: SuggestionStatus,
    #[serde(default)]
    reason: Option<String>,
    timestamp: BsonDateTime,
}

impl SuggestionDocument {
    pub(crate) fn from_domain(value: SuggestionRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            channel_id: value.channel_id.to_string(),
            message_id: value.message_id.to_string(),
            user_id: value.user_id.to_string(),
            suggestion: value.suggestion,
            status: value.status,
            stats: value.stats,
            status_updates: value
                .status_updates
                .into_iter()
                .map(SuggestionStatusUpdateDocument::from_domain)
                .collect(),
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
            updated_at: BsonDateTime::from_millis(value.updated_at.timestamp_millis()),
        }
    }

    pub(crate) fn into_domain(self) -> Result<SuggestionRecord, Error> {
        Ok(SuggestionRecord {
            guild_id: parse_snowflake(&self.guild_id, "suggestion guild id")?,
            channel_id: parse_snowflake(&self.channel_id, "suggestion channel id")?,
            message_id: parse_snowflake(&self.message_id, "suggestion message id")?,
            user_id: parse_snowflake(&self.user_id, "suggestion user id")?,
            suggestion: self.suggestion,
            status: self.status,
            stats: self.stats,
            status_updates: self
                .status_updates
                .into_iter()
                .map(SuggestionStatusUpdateDocument::into_domain)
                .collect::<Result<Vec<_>, _>>()?,
            created_at: self.created_at.to_system_time().into(),
            updated_at: self.updated_at.to_system_time().into(),
        })
    }
}

impl SuggestionStatusUpdateDocument {
    fn from_domain(value: SuggestionStatusUpdate) -> Self {
        Self {
            user_id: value.user_id.to_string(),
            status: value.status,
            reason: value.reason,
            timestamp: BsonDateTime::from_millis(value.timestamp.timestamp_millis()),
        }
    }

    fn into_domain(self) -> Result<SuggestionStatusUpdate, Error> {
        Ok(SuggestionStatusUpdate {
            user_id: parse_snowflake(&self.user_id, "suggestion status update user id")?,
            status: self.status,
            reason: self.reason,
            timestamp: self.timestamp.to_system_time().into(),
        })
    }
}
