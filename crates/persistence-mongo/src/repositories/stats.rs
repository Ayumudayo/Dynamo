use async_trait::async_trait;
use futures_util::TryStreamExt;
use mongodb::bson::{doc, to_document};

use crate::{Error, MongoPersistence, documents::MemberStatsDocument};
use dynamo_domain_stats::{
    MemberStatsDelta, MemberStatsRecord, MemberStatsUpdate, StatsLeaderboardMetric,
};
use dynamo_repositories::MemberStatsRepository;

impl MongoPersistence {
    async fn load_or_create_stats(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<MemberStatsDocument, Error> {
        let id = format!("stats:{guild_id}:{member_id}");
        if let Some(document) = self.member_stats.find_one(doc! { "_id": &id }).await? {
            return Ok(document);
        }
        let filter = doc! { "guild_id": guild_id.to_string(), "member_id": member_id.to_string() };
        if let Some(document) = self.member_stats.find_one(filter.clone()).await? {
            return Ok(document);
        }
        let now = chrono::Utc::now();
        let document = MemberStatsDocument::from_domain(MemberStatsRecord {
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
        });
        // The built-in unique _id index makes concurrent first events create
        // one row. Legacy rows retain their existing ObjectId and are read above.
        let insertion = self
            .member_stats
            .update_one(
                doc! { "_id": &id },
                doc! { "$setOnInsert": to_document(&document)? },
            )
            .upsert(true)
            .await;
        if let Some(document) = self.member_stats.find_one(filter).await? {
            return Ok(document);
        }
        insertion?;
        anyhow::bail!("member stats initialization did not produce a document")
    }
}

#[async_trait]
impl MemberStatsRepository for MongoPersistence {
    async fn get_or_create(
        &self,
        guild_id: u64,
        member_id: u64,
    ) -> Result<MemberStatsRecord, Error> {
        self.load_or_create_stats(guild_id, member_id)
            .await?
            .into_domain()
    }

    async fn apply_delta(
        &self,
        guild_id: u64,
        member_id: u64,
        delta: MemberStatsDelta,
    ) -> Result<MemberStatsUpdate, Error> {
        for _ in 0..128 {
            let previous = self.load_or_create_stats(guild_id, member_id).await?;
            let id = previous
                .id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("member stats has no Mongo id"))?;
            let revision = previous.revision;
            let mut record = previous.into_domain()?;
            let leveled_up = record
                .apply_delta(delta, chrono::Utc::now())
                .map_err(anyhow::Error::msg)?;
            let mut next = MemberStatsDocument::from_domain(record.clone());
            next.id = Some(id.clone());
            next.revision = revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("member stats revision overflow"))?;
            let mut filter = doc! { "_id": id };
            if revision == 0 {
                filter.insert(
                    "$or",
                    mongodb::bson::bson!([
                        { "revision": 0 }, { "revision": { "$exists": false } }
                    ]),
                );
            } else {
                filter.insert("revision", mongodb::bson::to_bson(&revision)?);
            }
            let result = self.member_stats.replace_one(filter, next).await?;
            if result.matched_count == 1 {
                return Ok(MemberStatsUpdate { record, leveled_up });
            }
            // Another event committed first; reapply to its newer state.
        }
        anyhow::bail!("member stats update contention exceeded retry limit")
    }

    async fn leaderboard(
        &self,
        guild_id: u64,
        metric: StatsLeaderboardMetric,
        limit: u32,
    ) -> Result<Vec<MemberStatsRecord>, Error> {
        let sort = match metric {
            StatsLeaderboardMetric::Xp => doc! { "level": -1, "xp": -1 },
            StatsLeaderboardMetric::Messages => doc! { "messages": -1 },
            StatsLeaderboardMetric::VoiceSeconds => doc! { "voice.time_seconds": -1 },
        };
        let mut cursor = self
            .member_stats
            .find(doc! { "guild_id": guild_id.to_string() })
            .sort(sort)
            .limit(i64::from(limit))
            .await?;
        let mut entries = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            entries.push(document.into_domain()?);
        }
        Ok(entries)
    }
}
