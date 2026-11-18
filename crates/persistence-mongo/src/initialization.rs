use mongodb::{IndexModel, bson::doc};

use crate::{Error, MongoPersistence, config::DEPLOYMENT_SETTINGS_ID};

#[derive(Debug, Clone)]
pub struct MongoInitializationReport {
    pub database_name: String,
    pub existing_collections: Vec<String>,
    pub created_collections: Vec<String>,
    pub final_collections: Vec<String>,
    pub deployment_settings_seeded: bool,
}

impl MongoPersistence {
    pub async fn ensure_initialized_report(&self) -> Result<MongoInitializationReport, Error> {
        let existing_collections = self.database.list_collection_names().await?;
        let mut created_collections = Vec::new();

        for collection_name in [
            "guild_settings",
            "deployment_settings",
            "provider_state",
            "suggestions",
            "giveaways",
            "members",
            "member-stats",
            "mod-logs",
            "dashboard-audit-logs",
        ] {
            if !existing_collections
                .iter()
                .any(|name| name == collection_name)
            {
                self.database.create_collection(collection_name).await?;
                created_collections.push(collection_name.to_string());
            }
        }

        self.ensure_indexes().await?;

        let deployment_settings_result = self
            .deployment_settings
            .update_one(
                doc! { "_id": DEPLOYMENT_SETTINGS_ID },
                doc! {
                    "$setOnInsert": {
                        "_id": DEPLOYMENT_SETTINGS_ID,
                        "modules": {}
                    }
                },
            )
            .upsert(true)
            .await?;

        let final_collections = self.database.list_collection_names().await?;

        Ok(MongoInitializationReport {
            database_name: self.database.name().to_string(),
            existing_collections,
            created_collections,
            final_collections,
            deployment_settings_seeded: deployment_settings_result.upserted_id.is_some(),
        })
    }

    async fn ensure_indexes(&self) -> Result<(), Error> {
        fn index(keys: mongodb::bson::Document) -> IndexModel {
            IndexModel::builder().keys(keys).build()
        }

        self.suggestions
            .create_index(index(doc! { "guild_id": 1, "message_id": 1 }))
            .await?;

        self.giveaways
            .create_indexes([
                index(doc! { "guild_id": 1, "message_id": 1 }),
                index(doc! { "status": 1, "ends_at": 1 }),
                index(doc! { "message_sync_pending": 1 }),
                index(doc! { "announcement_pending": 1 }),
                index(doc! { "guild_id": 1, "created_at": -1 }),
            ])
            .await?;

        self.invite_members
            .create_index(index(doc! { "guild_id": 1, "member_id": 1 }))
            .await?;

        self.member_stats
            .create_indexes([
                index(doc! { "guild_id": 1, "member_id": 1 }),
                index(doc! { "guild_id": 1, "level": -1, "xp": -1 }),
                index(doc! { "guild_id": 1, "messages": -1 }),
                index(doc! { "guild_id": 1, "voice.time_seconds": -1 }),
            ])
            .await?;

        self.warning_logs
            .create_index(index(
                doc! { "guild_id": 1, "member_id": 1, "created_at": -1 },
            ))
            .await?;

        self.dashboard_audit_logs
            .create_indexes([
                index(doc! { "timestamp": -1, "_id": -1 }),
                index(doc! { "scope": 1, "guild_id": 1, "timestamp": -1, "_id": -1 }),
            ])
            .await?;

        Ok(())
    }

    pub async fn ensure_initialized(&self) -> Result<(), Error> {
        self.ensure_initialized_report().await.map(|_| ())
    }
}
