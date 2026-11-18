use dynamo_domain_stats::MemberStatsDelta;
use dynamo_repositories::MemberStatsRepository;
use mongodb::bson::{doc, oid::ObjectId};

use super::support::run_isolated_mongo_test;
use crate::MongoPersistence;

async fn concurrent_stats_body(store: MongoPersistence) -> anyhow::Result<()> {
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..24 {
        let message_store = store.clone();
        tasks.spawn(async move {
            message_store
                .apply_delta(
                    42,
                    7,
                    MemberStatsDelta {
                        messages: 1,
                        xp: 10,
                        ..Default::default()
                    },
                )
                .await
        });
        let command_store = store.clone();
        tasks.spawn(async move {
            command_store
                .apply_delta(
                    42,
                    7,
                    MemberStatsDelta {
                        slash_commands: 1,
                        voice_connections: 1,
                        voice_seconds: 30,
                        ..Default::default()
                    },
                )
                .await
        });
    }
    let mut level_up_count = 0;
    while let Some(result) = tasks.join_next().await {
        if result??.leveled_up {
            level_up_count += 1;
        }
    }
    let record = store.get_or_create(42, 7).await?;
    assert_eq!(record.messages, 24);
    assert_eq!(record.commands.slash, 24);
    assert_eq!(record.voice.connections, 24);
    assert_eq!(record.voice.time_seconds, 720);
    assert_eq!(record.xp, 140);
    assert_eq!(record.level, 2);
    assert_eq!(level_up_count, 1);
    assert_eq!(
        store
            .member_stats
            .count_documents(doc! { "guild_id": "42", "member_id": "7" })
            .await?,
        1
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires scripts/test-isolated-mongo.ps1 and a disposable MongoDB"]
async fn concurrent_activity_preserves_counters_and_xp_against_mongo() -> anyhow::Result<()> {
    run_isolated_mongo_test(concurrent_stats_body).await
}

async fn legacy_stats_body(store: MongoPersistence) -> anyhow::Result<()> {
    let id = ObjectId::new();
    let now = mongodb::bson::DateTime::now();
    let collection = store
        .database()
        .collection::<mongodb::bson::Document>("member-stats");
    collection
        .insert_one(doc! {
            "_id": id, "guild_id": "42", "member_id": "9", "messages": 5,
            "voice": {}, "commands": {}, "contexts": {}, "xp": 95, "level": 1,
            "created_at": now, "updated_at": now,
        })
        .await?;
    let update = store
        .apply_delta(
            42,
            9,
            MemberStatsDelta {
                messages: 1,
                xp: 10,
                ..Default::default()
            },
        )
        .await?;
    assert!(update.leveled_up);
    assert_eq!(update.record.messages, 6);
    assert_eq!(update.record.xp, 5);
    assert_eq!(update.record.level, 2);
    assert_eq!(collection.count_documents(doc! {}).await?, 1);
    let saved = collection
        .find_one(doc! { "_id": id })
        .await?
        .expect("legacy row preserved");
    assert_eq!(
        saved
            .get_i64("revision")
            .or_else(|_| saved.get_i32("revision").map(i64::from))?,
        1
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires scripts/test-isolated-mongo.ps1 and a disposable MongoDB"]
async fn legacy_stats_migrates_without_duplicate_member_against_mongo() -> anyhow::Result<()> {
    run_isolated_mongo_test(legacy_stats_body).await
}
