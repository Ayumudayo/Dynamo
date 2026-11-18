use chrono::Utc;
use dynamo_domain_giveaway::{GiveawayRecord, GiveawayStatus};
use dynamo_repositories::GiveawaysRepository;
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude::{
    ChannelId, CreateMessage, EditMessage, Error as SerenityError, Http, HttpError, Nonce,
};
use rand::{seq::SliceRandom, thread_rng};
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU64,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::render::{build_embed, entry_button};
use crate::validation::parse_message_id;

const MAX_DISPLAYED_WINNERS: usize = 35;
type GiveawayLocks = Mutex<HashMap<(u64, u64), Weak<AsyncMutex<()>>>>;

pub(crate) fn schedule_debounced_sync(
    repo: Arc<dyn GiveawaysRepository>,
    http: Arc<Http>,
    guild_id: u64,
    message_id: u64,
) {
    static SCHEDULED: OnceLock<Mutex<HashSet<(u64, u64)>>> = OnceLock::new();
    {
        let mut set = SCHEDULED
            .get_or_init(Default::default)
            .lock()
            .expect("giveaway debounced sync registry poisoned");
        if !set.insert((guild_id, message_id)) {
            return;
        }
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if let Some(scheduled) = SCHEDULED.get() {
            scheduled
                .lock()
                .expect("giveaway debounced sync registry poisoned")
                .remove(&(guild_id, message_id));
        }
        let lock = giveaway_lock(guild_id, message_id);
        let _guard = lock.lock().await;
        if let Ok(Some(record)) = repo.get_by_message(guild_id, message_id).await
            && let Err(error) = deliver_saved_message(repo.as_ref(), http.as_ref(), record).await
        {
            tracing::warn!(
                guild_id,
                message_id,
                ?error,
                "debounced giveaway message refresh failed"
            );
        }
    });
}

fn is_permanent_discord_missing(error: &Error) -> bool {
    error.downcast_ref::<SerenityError>().is_some_and(|err| {
        matches!(
            err,
            SerenityError::Http(HttpError::UnsuccessfulRequest(resp))
                if matches!(resp.error.code, 10003 | 10008)
        )
    })
}

