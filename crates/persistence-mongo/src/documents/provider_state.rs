use mongodb::bson::{Bson, DateTime as BsonDateTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProviderStateDocument {
    #[serde(rename = "_id")]
    pub(crate) id: String,
    pub(crate) state: Bson,
    #[serde(default)]
    pub(crate) updated_at: Option<BsonDateTime>,
}
