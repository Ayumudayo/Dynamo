use chrono::Utc;
use dynamo_domain_suggestion::{SuggestionRecord, SuggestionStatus, SuggestionStatusUpdate};
use dynamo_repositories::SuggestionsRepository;
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude::{
    ChannelId, CreateMessage, Error as SerenityError, GuildId, HttpError, Member, Message,
    Permissions, UserId,
};
use std::{
    collections::HashMap,
    future::Future,
    num::NonZeroU64,
    sync::{Arc, OnceLock, Weak},
};
use tokio::sync::Mutex;

use crate::{
    constants::MODULE_ID,
    render::{create_reviewed_embed, moderation_buttons, vote_stats},
    settings::SuggestionSettings,
};

// Serialize Discord side effects within this bot; the repository CAS also
// prevents stale writers in other processes from replacing review state.
async fn guild_review_lock(guild_id: u64) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<u64, Weak<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().await;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&guild_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(guild_id, Arc::downgrade(&lock));
    lock
}

async fn commit_move<C, CF, D, DF>(
    repo: &dyn SuggestionsRepository,
    expected: &SuggestionRecord,
    next: SuggestionRecord,
    cleanup_staged: C,
    delete_original: D,
) -> Result<Option<String>, Error>
where
    C: FnOnce() -> CF,
    CF: Future<Output = Result<(), Error>>,
    D: FnOnce() -> DF,
    DF: Future<Output = Result<(), Error>>,
{
    let saved = repo.replace_if_unchanged(expected, next.clone()).await;
    let failure = match saved {
        Ok(Some(_)) => None,
        Ok(None) => Some(anyhow::anyhow!(
            "Suggestion changed during review; refresh and try again."
        )),
        Err(error) => {
            // A server may commit before the response is lost. Never delete
            // the new message while it could already be the stored address.
            let committed = repo.get_by_message(next.guild_id, next.message_id).await;
            if committed.ok().flatten().is_some_and(|record| {
                record.channel_id == next.channel_id
                    && record.status == next.status
                    && record.status_updates.len() == next.status_updates.len()
            }) {
                None
            } else {
                return Err(anyhow::anyhow!(
                    "Could not confirm the review save; both messages were preserved. Check the latest suggestion before retrying: {error}"
                ));
            }
        }
    };
    if let Some(error) = failure {
        if let Err(cleanup_error) = cleanup_staged().await {
            return Err(anyhow::anyhow!(
                "Review was not saved; original message was preserved. Failed to remove the staged message: {cleanup_error}. Save error: {error}"
            ));
        }
        return Err(error);
    }
    Ok(delete_original().await.err().map(|error| {
        format!("Review was saved, but the old message could not be removed: {error}")
    }))
}

async fn commit_in_place<S, SF>(
    repo: &dyn SuggestionsRepository,
    expected: &SuggestionRecord,
    next: SuggestionRecord,
    sync_message: S,
) -> Result<(), Error>
where
    S: FnOnce() -> SF,
    SF: Future<Output = Result<(), Error>>,
{
    if repo.replace_if_unchanged(expected, next).await?.is_none() {
        anyhow::bail!("Suggestion changed during review; refresh and try again.");
    }
    sync_message().await.map_err(|error| anyhow::anyhow!(
        "Review was saved, but the message could not be updated. Retry the same review action to refresh it: {error}"
    ))
}

