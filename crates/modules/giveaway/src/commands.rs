use std::num::NonZeroU64;

use chrono::{Duration as ChronoDuration, Utc};
use dynamo_domain_giveaway::{GiveawayRecord, GiveawayStatus};
use dynamo_runtime_api::{Context, Error};
use poise::{
    CreateReply,
    serenity_prelude::{ChannelId, CreateEmbed, CreateMessage, User},
};

use crate::constants::DEFAULT_BUTTON_LABEL;
use crate::lifecycle::{
    choose_winners, deliver_saved_message, finalize_giveaway, format_winners, giveaway_lock,
    load_giveaway, save_if_current,
};
use crate::render::{build_embed, entry_button};
use crate::settings::load_settings;
use crate::validation::{parse_message_id, parse_role_ids};

const MIN_GIVEAWAY_SECONDS: u64 = 10;
const MAX_GIVEAWAY_SECONDS: u64 = 90 * 24 * 60 * 60;
const MAX_GIVEAWAY_WINNERS: i32 = 50;

/// Manage giveaway lifecycle actions for this guild.
#[poise::command(
    slash_command,
    guild_only,
    category = "Giveaway",
    subcommands(
        "giveaway_start",
        "giveaway_list",
        "giveaway_end",
        "giveaway_pause",
        "giveaway_resume",
        "giveaway_reroll",
        "giveaway_edit"
    ),
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES"
)]
pub(crate) async fn giveaway(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Start a new giveaway message in the configured channel.
#[poise::command(
    slash_command,
    guild_only,
    rename = "start",
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_start(
    ctx: Context<'_>,
    #[description = "Duration like 30m, 1h, or 2d"] duration: String,
    #[description = "Prize shown in the giveaway message"] prize: String,
    #[description = "Number of winners (1-50)"] winners: i32,
    #[description = "Optional giveaway channel; omit to use module settings"] channel: Option<
        ChannelId,
    >,
    #[description = "Optional host user override"] host: Option<User>,
    #[description = "Optional comma-separated role IDs allowed to enter"] allowed_roles: Option<
        String,
    >,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.send(
            CreateReply::default()
                .content("Giveaway repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    let target_channel = channel
        .map(|value| value.get())
        .or(settings.default_channel)
        .and_then(NonZeroU64::new)
        .map(ChannelId::from)
        .ok_or_else(|| anyhow::anyhow!("Giveaway channel not configured."))?;
    anyhow::ensure!(
        (1..=MAX_GIVEAWAY_WINNERS).contains(&winners),
        "Winner count must be between 1 and {MAX_GIVEAWAY_WINNERS}."
    );
    let winner_count = winners as u64;
    let duration = humantime::parse_duration(&duration)
        .map_err(|error| anyhow::anyhow!("Invalid duration: {error}"))?;
    anyhow::ensure!(
        (MIN_GIVEAWAY_SECONDS..=MAX_GIVEAWAY_SECONDS).contains(&duration.as_secs()),
        "Duration must be between 10s and 90d."
    );
    let allowed_role_ids = parse_role_ids(allowed_roles.as_deref())?;
    let start_at = Utc::now();
    let ends_at = start_at + ChronoDuration::from_std(duration)?;
    let host_user_id = host.unwrap_or_else(|| ctx.author().clone()).id.get();
    let button_label = if settings.button_label.trim().is_empty() {
        DEFAULT_BUTTON_LABEL.to_string()
    } else {
        settings.button_label.clone()
    };
    ctx.defer_ephemeral().await?;
    let draft = GiveawayRecord {
        revision: 0,
        message_sync_pending: false,
        announcement_pending: false,
        guild_id: guild_id.get(),
        channel_id: target_channel.get(),
        message_id: 0,
        prize: prize.clone(),
        winner_count,
        host_user_id,
        allowed_role_ids,
        entries: Vec::new(),
        winner_ids: Vec::new(),
        status: GiveawayStatus::Active,
        started_at: start_at,
        ends_at,
        paused_at: None,
        button_label: button_label.clone(),
        created_at: start_at,
        updated_at: start_at,
    };
    let message = target_channel
        .send_message(
            ctx.serenity_context(),
            CreateMessage::new()
                .embed(build_embed(&draft))
                .components(vec![entry_button(&draft)]),
        )
        .await?;
    let mut record = draft;
    record.message_id = message.id.get();
    repo.create(record).await?;
    ctx.send(
        CreateReply::default()
            .content(format!(
                "Giveaway started in <#{}> with message `{}`.",
                target_channel.get(),
                message.id.get()
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// List the giveaways currently tracked in this guild.
#[poise::command(
    slash_command,
    guild_only,
    rename = "list",
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_list(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.send(
            CreateReply::default()
                .content("Giveaway repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    let mut giveaways = repo.list_by_guild(guild_id.get()).await?;
    giveaways.sort_by_key(|record| std::cmp::Reverse(record.created_at.timestamp()));
    if giveaways.is_empty() {
        ctx.send(
            CreateReply::default()
                .content("There are no giveaways tracked in this guild.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let description = giveaways
        .into_iter()
        .take(10)
        .map(|record| {
            format!(
                "`{}` {} in <#{}> | entries: {} | status: {:?}",
                record.message_id,
                record.prize,
                record.channel_id,
                record.entries.len(),
                record.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    ctx.send(
        CreateReply::default()
            .embed(
                CreateEmbed::new()
                    .title("Tracked Giveaways")
                    .description(description),
            )
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// End an active giveaway immediately.
#[poise::command(
    slash_command,
    guild_only,
    rename = "end",
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_end(
    ctx: Context<'_>,
    #[description = "Giveaway message ID or URL"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let lock = giveaway_lock(guild_id.get(), parse_message_id(&message_id)?);
    let _guard = lock.lock().await;
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.send(
            CreateReply::default()
                .content("Unable to find that giveaway.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if record.status == GiveawayStatus::Ended {
        ctx.send(
            CreateReply::default()
                .content("That giveaway has already ended.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    finalize_giveaway(ctx.serenity_context(), ctx.data(), &mut record).await?;
    ctx.send(
        CreateReply::default()
            .content("The giveaway has been ended.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Pause an active giveaway so no new entries can be accepted.
#[poise::command(
    slash_command,
    guild_only,
    rename = "pause",
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_pause(
    ctx: Context<'_>,
    #[description = "Giveaway message ID or URL"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.send(
            CreateReply::default()
                .content("Giveaway repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let lock = giveaway_lock(guild_id.get(), parse_message_id(&message_id)?);
    let _guard = lock.lock().await;
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.send(
            CreateReply::default()
                .content("Unable to find that giveaway.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if record.status != GiveawayStatus::Active {
        ctx.send(
            CreateReply::default()
                .content("Only active giveaways can be paused.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    record.status = GiveawayStatus::Paused;
    record.paused_at = Some(Utc::now());
    record.updated_at = Utc::now();
    record.message_sync_pending = true;
    let record = save_if_current(repo.as_ref(), record).await?;
    if let Err(error) = deliver_saved_message(
        repo.as_ref(),
        ctx.serenity_context().http.as_ref(),
        record.clone(),
    )
    .await
    {
        tracing::warn!(%error, "giveaway update saved; Discord delivery will be retried");
    }
    ctx.send(
        CreateReply::default()
            .content("The giveaway has been paused.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Resume a paused giveaway and extend its end time by the paused duration.
#[poise::command(
    slash_command,
    guild_only,
    rename = "resume",
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_resume(
    ctx: Context<'_>,
    #[description = "Giveaway message ID or URL"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.send(
            CreateReply::default()
                .content("Giveaway repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let lock = giveaway_lock(guild_id.get(), parse_message_id(&message_id)?);
    let _guard = lock.lock().await;
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.send(
            CreateReply::default()
                .content("Unable to find that giveaway.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if record.status != GiveawayStatus::Paused {
        ctx.send(
            CreateReply::default()
                .content("Only paused giveaways can be resumed.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let paused_at = record.paused_at.unwrap_or(record.updated_at);
    record.ends_at += Utc::now() - paused_at;
    record.status = GiveawayStatus::Active;
    record.paused_at = None;
    record.updated_at = Utc::now();
    record.message_sync_pending = true;
    let record = save_if_current(repo.as_ref(), record).await?;
    if let Err(error) = deliver_saved_message(
        repo.as_ref(),
        ctx.serenity_context().http.as_ref(),
        record.clone(),
    )
    .await
    {
        tracing::warn!(%error, "giveaway update saved; Discord delivery will be retried");
    }
    ctx.send(
        CreateReply::default()
            .content("The giveaway has been resumed.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Pick a fresh set of winners for an already ended giveaway.
#[poise::command(
    slash_command,
    guild_only,
    rename = "reroll",
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_reroll(
    ctx: Context<'_>,
    #[description = "Giveaway message ID or URL"] message_id: String,
    #[description = "Optional override winner count (1-50)"] winners: Option<i32>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.send(
            CreateReply::default()
                .content("Giveaway repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if let Some(winner_count) = winners {
        anyhow::ensure!(
            (1..=MAX_GIVEAWAY_WINNERS).contains(&winner_count),
            "Winner count must be between 1 and {MAX_GIVEAWAY_WINNERS}."
        );
    }
    ctx.defer_ephemeral().await?;
    let lock = giveaway_lock(guild_id.get(), parse_message_id(&message_id)?);
    let _guard = lock.lock().await;
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.send(
            CreateReply::default()
                .content("Unable to find that giveaway.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if record.status != GiveawayStatus::Ended {
        ctx.send(
            CreateReply::default()
                .content("Only ended giveaways can be rerolled.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    if record.announcement_pending {
        ctx.send(
            CreateReply::default()
                .content(
                    "The previous winner announcement is still pending. Retry after delivery completes.",
                )
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let remaining_pool = record
        .entries
        .iter()
        .copied()
        .filter(|entry| !record.winner_ids.contains(entry))
        .collect::<Vec<_>>();
    if remaining_pool.is_empty() {
        ctx.send(
            CreateReply::default()
                .content("No other eligible entrants remain to reroll.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    if let Some(winner_count) = winners {
        record.winner_count = winner_count as u64;
    }
    record.winner_ids = choose_winners(&remaining_pool, record.winner_count);
    record.updated_at = Utc::now();
    record.message_sync_pending = true;
    record.announcement_pending = true;
    let record = save_if_current(repo.as_ref(), record).await?;
    if let Err(error) = deliver_saved_message(
        repo.as_ref(),
        ctx.serenity_context().http.as_ref(),
        record.clone(),
    )
    .await
    {
        tracing::warn!(%error, "giveaway update saved; Discord delivery will be retried");
    }
    ctx.send(
        CreateReply::default()
            .content(format!(
                "Giveaway rerolled. Winners: {}",
                format_winners(&record.winner_ids)
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Edit the schedule, prize, or winner count of an active giveaway.
#[poise::command(
    slash_command,
    guild_only,
    rename = "edit",
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_edit(
    ctx: Context<'_>,
    #[description = "Giveaway message ID or URL"] message_id: String,
    #[description = "Optional minutes to add"] add_minutes: Option<i64>,
    #[description = "Optional new prize"] new_prize: Option<String>,
    #[description = "Optional new winner count (1-50)"] new_winners: Option<i32>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.send(
            CreateReply::default()
                .content("Giveaway repository is not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if let Some(winner_count) = new_winners {
        anyhow::ensure!(
            (1..=MAX_GIVEAWAY_WINNERS).contains(&winner_count),
            "Winner count must be between 1 and {MAX_GIVEAWAY_WINNERS}."
        );
    }
    ctx.defer_ephemeral().await?;
    let lock = giveaway_lock(guild_id.get(), parse_message_id(&message_id)?);
    let _guard = lock.lock().await;
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.send(
            CreateReply::default()
                .content("Unable to find that giveaway.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };
    if record.status == GiveawayStatus::Ended {
        ctx.send(
            CreateReply::default()
                .content("Ended giveaways cannot be edited.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    if let Some(minutes) = add_minutes {
        record.ends_at += ChronoDuration::minutes(minutes);
    }
    if let Some(prize) = new_prize {
        record.prize = prize;
    }
    if let Some(winners) = new_winners {
        record.winner_count = winners as u64;
    }
    record.updated_at = Utc::now();
    record.message_sync_pending = true;
    let record = save_if_current(repo.as_ref(), record).await?;
    if let Err(error) = deliver_saved_message(
        repo.as_ref(),
        ctx.serenity_context().http.as_ref(),
        record.clone(),
    )
    .await
    {
        tracing::warn!(%error, "giveaway update saved; Discord delivery will be retried");
    }
    ctx.send(
        CreateReply::default()
            .content("Giveaway updated.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}