pub(crate) fn giveaway_lock(guild_id: u64, message_id: u64) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<GiveawayLocks> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("giveaway locks poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    let key = (guild_id, message_id);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

pub(crate) async fn save_if_current(
    repo: &dyn GiveawaysRepository,
    record: GiveawayRecord,
) -> Result<GiveawayRecord, Error> {
    repo.compare_and_save(record).await?.ok_or_else(|| {
        anyhow::anyhow!("This giveaway changed while processing the request. Please try again.")
    })
}

pub(crate) async fn toggle_entry(
    repo: &dyn GiveawaysRepository,
    mut record: GiveawayRecord,
    user_id: u64,
) -> Result<(GiveawayRecord, bool), Error> {
    if record.status != GiveawayStatus::Active || record.ends_at <= Utc::now() {
        anyhow::bail!("This giveaway is no longer accepting entries.");
    }
    let entered = if let Some(position) = record.entries.iter().position(|id| *id == user_id) {
        record.entries.remove(position);
        false
    } else {
        record.entries.push(user_id);
        true
    };
    record.updated_at = Utc::now();
    record.message_sync_pending = true;
    Ok((save_if_current(repo, record).await?, entered))
}

pub async fn poll_due_giveaways(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
) -> Result<(), Error> {
    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(());
    };
    let due = repo.list_due_before(Utc::now()).await?;
    let mut first_error = None;
    for record in due {
        let lock = giveaway_lock(record.guild_id, record.message_id);
        let _guard = lock.lock().await;
        let result = async {
            let Some(record) = repo
                .get_by_message(record.guild_id, record.message_id)
                .await?
            else {
                return Ok(());
            };
            let record = finalize_record(repo.as_ref(), record, true).await?;
            deliver_pending(repo.as_ref(), &DiscordDelivery(ctx.http.as_ref()), record).await?;
            Ok::<(), Error>(())
        }
        .await;
        if let Err(error) = result
            && first_error.is_none()
        {
            first_error =
                Some(error.context(format!("giveaway {} remains pending", record.message_id)));
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(crate) async fn load_giveaway(
    data: &AppState,
    guild_id: u64,
    message_id: &str,
) -> Result<Option<GiveawayRecord>, Error> {
    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(None);
    };
    repo.get_by_message(guild_id, parse_message_id(message_id)?)
        .await
}

pub(crate) async fn finalize_giveaway(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    record: &mut GiveawayRecord,
) -> Result<(), Error> {
    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(());
    };
    *record = finalize_record(repo.as_ref(), record.clone(), false).await?;
    if let Err(error) = deliver_pending(
        repo.as_ref(),
        &DiscordDelivery(ctx.http.as_ref()),
        record.clone(),
    )
    .await
    {
        tracing::warn!(%error, "giveaway result saved; Discord delivery will be retried");
    }
    Ok(())
}

async fn finalize_record(
    repo: &dyn GiveawaysRepository,
    mut record: GiveawayRecord,
    only_if_due: bool,
) -> Result<GiveawayRecord, Error> {
    for _ in 0..8 {
        if record.status == GiveawayStatus::Ended
            || (only_if_due
                && (record.status != GiveawayStatus::Active || record.ends_at > Utc::now()))
        {
            return Ok(record);
        }
        let mut next = record.clone();
        next.status = GiveawayStatus::Ended;
        next.paused_at = None;
        next.winner_ids = choose_winners(&next.entries, next.winner_count);
        next.message_sync_pending = true;
        next.announcement_pending = true;
        next.updated_at = Utc::now();
        if let Some(saved) = repo.compare_and_save(next).await? {
            return Ok(saved);
        }
        record = repo
            .get_by_message(record.guild_id, record.message_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Giveaway no longer exists"))?;
    }
    anyhow::bail!("Giveaway changed repeatedly; retry the operation")
}

#[async_trait::async_trait]
trait GiveawayDelivery: Send + Sync {
    async fn sync(&self, record: &GiveawayRecord) -> Result<(), Error>;
    async fn announce(&self, record: &GiveawayRecord) -> Result<(), Error>;
}

struct DiscordDelivery<'a>(&'a poise::serenity_prelude::Http);

#[async_trait::async_trait]
impl GiveawayDelivery for DiscordDelivery<'_> {
    async fn sync(&self, record: &GiveawayRecord) -> Result<(), Error> {
        if let Err(error) = sync_message(self.0, record).await {
            if is_permanent_discord_missing(&error) {
                tracing::warn!(
                    guild_id = record.guild_id,
                    message_id = record.message_id,
                    ?error,
                    "giveaway channel or message missing on Discord; clearing pending sync flag"
                );
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }
    async fn announce(&self, record: &GiveawayRecord) -> Result<(), Error> {
        let Some(channel_id) = NonZeroU64::new(record.channel_id).map(ChannelId::from) else {
            return Ok(());
        };
        let message = if record.winner_ids.is_empty() {
            format!(
                "Giveaway `{}` ended with no eligible winners.",
                record.prize
            )
        } else {
            format!(
                "Giveaway `{}` ended. Winners: {}",
                record.prize,
                format_winners(&record.winner_ids)
            )
        };
        if let Err(error) = channel_id
            .send_message(
                self.0,
                CreateMessage::new()
                    .content(message)
                    .nonce(Nonce::String(format!(
                        "{:x}:{}",
                        record.message_id, record.revision
                    )))
                    .enforce_nonce(true),
            )
            .await
        {
            let wrapped = Error::from(error);
            if is_permanent_discord_missing(&wrapped) {
                tracing::warn!(
                    guild_id = record.guild_id,
                    message_id = record.message_id,
                    ?wrapped,
                    "giveaway channel missing on Discord; clearing pending announcement flag"
                );
                return Ok(());
            }
            return Err(wrapped);
        }
        Ok(())
    }
}

pub(crate) async fn deliver_saved_message(
    repo: &dyn GiveawaysRepository,
    http: &poise::serenity_prelude::Http,
    record: GiveawayRecord,
) -> Result<(), Error> {
    deliver_pending(repo, &DiscordDelivery(http), record).await
}

async fn deliver_pending(
    repo: &dyn GiveawaysRepository,
    delivery: &dyn GiveawayDelivery,
    mut record: GiveawayRecord,
) -> Result<(), Error> {
    if record.message_sync_pending {
        delivery.sync(&record).await?;
        record.message_sync_pending = false;
        record = save_if_current(repo, record).await?;
    }
    if record.status == GiveawayStatus::Ended && record.announcement_pending {
        delivery.announce(&record).await?;
        record.announcement_pending = false;
        save_if_current(repo, record).await?;
    }
    Ok(())
}

pub(crate) async fn sync_message(
    http: &poise::serenity_prelude::Http,
    record: &GiveawayRecord,
) -> Result<(), Error> {
    let Some(channel_id) = NonZeroU64::new(record.channel_id).map(ChannelId::from) else {
        return Ok(());
    };
    channel_id
        .edit_message(
            http,
            record.message_id,
            EditMessage::new()
                .embed(build_embed(record))
                .components(vec![entry_button(record)]),
        )
        .await?;
    Ok(())
}

pub(crate) fn choose_winners(entries: &[u64], winner_count: u64) -> Vec<u64> {
    let mut pool = entries.to_vec();
    pool.sort_unstable();
    pool.dedup();
    pool.shuffle(&mut thread_rng());
    pool.truncate(winner_count as usize);
    pool
}

pub(crate) fn format_winners(winner_ids: &[u64]) -> String {
    if winner_ids.is_empty() {
        "No winners".to_string()
    } else if winner_ids.len() <= MAX_DISPLAYED_WINNERS {
        winner_ids
            .iter()
            .map(|winner_id| format!("<@{winner_id}>"))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        let shown = winner_ids
            .iter()
            .take(MAX_DISPLAYED_WINNERS)
            .map(|winner_id| format!("<@{winner_id}>"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{shown} ... and {} more",
            winner_ids.len() - MAX_DISPLAYED_WINNERS
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct MemoryRepo(AsyncMutex<GiveawayRecord>);

    #[async_trait::async_trait]
    impl GiveawaysRepository for MemoryRepo {
        async fn create(&self, record: GiveawayRecord) -> Result<GiveawayRecord, Error> {
            *self.0.lock().await = record.clone();
            Ok(record)
        }
        async fn get_by_message(&self, _: u64, _: u64) -> Result<Option<GiveawayRecord>, Error> {
            Ok(Some(self.0.lock().await.clone()))
        }
        async fn compare_and_save(
            &self,
            mut record: GiveawayRecord,
        ) -> Result<Option<GiveawayRecord>, Error> {
            let mut current = self.0.lock().await;
            if record.revision != current.revision {
                return Ok(None);
            }
            record.revision += 1;
            *current = record.clone();
            Ok(Some(record))
        }
        async fn list_by_guild(&self, _: u64) -> Result<Vec<GiveawayRecord>, Error> {
            Ok(vec![self.0.lock().await.clone()])
        }
        async fn list_due_before(
            &self,
            now: chrono::DateTime<Utc>,
        ) -> Result<Vec<GiveawayRecord>, Error> {
            let record = self.0.lock().await.clone();
            Ok(
                if (record.status == GiveawayStatus::Active && record.ends_at <= now)
                    || record.message_sync_pending
                    || record.announcement_pending
                {
                    vec![record]
                } else {
                    vec![]
                },
            )
        }
    }

    struct FailingDelivery {
        fail_sync: AtomicBool,
        fail_announce: AtomicBool,
        sync_count: AtomicUsize,
        announce_count: AtomicUsize,
        announced_winners: AsyncMutex<Vec<Vec<u64>>>,
    }

    #[async_trait::async_trait]
    impl GiveawayDelivery for FailingDelivery {
        async fn sync(&self, _: &GiveawayRecord) -> Result<(), Error> {
            self.sync_count.fetch_add(1, Ordering::SeqCst);
            if self.fail_sync.swap(false, Ordering::SeqCst) {
                anyhow::bail!("Discord edit failed");
            }
            Ok(())
        }
        async fn announce(&self, record: &GiveawayRecord) -> Result<(), Error> {
            self.announce_count.fetch_add(1, Ordering::SeqCst);
            if self.fail_announce.swap(false, Ordering::SeqCst) {
                anyhow::bail!("Discord send failed");
            }
            self.announced_winners
                .lock()
                .await
                .push(record.winner_ids.clone());
            Ok(())
        }
    }

    fn active_record(message_id: u64) -> GiveawayRecord {
        GiveawayRecord {
            guild_id: 1,
            message_id,
            ends_at: Utc::now() + chrono::Duration::hours(1),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn concurrent_entries_preserve_every_participant() {
        let repo = Arc::new(MemoryRepo(AsyncMutex::new(active_record(100))));
        let mut tasks = Vec::new();
        for user_id in 1..=24 {
            let repo = repo.clone();
            tasks.push(tokio::spawn(async move {
                let lock = giveaway_lock(1, 100);
                let _guard = lock.lock().await;
                let record = repo.get_by_message(1, 100).await.unwrap().unwrap();
                tokio::task::yield_now().await;
                toggle_entry(repo.as_ref(), record, user_id).await.unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let mut entries = repo.0.lock().await.entries.clone();
        entries.sort_unstable();
        assert_eq!(entries, (1..=24).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn stale_participant_cannot_restore_ended_state() {
        let repo = MemoryRepo(AsyncMutex::new(active_record(101)));
        let stale = repo.get_by_message(1, 101).await.unwrap().unwrap();
        let ended = finalize_record(&repo, stale.clone(), false).await.unwrap();
        assert!(toggle_entry(&repo, stale, 42).await.is_err());
        let current = repo.0.lock().await;
        assert_eq!(current.status, GiveawayStatus::Ended);
        assert_eq!(current.revision, ended.revision);
        assert!(current.entries.is_empty());
    }

    #[tokio::test]
    async fn racing_finalizers_return_the_same_committed_winners() {
        let mut record = active_record(105);
        record.entries = vec![1, 2, 3, 4, 5, 6];
        record.winner_count = 2;
        let repo = MemoryRepo(AsyncMutex::new(record.clone()));
        let (first, second) = tokio::join!(
            finalize_record(&repo, record.clone(), false),
            finalize_record(&repo, record, false),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.winner_ids, second.winner_ids);
        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 1);
        assert_eq!(repo.0.lock().await.winner_ids, first.winner_ids);
    }

    #[tokio::test]
    async fn failed_edit_and_announcement_retry_the_same_durable_winners() {
        let mut draft = active_record(102);
        draft.entries = vec![1, 2, 3];
        let repo = MemoryRepo(AsyncMutex::new(draft.clone()));
        let delivery = FailingDelivery {
            fail_sync: AtomicBool::new(true),
            fail_announce: AtomicBool::new(true),
            sync_count: AtomicUsize::new(0),
            announce_count: AtomicUsize::new(0),
            announced_winners: AsyncMutex::new(vec![]),
        };
        let ended = finalize_record(&repo, draft, false).await.unwrap();
        assert!(
            deliver_pending(&repo, &delivery, ended.clone())
                .await
                .is_err()
        );
        assert_eq!(repo.list_due_before(Utc::now()).await.unwrap().len(), 1);
        let restored = repo.get_by_message(1, 102).await.unwrap().unwrap();
        assert_eq!(restored.winner_ids, ended.winner_ids);
        assert!(deliver_pending(&repo, &delivery, restored).await.is_err());
        let restored = repo.get_by_message(1, 102).await.unwrap().unwrap();
        assert!(!restored.message_sync_pending);
        assert!(restored.announcement_pending);
        let restored = finalize_record(&repo, restored, true).await.unwrap();
        deliver_pending(&repo, &delivery, restored).await.unwrap();
        assert!(repo.list_due_before(Utc::now()).await.unwrap().is_empty());
        assert_eq!(delivery.sync_count.load(Ordering::SeqCst), 2);
        assert_eq!(delivery.announce_count.load(Ordering::SeqCst), 2);
        assert_eq!(
            *delivery.announced_winners.lock().await,
            vec![ended.winner_ids]
        );
    }

    #[tokio::test]
    async fn automatic_finalization_respects_a_new_pause_or_extended_schedule() {
        let mut paused = active_record(103);
        paused.status = GiveawayStatus::Paused;
        paused.ends_at = Utc::now() - chrono::Duration::minutes(1);
        let repo = MemoryRepo(AsyncMutex::new(paused.clone()));
        assert_eq!(
            finalize_record(&repo, paused, true).await.unwrap().status,
            GiveawayStatus::Paused
        );
        let future = active_record(104);
        assert_eq!(
            finalize_record(&repo, future, true).await.unwrap().status,
            GiveawayStatus::Active
        );
    }
    #[test]
    fn choose_winners_never_exceeds_deduped_pool() {
        assert!(choose_winners(&[1, 1, 2], 3).len() <= 2);
    }
}
