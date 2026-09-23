use mongodb::bson::doc;

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

    pub async fn ensure_initialized(&self) -> Result<(), Error> {
        self.ensure_initialized_report().await.map(|_| ())
    }
}
