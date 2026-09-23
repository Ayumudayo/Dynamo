use async_trait::async_trait;
use mongodb::bson::doc;

use crate::{Error, MongoPersistence, documents::MemberStatsDocument};
use dynamo_domain_stats::MemberStatsRecord;
use dynamo_repositories::MemberStatsRepository;

#[async_trait]
impl MemberStatsRepository for MongoPersistence {
    async fn get_or_create(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<MemberStatsRecord, Error> {
        let document = self
            .member_stats
            .find_one(doc! {
                "guild_id": guild_id.to_string(),
                "member_id": member_id.to_string(),
            })
            .await?;

        if let Some(document) = document {
            return document.into_domain();
        }

        let now = chrono::Utc::now();
        let record = MemberStatsRecord {
            guild_id,
            member_id,
            messages: 0,
            voice: Default::default(),
            commands: Default::default(),
            contexts: Default::default(),
            xp: 0,
            level: 1,
            created_at: now,
            updated_at: now,
        };
        let document = MemberStatsDocument::from_domain(record);
        self.member_stats.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn save(&self, record: MemberStatsRecord) -> Result<MemberStatsRecord, Error> {
        let document = MemberStatsDocument::from_domain(record);
        self.member_stats
            .replace_one(
                doc! {
                    "guild_id": &document.guild_id,
                    "member_id": &document.member_id,
                },
                document.clone(),
            )
            .upsert(true)
            .await?;
        document.into_domain()
    }
}
