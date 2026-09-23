use async_trait::async_trait;
use futures_util::TryStreamExt;
use mongodb::bson::doc;

use crate::{Error, MongoPersistence, documents::WarningLogDocument};
use dynamo_domain_moderation::WarningLogRecord;
use dynamo_repositories::WarningLogRepository;

#[async_trait]
impl WarningLogRepository for MongoPersistence {
    async fn add(&self, record: WarningLogRecord) -> Result<WarningLogRecord, Error> {
        let document = WarningLogDocument::from_domain(record);
        self.warning_logs.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn list_for_member(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<Vec<WarningLogRecord>, Error> {
        let mut cursor = self
            .warning_logs
            .find(doc! {
                "guild_id": guild_id.to_string(),
                "member_id": member_id.to_string(),
            })
            .await?;

        let mut records = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            records.push(document.into_domain()?);
        }
        Ok(records)
    }

    async fn clear_for_member(&self, guild_id: u64, member_id: u64) -> Result<u64, Error> {
        let deleted = self
            .warning_logs
            .delete_many(doc! {
                "guild_id": guild_id.to_string(),
                "member_id": member_id.to_string(),
            })
            .await?;
        Ok(deleted.deleted_count)
    }
}
