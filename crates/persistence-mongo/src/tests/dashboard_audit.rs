use dynamo_ops::{
    DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry, DashboardAuditLogQuery,
    DashboardAuditLogRepository, DashboardAuditScope,
};

use crate::MongoPersistence;

use super::support::run_isolated_mongo_test;

async fn dashboard_audit_logs_round_trip_body(store: MongoPersistence) -> anyhow::Result<()> {
    store.ensure_initialized().await?;
    let marker = format!("integration::{}", chrono::Utc::now().timestamp_millis());
    let entry = DashboardAuditLogEntry {
        id: None,
        timestamp: chrono::Utc::now(),
        actor_user_id: 1,
        actor_username: "integration-test".to_string(),
        scope: DashboardAuditScope::Guild,
        guild_id: Some(42),
        entity_type: DashboardAuditEntityType::Command,
        entity_id: marker.clone(),
        action: DashboardAuditAction::SaveSettings,
        summary: "Saved guild settings for command integration::test.".to_string(),
    };

    let saved = store.append(entry).await?;
    assert!(saved.id.is_some());

    let page = store
        .list(DashboardAuditLogQuery {
            scope: DashboardAuditScope::Guild,
            guild_id: Some(42),
            entity_type: Some(DashboardAuditEntityType::Command),
            action: Some(DashboardAuditAction::SaveSettings),
            page: 1,
            page_size: 10,
        })
        .await?;

    assert!(page.entries.iter().any(|row| row.entity_id == marker));
    Ok(())
}

#[tokio::test]
#[ignore = "requires scripts/test-isolated-mongo.ps1 and a disposable MongoDB"]
async fn dashboard_audit_logs_round_trip_against_mongo() -> anyhow::Result<()> {
    run_isolated_mongo_test(dashboard_audit_logs_round_trip_body).await
}
