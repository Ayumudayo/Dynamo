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

    async fn save(&self, record: SuggestionRecord) -> Result<SuggestionRecord, Error> {
        let document = SuggestionDocument::from_domain(record);
        self.suggestions
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
}
