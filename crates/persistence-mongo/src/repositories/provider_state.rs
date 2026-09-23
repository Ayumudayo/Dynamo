use async_trait::async_trait;
use mongodb::bson::{DateTime as BsonDateTime, doc, from_bson, to_bson};

use crate::{Error, MongoPersistence};
use dynamo_repositories::ProviderStateRepository;

impl MongoPersistence {
    pub async fn load_provider_state(
        &self,
        provider_id: &str,
    ) -> Result<Option<serde_json::Value>, Error> {
        let document = self
            .provider_state
            .find_one(doc! { "_id": provider_id })
            .await?;
        let Some(document) = document else {
            return Ok(None);
        };

        Ok(Some(from_bson(document.state)?))
    }

    pub async fn save_provider_state(
        &self,
        provider_id: &str,
        state: serde_json::Value,
    ) -> Result<(), Error> {
        self.provider_state
            .update_one(
                doc! { "_id": provider_id },
                doc! {
                    "$setOnInsert": { "_id": provider_id },
                    "$set": {
                        "state": to_bson(&state)?,
                        "updated_at": BsonDateTime::now(),
                    },
                },
            )
            .upsert(true)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl ProviderStateRepository for MongoPersistence {
    async fn load_json(&self, provider_id: &str) -> Result<Option<serde_json::Value>, Error> {
        self.load_provider_state(provider_id).await
    }

    async fn save_json(&self, provider_id: &str, value: serde_json::Value) -> Result<(), Error> {
        self.save_provider_state(provider_id, value).await
    }
}
