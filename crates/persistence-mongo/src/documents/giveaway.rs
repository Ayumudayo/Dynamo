use dynamo_domain_giveaway::{GiveawayRecord, GiveawayStatus};
use mongodb::bson::DateTime as BsonDateTime;
use serde::{Deserialize, Serialize};

use crate::{Error, documents::parse_snowflake};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GiveawayDocument {
    pub(crate) guild_id: String,
    channel_id: String,
    pub(crate) message_id: String,
    prize: String,
    winner_count: u64,
    host_user_id: String,
    #[serde(default)]
    allowed_role_ids: Vec<String>,
    #[serde(default)]
    entries: Vec<String>,
    #[serde(default)]
    winner_ids: Vec<String>,
    status: GiveawayStatus,
    started_at: BsonDateTime,
    ends_at: BsonDateTime,
    #[serde(default)]
    paused_at: Option<BsonDateTime>,
    button_label: String,
    created_at: BsonDateTime,
    updated_at: BsonDateTime,
}

impl GiveawayDocument {
    pub(crate) fn from_domain(value: GiveawayRecord) -> Self {
        Self {
            guild_id: value.guild_id.to_string(),
            channel_id: value.channel_id.to_string(),
            message_id: value.message_id.to_string(),
            prize: value.prize,
            winner_count: value.winner_count,
            host_user_id: value.host_user_id.to_string(),
            allowed_role_ids: value
                .allowed_role_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            entries: value.entries.into_iter().map(|id| id.to_string()).collect(),
            winner_ids: value
                .winner_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            status: value.status,
            started_at: BsonDateTime::from_millis(value.started_at.timestamp_millis()),
            ends_at: BsonDateTime::from_millis(value.ends_at.timestamp_millis()),
            paused_at: value
                .paused_at
                .map(|timestamp| BsonDateTime::from_millis(timestamp.timestamp_millis())),
            button_label: value.button_label,
            created_at: BsonDateTime::from_millis(value.created_at.timestamp_millis()),
            updated_at: BsonDateTime::from_millis(value.updated_at.timestamp_millis()),
        }
    }

    pub(crate) fn into_domain(self) -> Result<GiveawayRecord, Error> {
        Ok(GiveawayRecord {
            guild_id: parse_snowflake(&self.guild_id, "giveaway guild id")?,
            channel_id: parse_snowflake(&self.channel_id, "giveaway channel id")?,
            message_id: parse_snowflake(&self.message_id, "giveaway message id")?,
            prize: self.prize,
            winner_count: self.winner_count,
            host_user_id: parse_snowflake(&self.host_user_id, "giveaway host user id")?,
            allowed_role_ids: self
                .allowed_role_ids
                .into_iter()
                .map(|value| parse_snowflake(&value, "giveaway allowed role id"))
                .collect::<Result<Vec<_>, _>>()?,
            entries: self
                .entries
                .into_iter()
                .map(|value| parse_snowflake(&value, "giveaway entry user id"))
                .collect::<Result<Vec<_>, _>>()?,
            winner_ids: self
                .winner_ids
                .into_iter()
                .map(|value| parse_snowflake(&value, "giveaway winner user id"))
                .collect::<Result<Vec<_>, _>>()?,
            status: self.status,
            started_at: self.started_at.to_system_time().into(),
            ends_at: self.ends_at.to_system_time().into(),
            paused_at: self
                .paused_at
                .map(|timestamp| timestamp.to_system_time().into()),
            button_label: self.button_label,
            created_at: self.created_at.to_system_time().into(),
            updated_at: self.updated_at.to_system_time().into(),
        })
    }
}
