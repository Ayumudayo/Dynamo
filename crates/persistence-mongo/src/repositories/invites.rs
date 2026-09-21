use async_trait::async_trait;
use futures_util::TryStreamExt;
use mongodb::bson::doc;

use crate::{Error, MongoPersistence, documents::InviteMemberDocument};
use dynamo_domain_invite::{InviteLeaderboardEntry, InviteMemberRecord};
use dynamo_repositories::InviteRepository;

#[async_trait]
impl InviteRepository for MongoPersistence {
    async fn get_or_create(
        &self,
        guild_id: u64,
        member_id: &str,
    ) -> Result<InviteMemberRecord, Error> {
        let document = self
            .invite_members
            .find_one(doc! {
                "guild_id": guild_id.to_string(),
                "member_id": member_id,
            })
            .await?;

        if let Some(document) = document {
            return document.into_domain();
        }

        let now = chrono::Utc::now();
        let record = InviteMemberRecord {
            guild_id,
            member_id: member_id.to_string(),
            invite_data: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let document = InviteMemberDocument::from_domain(record);
        self.invite_members.insert_one(document.clone()).await?;
        document.into_domain()
    }

    async fn save(&self, record: InviteMemberRecord) -> Result<InviteMemberRecord, Error> {
        let document = InviteMemberDocument::from_domain(record);
        self.invite_members
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

    async fn leaderboard(
        &self,
        guild_id: u64,
        limit: u32,
    ) -> Result<Vec<InviteLeaderboardEntry>, Error> {
        let pipeline = vec![
            doc! { "$match": { "guild_id": guild_id.to_string() } },
            doc! {
                "$project": {
                    "member_id": "$member_id",
                    "invites": {
                        "$subtract": [
                            { "$add": ["$invite_data.tracked", "$invite_data.added"] },
                            { "$add": ["$invite_data.left", "$invite_data.fake"] }
                        ]
                    }
                }
            },
            doc! { "$match": { "invites": { "$gt": 0 } } },
            doc! { "$sort": { "invites": -1 } },
            doc! { "$limit": limit as i64 },
        ];

        let mut cursor = self.invite_members.aggregate(pipeline).await?;
        let mut entries = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            let member_id = document
                .get_str("member_id")
                .map_err(|error| anyhow::anyhow!("invite leaderboard member_id missing: {error}"))?
                .to_string();
            let invites = document
                .get_i64("invites")
                .map_err(|error| anyhow::anyhow!("invite leaderboard invites missing: {error}"))?;
            entries.push(InviteLeaderboardEntry { member_id, invites });
        }
        Ok(entries)
    }
}