async fn refresh_review_message(
    ctx: &poise::serenity_prelude::Context,
    source_message: &Message,
    record: &SuggestionRecord,
) -> Result<Option<String>, Error> {
    let last_update = record.status_updates.last();
    let reviewer = if let Some(user_id) = last_update.and_then(|u| NonZeroU64::new(u.user_id)) {
        UserId::from(user_id).to_user(ctx).await.ok()
    } else {
        None
    };
    let name = reviewer
        .as_ref()
        .map(|user| user.name.clone())
        .unwrap_or_else(|| {
            last_update
                .map(|update| update.user_id.to_string())
                .unwrap_or_else(|| "moderator".to_string())
        });
    let avatar = reviewer
        .as_ref()
        .map(|user| user.face())
        .unwrap_or_else(|| source_message.author.face());
    let embed = create_reviewed_embed(
        record,
        &name,
        avatar,
        last_update.and_then(|update| update.reason.as_deref()),
    );
    source_message
        .channel_id
        .edit_message(
            ctx,
            source_message.id,
            poise::serenity_prelude::EditMessage::new()
                .embed(embed)
                .components(moderation_buttons(record.status)),
        )
        .await?;
    Ok(source_message
        .delete_reactions(ctx)
        .await
        .err()
        .map(|error| {
            format!(
                "Review was saved and displayed, but the old votes could not be cleared: {error}"
            )
        }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn transition_suggestion(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    repo: Arc<dyn SuggestionsRepository>,
    record: SuggestionRecord,
    source_message: &Message,
    moderator: &Member,
    settings: &SuggestionSettings,
    next_status: SuggestionStatus,
    reason: Option<String>,
) -> Result<String, Error> {
    let lock = guild_review_lock(record.guild_id).await;
    let _guard = lock.lock().await;
    let Some(mut record) = repo
        .get_by_message(record.guild_id, record.message_id)
        .await?
    else {
        return Ok("This suggestion has already moved. Use its latest message.".to_string());
    };
    let expected = record.clone();
    if record.status == SuggestionStatus::Deleted {
        return Ok("This suggestion has been deleted.".to_string());
    }
    if record.status == next_status {
        // A prior database commit can outlive a failed Discord edit. Repeating
        // the action repairs the message without duplicating review history.
        if let Some(warning) = refresh_review_message(ctx, source_message, &record).await? {
            return Ok(warning);
        }
        return Ok(match next_status {
            SuggestionStatus::Approved => "Suggestion already approved.".to_string(),
            SuggestionStatus::Rejected => "Suggestion already rejected.".to_string(),
            _ => "Suggestion already updated.".to_string(),
        });
    }

    let stats = if record.status == SuggestionStatus::Pending {
        vote_stats(source_message)
    } else {
        record.stats.clone()
    };
    record.status = next_status;
    record.stats = stats;
    record.status_updates.push(SuggestionStatusUpdate {
        user_id: moderator.user.id.get(),
        status: next_status,
        reason: reason.clone(),
        timestamp: Utc::now(),
    });
    record.updated_at = Utc::now();

    let target_channel_id = match next_status {
        SuggestionStatus::Approved => settings.approved_channel_id,
        SuggestionStatus::Rejected => settings.rejected_channel_id,
        _ => None,
    };

    let embed = create_reviewed_embed(
        &record,
        &moderator.user.name,
        moderator.user.face(),
        reason.as_deref(),
    );
    let buttons = moderation_buttons(next_status);

    let cleanup_warning;
    if let Some(target_channel_id) = target_channel_id
        .filter(|id| *id != source_message.channel_id.get())
        .and_then(NonZeroU64::new)
        .map(ChannelId::from)
    {
        let sent = target_channel_id
            .send_message(
                ctx,
                CreateMessage::new()
                    .embed(embed)
                    .components(buttons.clone()),
            )
            .await?;
        record.channel_id = sent.channel_id.get();
        record.message_id = sent.id.get();
        cleanup_warning = commit_move(
            repo.as_ref(),
            &expected,
            record,
            || async { sent.delete(ctx).await.map_err(Into::into) },
            || async { source_message.delete(ctx).await.map_err(Into::into) },
        )
        .await?;
    } else {
        commit_in_place(repo.as_ref(), &expected, record, || async {
            source_message
                .channel_id
                .edit_message(
                    ctx,
                    source_message.id,
                    poise::serenity_prelude::EditMessage::new()
                        .embed(embed)
                        .components(buttons),
                )
                .await?;
            Ok(())
        })
        .await?;
        cleanup_warning = source_message.delete_reactions(ctx).await.err().map(|error| {
            format!("Review was saved and displayed, but the old votes could not be cleared: {error}")
        });
    }

    if let Some(warning) = cleanup_warning {
        return Ok(warning);
    }

    let deployment = data.deployment_settings_cached().await?;
    let response = if deployment
        .modules
        .get(MODULE_ID)
        .is_some_and(|module| !module.enabled)
    {
        "Suggestion updated while the module is currently disabled for the deployment.".to_string()
    } else {
        match next_status {
            SuggestionStatus::Approved => "Suggestion approved.".to_string(),
            SuggestionStatus::Rejected => "Suggestion rejected.".to_string(),
            _ => "Suggestion updated.".to_string(),
        }
    };

    Ok(response)
}

pub(crate) async fn delete_suggestion(
    ctx: &poise::serenity_prelude::Context,
    repo: Arc<dyn SuggestionsRepository>,
    record: SuggestionRecord,
    source_message: &Message,
    moderator: &Member,
    reason: Option<String>,
) -> Result<String, Error> {
    let lock = guild_review_lock(record.guild_id).await;
    let _guard = lock.lock().await;
    let Some(mut record) = repo
        .get_by_message(record.guild_id, record.message_id)
        .await?
    else {
        return Ok("This suggestion has already moved. Use its latest message.".to_string());
    };
    if record.status != SuggestionStatus::Deleted {
        let expected = record.clone();
        record.status = SuggestionStatus::Deleted;
        record.updated_at = Utc::now();
        record.status_updates.push(SuggestionStatusUpdate {
            user_id: moderator.user.id.get(),
            status: SuggestionStatus::Deleted,
            reason,
            timestamp: Utc::now(),
        });
        if repo
            .replace_if_unchanged(&expected, record)
            .await?
            .is_none()
        {
            return Err(anyhow::anyhow!(
                "Suggestion changed during deletion; refresh and try again."
            ));
        }
    }
    // Keep the original message available for retry if persistence fails.
    match source_message.delete(ctx).await {
        Ok(()) => {}
        Err(SerenityError::Http(HttpError::UnsuccessfulRequest(resp)))
            if resp.error.code == 10008 => {}
        Err(error) => return Err(error.into()),
    }

    Ok("Suggestion deleted.".to_string())
}

pub(crate) fn has_moderation_permissions(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
    member: &Member,
    settings: &SuggestionSettings,
) -> bool {
    let perms = member
        .permissions
        .or_else(|| {
            ctx.cache
                .guild(guild_id)
                .map(|guild| guild.member_permissions(member))
        })
        .unwrap_or_else(Permissions::empty);
    if perms.manage_guild() || perms.administrator() {
        return true;
    }

    member
        .roles
        .iter()
        .any(|role_id| settings.staff_role_ids.contains(&role_id.get()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dynamo_domain_suggestion::SuggestionStats;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Repository {
        record: Mutex<SuggestionRecord>,
        fail: AtomicBool,
        fail_after_commit: AtomicBool,
    }

    #[async_trait::async_trait]
    impl SuggestionsRepository for Repository {
        async fn create(&self, record: SuggestionRecord) -> Result<SuggestionRecord, Error> {
            *self.record.lock().await = record.clone();
            Ok(record)
        }
        async fn get_by_message(
            &self,
            guild_id: u64,
            message_id: u64,
        ) -> Result<Option<SuggestionRecord>, Error> {
            let record = self.record.lock().await;
            Ok(
                (record.guild_id == guild_id && record.message_id == message_id)
                    .then(|| record.clone()),
            )
        }
        async fn replace_if_unchanged(
            &self,
            expected: &SuggestionRecord,
            next: SuggestionRecord,
        ) -> Result<Option<SuggestionRecord>, Error> {
            if self.fail.load(Ordering::SeqCst) {
                anyhow::bail!("database unavailable");
            }
            let mut record = self.record.lock().await;
            if record.message_id != expected.message_id
                || record.status != expected.status
                || record.status_updates.len() != expected.status_updates.len()
            {
                return Ok(None);
            }
            *record = next.clone();
            if self.fail_after_commit.load(Ordering::SeqCst) {
                anyhow::bail!("write response lost");
            }
            Ok(Some(next))
        }
    }

    fn record() -> SuggestionRecord {
        SuggestionRecord {
            guild_id: 1,
            channel_id: 2,
            message_id: 3,
            user_id: 4,
            suggestion: "proposal".into(),
            status: SuggestionStatus::Pending,
            stats: SuggestionStats::default(),
            status_updates: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn moved_review_replaces_original_before_deleting_its_message() {
        let original = record();
        let repo = Repository {
            record: Mutex::new(original.clone()),
            fail: AtomicBool::new(false),
            fail_after_commit: AtomicBool::new(false),
        };
        let mut next = original.clone();
        next.message_id = 10;
        next.status = SuggestionStatus::Approved;
        let deleted = AtomicBool::new(false);
        let warning = commit_move(
            &repo,
            &original,
            next,
            || async { panic!("successful commit must not remove destination") },
            || async {
                assert!(repo.get_by_message(1, 3).await?.is_none());
                assert_eq!(
                    repo.get_by_message(1, 10).await?.unwrap().status,
                    SuggestionStatus::Approved
                );
                deleted.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(warning.is_none());
        assert!(deleted.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unknown_commit_outcome_preserves_both_messages() {
        let original = record();
        let repo = Repository {
            record: Mutex::new(original.clone()),
            fail: AtomicBool::new(true),
            fail_after_commit: AtomicBool::new(false),
        };
        let mut next = original.clone();
        next.message_id = 10;
        let cleaned = AtomicBool::new(false);
        let result = commit_move(
            &repo,
            &original,
            next,
            || async {
                cleaned.store(true, Ordering::SeqCst);
                Ok(())
            },
            || async { panic!("save failure must not delete original") },
        )
        .await;
        assert!(result.is_err());
        assert!(!cleaned.load(Ordering::SeqCst));
        assert!(repo.get_by_message(1, 3).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn stale_review_cleans_staged_message_without_overwriting_winner() {
        let original = record();
        let mut winner = original.clone();
        winner.message_id = 10;
        winner.status = SuggestionStatus::Approved;
        let repo = Repository {
            record: Mutex::new(winner),
            fail: AtomicBool::new(false),
            fail_after_commit: AtomicBool::new(false),
        };
        let cleaned = AtomicBool::new(false);
        assert!(
            commit_move(
                &repo,
                &original,
                original.clone(),
                || async {
                    cleaned.store(true, Ordering::SeqCst);
                    Ok(())
                },
                || async { panic!("stale review must not delete original") },
            )
            .await
            .is_err()
        );
        assert!(cleaned.load(Ordering::SeqCst));
        assert_eq!(
            repo.get_by_message(1, 10).await.unwrap().unwrap().status,
            SuggestionStatus::Approved
        );
    }

    #[tokio::test]
    async fn original_delete_failure_keeps_committed_destination_addressable() {
        let original = record();
        let repo = Repository {
            record: Mutex::new(original.clone()),
            fail: AtomicBool::new(false),
            fail_after_commit: AtomicBool::new(false),
        };
        let mut next = original.clone();
        next.message_id = 10;
        let warning = commit_move(
            &repo,
            &original,
            next,
            || async { panic!("committed destination must remain") },
            || async { anyhow::bail!("Discord unavailable") },
        )
        .await
        .unwrap();
        assert!(warning.unwrap().contains("Review was saved"));
        assert!(repo.get_by_message(1, 10).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn lost_write_response_reconciles_destination_before_deleting_original() {
        let original = record();
        let repo = Repository {
            record: Mutex::new(original.clone()),
            fail: AtomicBool::new(false),
            fail_after_commit: AtomicBool::new(true),
        };
        let mut next = original.clone();
        next.message_id = 10;
        next.status = SuggestionStatus::Approved;
        let deleted = AtomicBool::new(false);
        commit_move(
            &repo,
            &original,
            next,
            || async { panic!("committed destination must not be removed") },
            || async {
                deleted.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(deleted.load(Ordering::SeqCst));
        assert!(repo.get_by_message(1, 10).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn in_place_database_failure_preserves_discord_message_and_votes() {
        let original = record();
        let repo = Repository {
            record: Mutex::new(original.clone()),
            fail: AtomicBool::new(true),
            fail_after_commit: AtomicBool::new(false),
        };
        let mut next = original.clone();
        next.status = SuggestionStatus::Approved;
        assert!(
            commit_in_place(&repo, &original, next, || async {
                panic!("database failure must not edit message or delete votes")
            })
            .await
            .is_err()
        );
        assert_eq!(
            repo.get_by_message(1, 3).await.unwrap().unwrap().status,
            SuggestionStatus::Pending
        );
    }

    #[tokio::test]
    async fn in_place_discord_failure_preserves_committed_review_for_refresh() {
        let original = record();
        let repo = Repository {
            record: Mutex::new(original.clone()),
            fail: AtomicBool::new(false),
            fail_after_commit: AtomicBool::new(false),
        };
        let mut next = original.clone();
        next.status = SuggestionStatus::Approved;
        let error = commit_in_place(&repo, &original, next, || async {
            anyhow::bail!("Discord unavailable")
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Review was saved"));
        assert_eq!(
            repo.get_by_message(1, 3).await.unwrap().unwrap().status,
            SuggestionStatus::Approved
        );
    }
}
