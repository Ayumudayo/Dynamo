mod config;
mod documents;
mod ids;
mod initialization;
mod repositories;
mod settings;
mod store;

pub use config::{DEFAULT_DATABASE_NAME, MongoPersistenceConfig};
pub use initialization::MongoInitializationReport;
pub use store::MongoPersistence;

use crate::documents::DashboardAuditLogDocument;

use async_trait::async_trait;
use dynamo_ops::{
    DashboardAuditLogEntry, DashboardAuditLogPage, DashboardAuditLogQuery,
    DashboardAuditLogRepository,
};
use futures_util::TryStreamExt;
use mongodb::bson::{doc, to_bson};

type Error = anyhow::Error;

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

#[cfg(test)]
mod tests {
    use super::{DEFAULT_DATABASE_NAME, MongoPersistence};
    use crate::MongoInitializationReport;
    use crate::documents::{DeploymentSettingsDocument, GuildSettingsDocument};
    use dynamo_ops::DashboardAuditLogRepository;
    use dynamo_ops::{
        DashboardAuditAction, DashboardAuditEntityType, DashboardAuditLogEntry,
        DashboardAuditLogQuery, DashboardAuditScope,
    };
    use dynamo_repositories::{DeploymentSettingsRepository, GuildSettingsRepository};
    use dynamo_settings::{
        DeploymentCommandSettings, DeploymentModuleSettings, GuildCommandSettings,
        GuildModuleSettings,
    };
    use futures_util::FutureExt;
    use mongodb::{
        Client,
        bson::{Bson, doc, oid::ObjectId, to_bson},
    };
    use serde_json::json;
    use std::{
        any::Any,
        env,
        future::Future,
        panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    };

    type PanicPayload = Box<dyn Any + Send + 'static>;

    enum IsolatedTestOutcome {
        Completed(anyhow::Result<()>),
        Panicked(PanicPayload),
    }

    struct IsolatedMongoTest {
        client: Client,
        database_name: String,
    }

    impl IsolatedMongoTest {
        async fn create() -> anyhow::Result<Self> {
            let connection_string = env::var("MONGODB_URI_FOR_ISOLATED_TEST").map_err(|_| {
                anyhow::anyhow!("isolated Mongo tests require the dedicated PowerShell runner")
            })?;
            let client = Client::with_uri_str(connection_string)
                .await
                .map_err(|_| anyhow::anyhow!("isolated Mongo client initialization failed"))?;

            Ok(Self {
                client,
                database_name: isolated_database_name(),
            })
        }

        fn store(&self) -> MongoPersistence {
            MongoPersistence::from_database(self.client.database(&self.database_name))
        }

        async fn cleanup(self) -> anyhow::Result<()> {
            self.client
                .database(&self.database_name)
                .drop()
                .await
                .map_err(|_| anyhow::anyhow!("isolated Mongo database drop failed"))?;

            let remaining_databases = self
                .client
                .list_database_names()
                .await
                .map_err(|_| anyhow::anyhow!("isolated Mongo cleanup verification failed"))?;
            anyhow::ensure!(
                !remaining_databases
                    .iter()
                    .any(|name| name == &self.database_name),
                "isolated Mongo database still exists after cleanup"
            );
            Ok(())
        }
    }

    fn isolated_database_name() -> String {
        format!(
            "dynmongo_{}_{}",
            std::process::id(),
            ObjectId::new().to_hex()
        )
    }

    fn resolve_isolated_test_outcome(
        test_outcome: IsolatedTestOutcome,
        cleanup_result: anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        if let Err(error) = cleanup_result {
            return Err(anyhow::anyhow!("isolated Mongo cleanup failed: {error}"));
        }

        match test_outcome {
            IsolatedTestOutcome::Completed(result) => result,
            IsolatedTestOutcome::Panicked(payload) => resume_unwind(payload),
        }
    }

