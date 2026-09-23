use async_trait::async_trait;
use dynamo_ops::{
    DashboardAuditLogEntry, DashboardAuditLogPage, DashboardAuditLogQuery,
    DashboardAuditLogRepository,
};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, to_bson};

use crate::{Error, MongoPersistence, documents::DashboardAuditLogDocument};

#[async_trait]
impl DashboardAuditLogRepository for MongoPersistence {
    async fn append(
        &self,
        record: DashboardAuditLogEntry,
    ) -> Result<DashboardAuditLogEntry, Error> {
        let mut document = DashboardAuditLogDocument::from_domain(record);
        let result = self
            .dashboard_audit_logs
            .insert_one(document.clone())
            .await?;
        document.id = result.inserted_id.as_object_id();
        document.into_domain()
    }

    async fn list(&self, query: DashboardAuditLogQuery) -> Result<DashboardAuditLogPage, Error> {
        let page = query.page.max(1);
        let page_size = query.page_size.clamp(1, 100);
        let skip = page.saturating_sub(1).saturating_mul(page_size);

        let mut filter = doc! {
            "scope": to_bson(&query.scope)?,
        };
        if let Some(guild_id) = query.guild_id {
            filter.insert("guild_id", guild_id.to_string());
        }
        if let Some(entity_type) = query.entity_type {
            filter.insert("entity_type", to_bson(&entity_type)?);
        }
        if let Some(action) = query.action {
            filter.insert("action", to_bson(&action)?);
        }

        let total = self
            .dashboard_audit_logs
            .count_documents(filter.clone())
            .await?;
        let mut cursor = self
            .dashboard_audit_logs
            .find(filter)
            .sort(doc! { "timestamp": -1, "_id": -1 })
            .skip(skip)
            .limit(page_size as i64)
            .await?;

        let mut entries = Vec::new();
        while let Some(document) = cursor.try_next().await? {
            entries.push(document.into_domain()?);
        }

        Ok(DashboardAuditLogPage {
            entries,
            page,
            page_size,
            total,
        })
    }
}
