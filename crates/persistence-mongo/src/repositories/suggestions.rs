use async_trait::async_trait;
use mongodb::bson::doc;

use crate::{Error, MongoPersistence, documents::SuggestionDocument};
use dynamo_domain_suggestion::SuggestionRecord;
use dynamo_repositories::SuggestionsRepository;

#[async_trait]
impl SuggestionsRepository for MongoPersistence {
    async fn create(&self, record: SuggestionRecord) -> Result<SuggestionRecord, Error> {
        let document = SuggestionDocument::from_domain(record);
        self.suggestions.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn get_by_message(
        &self,
        guild_id: u64,
        message_id: u64,
    ) -> Result<Option<SuggestionRecord>, Error> {
        let document = self
            .suggestions
            .find_one(doc! {
                "guild_id": guild_id.to_string(),
                "message_id": message_id.to_string(),
            })
            .await?;

        document.map(SuggestionDocument::into_domain).transpose()
    }

    async fn replace_if_unchanged(
        &self,
        expected: &SuggestionRecord,
        record: SuggestionRecord,
    ) -> Result<Option<SuggestionRecord>, Error> {
        let document = SuggestionDocument::from_domain(record);
        let result = self.suggestions
            .replace_one(
                doc! {
                    "guild_id": expected.guild_id.to_string(),
                    "message_id": expected.message_id.to_string(),
                    "status": mongodb::bson::to_bson(&expected.status)?,
                    "updated_at": mongodb::bson::DateTime::from_millis(expected.updated_at.timestamp_millis()),
                    "$expr": { "$eq": [
                        { "$size": { "$ifNull": ["$status_updates", []] } },
                        expected.status_updates.len() as i64,
                    ] },
                },
                document.clone(),
            )
            .await?;

        if result.matched_count == 0 {
            return Ok(None);
        }
        document.into_domain().map(Some)
    }
}