    async fn run_isolated_test_lifecycle<F, Fut, C, CleanupFut>(
        test: F,
        cleanup: C,
    ) -> anyhow::Result<()>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
        C: FnOnce() -> CleanupFut,
        CleanupFut: Future<Output = anyhow::Result<()>>,
    {
        let test_outcome = match catch_unwind(AssertUnwindSafe(test)) {
            Ok(future) => match AssertUnwindSafe(future).catch_unwind().await {
                Ok(result) => IsolatedTestOutcome::Completed(result),
                Err(payload) => IsolatedTestOutcome::Panicked(payload),
            },
            Err(payload) => IsolatedTestOutcome::Panicked(payload),
        };
        let cleanup_result = cleanup().await;

        resolve_isolated_test_outcome(test_outcome, cleanup_result)
    }

    async fn run_isolated_mongo_test<F, Fut>(test: F) -> anyhow::Result<()>
    where
        F: FnOnce(MongoPersistence) -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        let isolated = IsolatedMongoTest::create().await?;
        let store = isolated.store();

        run_isolated_test_lifecycle(|| test(store), || isolated.cleanup()).await
    }

    #[test]
    fn initialization_report_can_include_dashboard_audit_collection() {
        let report = MongoInitializationReport {
            database_name: DEFAULT_DATABASE_NAME.to_string(),
            existing_collections: vec!["guild_settings".to_string()],
            created_collections: vec!["dashboard-audit-logs".to_string()],
            final_collections: vec![
                "guild_settings".to_string(),
                "dashboard-audit-logs".to_string(),
            ],
            deployment_settings_seeded: false,
        };

        assert!(
            report
                .final_collections
                .iter()
                .any(|value| value == "dashboard-audit-logs")
        );
    }

    #[test]
    fn guild_default_document_initializes_required_fields() {
        let document = GuildSettingsDocument::default_for_guild(42);

        assert_eq!(document.id, "42");
        assert!(document.modules.is_empty());
        assert!(document.commands.is_empty());
    }

    #[test]
    fn guild_upsert_update_skips_conflicting_module_parent_on_insert() {
        let settings = GuildModuleSettings {
            enabled: false,
            configuration: json!({ "threshold": 7 }),
        };

        let update = crate::settings::settings_upsert_update(
            "42",
            "modules.stock",
            to_bson(&settings).expect("guild module settings serialize"),
        );

        assert_eq!(
            update,
            doc! {
                "$setOnInsert": {
                    "_id": "42",
                    "commands": {},
                },
                "$set": {
                    "modules.stock": {
                        "enabled": false,
                        "configuration": { "threshold": 7i64 },
                    },
                },
            }
        );
    }

    #[test]
    fn guild_upsert_update_skips_conflicting_command_parent_on_insert() {
        let settings = GuildCommandSettings {
            enabled: false,
            configuration: json!({ "precision": 2 }),
        };

        let update = crate::settings::settings_upsert_update(
            "42",
            "commands.exchange::rate",
            to_bson(&settings).expect("guild command settings serialize"),
        );

        assert_eq!(
            update,
            doc! {
                "$setOnInsert": {
                    "_id": "42",
                    "modules": {},
                },
                "$set": {
                    "commands.exchange::rate": {
                        "enabled": false,
                        "configuration": { "precision": 2i64 },
                    },
                },
            }
        );
    }

    #[test]
    fn deployment_upsert_update_skips_conflicting_module_parent_on_insert() {
        let settings = DeploymentModuleSettings {
            installed: false,
            enabled: true,
        };

        let update = crate::settings::settings_upsert_update(
            "global",
            "modules.stock",
            to_bson(&settings).expect("deployment module settings serialize"),
        );

        assert_eq!(
            update,
            doc! {
                "$setOnInsert": {
                    "_id": "global",
                    "commands": {},
                },
                "$set": {
                    "modules.stock": {
                        "installed": false,
                        "enabled": true,
                    },
                },
            }
        );
    }

    #[test]
    fn deployment_upsert_update_skips_conflicting_command_parent_on_insert() {
        let settings = DeploymentCommandSettings {
            installed: true,
            enabled: false,
            configuration: json!({ "visible": true }),
        };

        let update = crate::settings::settings_upsert_update(
            "global",
            "commands.exchange::rate",
            to_bson(&settings).expect("deployment command settings serialize"),
        );

        assert_eq!(
            update,
            doc! {
                "$setOnInsert": {
                    "_id": "global",
                    "modules": {},
                },
                "$set": {
                    "commands.exchange::rate": {
                        "installed": true,
                        "enabled": false,
                        "configuration": { "visible": true },
                    },
                },
            }
        );
    }

    #[test]
    fn deployment_default_document_initializes_required_fields() {
        let document = DeploymentSettingsDocument::default_document();

        assert_eq!(document.id, "global");
        assert_eq!(
            to_bson(&document.modules).ok(),
            Some(Bson::Document(doc! {}))
        );
        assert_eq!(
            to_bson(&document.commands).ok(),
            Some(Bson::Document(doc! {}))
        );
    }

    #[test]
    fn isolated_mongo_test_generates_exact_database_name_shape() {
        let database_name = isolated_database_name();
        let parts = database_name.split('_').collect::<Vec<_>>();

        assert_eq!(parts.len(), 3, "{database_name}");
        assert_eq!(parts[0], "dynmongo");
        assert_eq!(parts[1], std::process::id().to_string());
        assert_eq!(parts[2].len(), 24, "{database_name}");
        assert!(
            parts[2]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{database_name}"
        );
    }

    #[test]
    fn cleanup_failure_outranks_test_error() {
        let outcome = IsolatedTestOutcome::Completed(Err(anyhow::anyhow!("test failed")));
        let result =
            resolve_isolated_test_outcome(outcome, Err(anyhow::anyhow!("cleanup sentinel")));

        let error = result.expect_err("cleanup failure must win").to_string();
        assert!(error.contains("isolated Mongo cleanup failed"));
        assert!(error.contains("cleanup sentinel"));
        assert!(!error.contains("test failed"));
    }

    #[test]
    fn cleanup_failure_outranks_test_panic() {
        let outcome = IsolatedTestOutcome::Panicked(Box::new("panic sentinel"));
        let result =
            resolve_isolated_test_outcome(outcome, Err(anyhow::anyhow!("cleanup sentinel")));

        let error = result.expect_err("cleanup failure must win").to_string();
        assert!(error.contains("isolated Mongo cleanup failed"));
        assert!(error.contains("cleanup sentinel"));
    }

    #[test]
    fn successful_cleanup_preserves_test_error() {
        let outcome = IsolatedTestOutcome::Completed(Err(anyhow::anyhow!("test sentinel")));
        let error = resolve_isolated_test_outcome(outcome, Ok(()))
            .expect_err("test error must be returned")
            .to_string();

        assert_eq!(error, "test sentinel");
    }

    #[test]
    fn successful_cleanup_resumes_test_panic() {
        let result = catch_unwind(AssertUnwindSafe(|| {
            resolve_isolated_test_outcome(
                IsolatedTestOutcome::Panicked(Box::new("panic sentinel")),
                Ok(()),
            )
        }));

        let payload = result.expect_err("test panic must resume");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"panic sentinel"));
    }

    #[tokio::test]
    async fn cleanup_runs_once_after_test_success() {
        let cleanup_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cleanup_count_for_task = cleanup_count.clone();

        let result = run_isolated_test_lifecycle(
            || async { Ok(()) },
            || async move {
                cleanup_count_for_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cleanup_runs_once_after_test_error() {
        let cleanup_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cleanup_count_for_task = cleanup_count.clone();

        let result = run_isolated_test_lifecycle(
            || async { Err(anyhow::anyhow!("test sentinel")) },
            || async move {
                cleanup_count_for_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await;

        assert_eq!(
            result.expect_err("test error must survive").to_string(),
            "test sentinel"
        );
        assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cleanup_runs_once_after_test_panic() {
        let cleanup_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cleanup_count_for_task = cleanup_count.clone();

        let result = AssertUnwindSafe(run_isolated_test_lifecycle(
            || async {
                panic!("panic sentinel");
            },
            || async move {
                cleanup_count_for_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        ))
        .catch_unwind()
        .await;

        assert!(result.is_err(), "test panic must resume after cleanup");
        assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cleanup_runs_once_after_synchronous_test_factory_panic() {
        let cleanup_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cleanup_count_for_task = cleanup_count.clone();

        let result = AssertUnwindSafe(run_isolated_test_lifecycle(
            || -> std::future::Ready<anyhow::Result<()>> {
                panic!("synchronous panic sentinel");
            },
            || async move {
                cleanup_count_for_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        ))
        .catch_unwind()
        .await;

        assert!(
            result.is_err(),
            "synchronous test factory panic must resume after cleanup"
        );
        assert_eq!(cleanup_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn settings_field_path_accepts_real_style_ids() {
        assert_eq!(
            crate::settings::settings_field_path("modules", "module", "stock").unwrap(),
            "modules.stock"
        );
        assert_eq!(
            crate::settings::settings_field_path("commands", "command", "exchange::rate").unwrap(),
            "commands.exchange::rate"
        );
    }

    #[test]
    fn settings_field_path_rejects_invalid_ids() {
        let empty = crate::settings::settings_field_path("modules", "module", "").unwrap_err();
        assert!(
            empty
                .to_string()
                .contains("module id cannot be empty for Mongo settings paths")
        );

        let dotted = crate::settings::settings_field_path("modules", "module", "a.b").unwrap_err();
        assert!(
            dotted
                .to_string()
                .contains("module id `a.b` cannot contain `.` for Mongo settings paths")
        );

        let dollar =
            crate::settings::settings_field_path("commands", "command", "$bad").unwrap_err();
        assert!(
            dollar
                .to_string()
                .contains("command id `$bad` cannot start with `$` for Mongo settings paths")
        );
    }

    async fn settings_round_trip_body(guild_store: MongoPersistence) -> anyhow::Result<()> {
        let marker = chrono::Utc::now().timestamp_millis().unsigned_abs();
        let created_guild_id = marker;
        let guild_module_guild_id = marker + 1;
        let guild_command_guild_id = marker + 2;
        let guild_module_id = format!("integration_guild_module_{marker}");
        let guild_command_id = format!("integration::guild::command::{marker}");

        let count_before_absent_read = guild_store.guild_settings.count_documents(doc! {}).await?;
        let absent = GuildSettingsRepository::get(&guild_store, created_guild_id).await?;
        let count_after_absent_read = guild_store.guild_settings.count_documents(doc! {}).await?;
        assert_eq!(absent, None);
        assert_eq!(count_after_absent_read, count_before_absent_read);

        let guild_module_settings = GuildModuleSettings {
            enabled: false,
            configuration: json!({ "threshold": 7 }),
        };
        let guild_after_module = GuildSettingsRepository::upsert_module_settings(
            &guild_store,
            guild_module_guild_id,
            &guild_module_id,
            guild_module_settings.clone(),
        )
        .await?;
        assert_eq!(guild_after_module.guild_id, guild_module_guild_id);
        assert_eq!(
            guild_after_module.modules.get(&guild_module_id),
            Some(&guild_module_settings)
        );
        assert!(guild_after_module.commands.is_empty());

        let count_before_existing_read =
            guild_store.guild_settings.count_documents(doc! {}).await?;
        let existing = GuildSettingsRepository::get(&guild_store, guild_module_guild_id)
            .await?
            .expect("upserted guild settings should exist");
        let count_after_existing_read = guild_store.guild_settings.count_documents(doc! {}).await?;
        assert_eq!(existing, guild_after_module);
        assert_eq!(count_after_existing_read, count_before_existing_read);

        let guild_module_settings_updated = GuildModuleSettings {
            enabled: true,
            configuration: json!({ "threshold": 11, "mode": "updated" }),
        };
        let guild_after_module_update = GuildSettingsRepository::upsert_module_settings(
            &guild_store,
            guild_module_guild_id,
            &guild_module_id,
            guild_module_settings_updated.clone(),
        )
        .await?;
        assert_eq!(
            guild_after_module_update.modules.get(&guild_module_id),
            Some(&guild_module_settings_updated)
        );
        assert_ne!(
            guild_after_module_update.modules.get(&guild_module_id),
            Some(&guild_module_settings)
        );

        let guild_command_settings = GuildCommandSettings {
            enabled: false,
            configuration: json!({ "mode": "strict" }),
        };
        let guild_after_command = GuildSettingsRepository::upsert_command_settings(
            &guild_store,
            guild_command_guild_id,
            &guild_command_id,
            guild_command_settings.clone(),
        )
        .await?;
        assert_eq!(guild_after_command.guild_id, guild_command_guild_id);
        assert_eq!(
            guild_after_command.commands.get(&guild_command_id),
            Some(&guild_command_settings)
        );
        assert!(guild_after_command.modules.is_empty());

        let guild_command_settings_updated = GuildCommandSettings {
            enabled: true,
            configuration: json!({ "mode": "relaxed", "version": 2 }),
        };
        let guild_after_command_update = GuildSettingsRepository::upsert_command_settings(
            &guild_store,
            guild_command_guild_id,
            &guild_command_id,
            guild_command_settings_updated.clone(),
        )
        .await?;
        assert_eq!(
            guild_after_command_update.commands.get(&guild_command_id),
            Some(&guild_command_settings_updated)
        );
        assert_ne!(
            guild_after_command_update.commands.get(&guild_command_id),
            Some(&guild_command_settings)
        );
        assert!(guild_after_command_update.modules.is_empty());

        let deployment_module_store = guild_store.clone();
        let deployment_module_id = format!("integration_deployment_module_{marker}");

        let deployment_module_settings = DeploymentModuleSettings {
            installed: false,
            enabled: true,
        };
        let deployment_after_module = DeploymentSettingsRepository::upsert_module_settings(
            &deployment_module_store,
            &deployment_module_id,
            deployment_module_settings.clone(),
        )
        .await?;
        assert_eq!(
            deployment_after_module.modules.get(&deployment_module_id),
            Some(&deployment_module_settings)
        );

        let deployment_module_settings_updated = DeploymentModuleSettings {
            installed: true,
            enabled: false,
        };
        let deployment_after_module_update = DeploymentSettingsRepository::upsert_module_settings(
            &deployment_module_store,
            &deployment_module_id,
            deployment_module_settings_updated.clone(),
        )
        .await?;
        assert_eq!(
            deployment_after_module_update
                .modules
                .get(&deployment_module_id),
            Some(&deployment_module_settings_updated)
        );
        assert_ne!(
            deployment_after_module_update
                .modules
                .get(&deployment_module_id),
            Some(&deployment_module_settings)
        );

        let deployment_command_store = guild_store.clone();
        let deployment_command_id = format!("integration::deployment::command::{marker}");
        let deployment_command_settings = DeploymentCommandSettings {
            installed: false,
            enabled: false,
            configuration: json!({ "mode": "dry-run" }),
        };
        let deployment_after_command = DeploymentSettingsRepository::upsert_command_settings(
            &deployment_command_store,
            &deployment_command_id,
            deployment_command_settings.clone(),
        )
        .await?;
        assert_eq!(
            deployment_after_command
                .commands
                .get(&deployment_command_id),
            Some(&deployment_command_settings)
        );
        assert_eq!(
            deployment_after_command.modules.get(&deployment_module_id),
            Some(&deployment_module_settings_updated)
        );

        let deployment_command_settings_updated = DeploymentCommandSettings {
            installed: true,
            enabled: true,
            configuration: json!({ "mode": "live", "version": 2 }),
        };
        let deployment_after_command_update =
            DeploymentSettingsRepository::upsert_command_settings(
                &deployment_command_store,
                &deployment_command_id,
                deployment_command_settings_updated.clone(),
            )
            .await?;
        assert_eq!(
            deployment_after_command_update
                .commands
                .get(&deployment_command_id),
            Some(&deployment_command_settings_updated)
        );
        assert_ne!(
            deployment_after_command_update
                .commands
                .get(&deployment_command_id),
            Some(&deployment_command_settings)
        );
        assert_eq!(
            deployment_after_command_update
                .modules
                .get(&deployment_module_id),
            Some(&deployment_module_settings_updated)
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires scripts/test-isolated-mongo.ps1 and a disposable MongoDB"]
    async fn settings_round_trip_against_mongo() -> anyhow::Result<()> {
        run_isolated_mongo_test(settings_round_trip_body).await
    }

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
}
