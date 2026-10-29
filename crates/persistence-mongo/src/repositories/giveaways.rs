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

    async fn compare_and_save(
        &self,
        record: GiveawayRecord,
    ) -> Result<Option<GiveawayRecord>, Error> {
        let mut document = GiveawayDocument::from_domain(record);
        let expected_revision = document.revision;
        document.revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("giveaway revision overflow"))?;
        let mut filter = doc! {
            "guild_id": &document.guild_id,
            "message_id": &document.message_id,
        };
        if expected_revision == 0 {
            filter.insert(
                "$or",
                mongodb::bson::bson!([
                    { "revision": 0 }, { "revision": { "$exists": false } }
                ]),
            );
        } else {
            filter.insert("revision", expected_revision);
        }
        let result = self.giveaways.replace_one(filter, document.clone()).await?;
        if result.matched_count == 0 {
            return Ok(None);
        }
        Ok(Some(document.into_domain()?))
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
                "$or": [
                    { "status": "ACTIVE", "ends_at": { "$lte": BsonDateTime::from_millis(timestamp.timestamp_millis()) } },
                    { "message_sync_pending": true },
                    { "announcement_pending": true },
                ],
            })
            .await?;

        let mut records = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            records.push(document.into_domain()?);
        }
        Ok(records)
    }
}
