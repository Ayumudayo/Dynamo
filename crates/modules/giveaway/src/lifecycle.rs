use chrono::Utc;
use dynamo_domain_giveaway::{GiveawayRecord, GiveawayStatus};
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude::{ChannelId, CreateMessage, EditMessage};
use rand::{seq::SliceRandom, thread_rng};

use crate::render::{build_embed, entry_button};
use crate::validation::parse_message_id;

pub async fn poll_due_giveaways(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
) -> Result<(), Error> {
    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(());
    };
    let due = repo.list_due_before(Utc::now()).await?;
    for mut record in due {
        finalize_giveaway(ctx, data, &mut record).await?;
    }
    Ok(())
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
    if record.status == GiveawayStatus::Ended {
        return Ok(());
    }
    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(());
    };
    record.status = GiveawayStatus::Ended;
    record.paused_at = None;
    record.winner_ids = choose_winners(&record.entries, record.winner_count);
    record.updated_at = Utc::now();
    let record = repo.save(record.clone()).await?;
    sync_message(ctx.http.as_ref(), &record).await?;
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
    let _ = ChannelId::new(record.channel_id)
        .send_message(ctx, CreateMessage::new().content(message))
        .await;
    Ok(())
}

pub(crate) async fn sync_message(
    http: &poise::serenity_prelude::Http,
    record: &GiveawayRecord,
) -> Result<(), Error> {
    ChannelId::new(record.channel_id)
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
    } else {
        winner_ids
            .iter()
            .map(|winner_id| format!("<@{}>", winner_id))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::choose_winners;
    #[test]
    fn choose_winners_never_exceeds_deduped_pool() {
        assert!(choose_winners(&[1, 1, 2], 3).len() <= 2);
    }
}
