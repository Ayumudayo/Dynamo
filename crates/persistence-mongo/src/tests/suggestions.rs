use chrono::Utc;
use dynamo_domain_suggestion::{
    SuggestionRecord, SuggestionStats, SuggestionStatus, SuggestionStatusUpdate,
};
use dynamo_repositories::SuggestionsRepository;
use mongodb::bson::doc;

use super::support::run_isolated_mongo_test;
use crate::MongoPersistence;

async fn suggestion_move_cas_body(store: MongoPersistence) -> anyhow::Result<()> {
    store.ensure_initialized().await?;
    let original = SuggestionRecord {
        guild_id: 42,
        channel_id: 100,
        message_id: 101,
        user_id: 1,
        suggestion: "move safely".into(),
        status: SuggestionStatus::Pending,
        stats: SuggestionStats::default(),
        status_updates: vec![],
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    SuggestionsRepository::create(&store, original).await?;
    // Historical documents may not have a status_updates array at all.
    store
        .suggestions
        .update_one(
            doc! { "guild_id": "42", "message_id": "101" },
            doc! { "$unset": { "status_updates": "" } },
        )
        .await?;
    let expected = store.get_by_message(42, 101).await?.unwrap();
    let mut approved = expected.clone();
    approved.channel_id = 200;
    approved.message_id = 201;
    approved.status = SuggestionStatus::Approved;
    approved.status_updates.push(SuggestionStatusUpdate {
        user_id: 2,
        status: approved.status,
        reason: None,
        timestamp: Utc::now(),
    });
    let mut rejected = approved.clone();
    rejected.message_id = 301;
    rejected.channel_id = 300;
    rejected.status = SuggestionStatus::Rejected;
    rejected.status_updates[0].status = rejected.status;
    let (first, second) = tokio::join!(
        store.replace_if_unchanged(&expected, approved),
        store.replace_if_unchanged(&expected, rejected),
    );
    let first = first?;
    let second = second?;
    assert_ne!(
        first.is_some(),
        second.is_some(),
        "exactly one reviewer must win"
    );
    let winner = first.or(second).unwrap();
    assert!(
        store.get_by_message(42, 101).await?.is_none(),
        "old pending document must be gone"
    );
    assert_eq!(
        store
            .get_by_message(42, winner.message_id)
            .await?
            .unwrap()
            .status,
        winner.status
    );
    assert_eq!(
        store
            .suggestions
            .count_documents(doc! { "guild_id": "42" })
            .await?,
        1
    );
    assert!(
        store
            .replace_if_unchanged(&expected, expected.clone())
            .await?
            .is_none(),
        "a stale save must never upsert the original"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires scripts/test-isolated-mongo.ps1 and a disposable MongoDB"]
async fn suggestion_move_has_one_winner_and_no_orphan_against_mongo() -> anyhow::Result<()> {
    run_isolated_mongo_test(suggestion_move_cas_body).await
}
