use chrono::{Duration, Utc};
use dynamo_domain_giveaway::{GiveawayRecord, GiveawayStatus};
use dynamo_repositories::GiveawaysRepository;
use mongodb::bson::doc;

use super::support::run_isolated_mongo_test;

#[tokio::test]
#[ignore = "requires scripts/test-isolated-mongo.ps1 and a disposable MongoDB"]
async fn giveaway_cas_rejects_stale_entries_and_preserves_pending_delivery_against_mongo()
-> anyhow::Result<()> {
    run_isolated_mongo_test(|store| async move {
        let draft = GiveawayRecord {
            guild_id: 42,
            channel_id: 43,
            message_id: 44,
            ends_at: Utc::now() + Duration::hours(1),
            entries: vec![1],
            ..Default::default()
        };
        store.create(draft).await?;
        let original = store.get_by_message(42, 44).await?.unwrap();
        let mut entry_a = original.clone();
        entry_a.entries.push(2);
        let mut entry_b = original.clone();
        entry_b.entries.push(3);
        let (a, b) = tokio::join!(
            store.compare_and_save(entry_a),
            store.compare_and_save(entry_b)
        );
        assert_ne!(
            a?.is_some(),
            b?.is_some(),
            "only one stale snapshot may commit"
        );
        let mut current = store.get_by_message(42, 44).await?.unwrap();
        let missing_entry = if current.entries.contains(&2) { 3 } else { 2 };
        current.entries.push(missing_entry);
        let current = store.compare_and_save(current).await?.unwrap();
        assert_eq!(current.entries.len(), 3);
        let stale_active = current.clone();
        let mut ended = current;
        ended.status = GiveawayStatus::Ended;
        ended.winner_ids = vec![2];
        ended.message_sync_pending = true;
        ended.announcement_pending = true;
        let ended = store.compare_and_save(ended).await?.unwrap();
        assert!(store.compare_and_save(stale_active).await?.is_none());
        // The deadline is still in the future, but durable delivery work must
        // survive rebuilding the repository after a restart.
        let restored_store = crate::MongoPersistence::from_database(store.database().clone());
        let pending = restored_store.list_due_before(Utc::now()).await?;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, GiveawayStatus::Ended);
        assert_eq!(pending[0].winner_ids, vec![2]);
        let mut acknowledged = ended;
        acknowledged.message_sync_pending = false;
        acknowledged.announcement_pending = false;
        restored_store
            .compare_and_save(acknowledged)
            .await?
            .unwrap();
        assert!(restored_store.list_due_before(Utc::now()).await?.is_empty());
        assert_eq!(store.giveaways.count_documents(doc! {}).await?, 1);
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "requires scripts/test-isolated-mongo.ps1 and a disposable MongoDB"]
async fn legacy_giveaway_revision_and_delivery_defaults_against_mongo() -> anyhow::Result<()> {
    run_isolated_mongo_test(|store| async move {
        let record = GiveawayRecord {
            guild_id: 42,
            channel_id: 43,
            message_id: 45,
            status: GiveawayStatus::Ended,
            winner_ids: vec![7],
            ..Default::default()
        };
        store.create(record).await?;
        store.database().collection::<mongodb::bson::Document>("giveaways")
            .update_one(doc! {"guild_id": "42", "message_id": "45"},
                doc! {"$unset": {"revision": "", "message_sync_pending": "", "announcement_pending": ""}})
            .await?;
        let legacy = store.get_by_message(42, 45).await?.unwrap();
        assert_eq!(legacy.revision, 0);
        assert!(!legacy.announcement_pending);
        assert!(!legacy.message_sync_pending);
        assert!(store.list_due_before(Utc::now()).await?.is_empty());
        let saved = store.compare_and_save(legacy.clone()).await?.unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.winner_ids, vec![7]);
        assert!(store.compare_and_save(legacy).await?.is_none());
        assert_eq!(store.giveaways.count_documents(doc! {}).await?, 1);
        Ok(())
    }).await
}
