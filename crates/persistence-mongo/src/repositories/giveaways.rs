use async_trait::async_trait;
use futures_util::TryStreamExt;
use mongodb::bson::{DateTime as BsonDateTime, doc};

use crate::{Error, MongoPersistence, documents::GiveawayDocument};
use dynamo_domain_giveaway::GiveawayRecord;
use dynamo_repositories::GiveawaysRepository;

#[async_trait]
impl GiveawaysRepository for MongoPersistence {
    async fn create(&self, record: GiveawayRecord) -> Result<GiveawayRecord, Error> {
        let document = GiveawayDocument::from_domain(record);
        self.giveaways.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn get_by_message(
        &self,
        guild_id: u64,
        message_id: u64,
    ) -> Result<Option<GiveawayRecord>, Error> {
        let document = self
            .giveaways
            .find_one(doc! {
                "guild_id": guild_id.to_string(),
                "message_id": message_id.to_string(),
            })
            .await?;

        document.map(GiveawayDocument::into_domain).transpose()
    }

    async fn save(&self, record: GiveawayRecord) -> Result<GiveawayRecord, Error> {
        let document = GiveawayDocument::from_domain(record);
        self.giveaways
            .replace_one(
                doc! {
                    "guild_id": &document.guild_id,
                    "message_id": &document.message_id,
                },
                document.clone(),
            )
            .upsert(true)
            .await?;

        document.into_domain()
    }

    async fn list_by_guild(&self, guild_id: u64) -> Result<Vec<GiveawayRecord>, Error> {
        let mut cursor = self
            .giveaways
            .find(doc! { "guild_id": guild_id.to_string() })
            .await?;

        let mut records = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            records.push(document.into_domain()?);
        }
        Ok(records)
    }

    async fn list_due_before(
        &self,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<GiveawayRecord>, Error> {
        let mut cursor = self
            .giveaways
            .find(doc! {
                "status": "ACTIVE",
                "ends_at": { "$lte": BsonDateTime::from_millis(timestamp.timestamp_millis()) },
            })
            .await?;

        let mut records = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            records.push(document.into_domain()?);
        }
        Ok(records)
    }
}
