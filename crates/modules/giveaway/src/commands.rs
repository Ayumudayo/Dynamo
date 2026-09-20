use chrono::{Duration as ChronoDuration, Utc};
use dynamo_domain_giveaway::{GiveawayRecord, GiveawayStatus};
use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{ChannelId, CreateEmbed, CreateMessage, User};

use crate::constants::DEFAULT_BUTTON_LABEL;
use crate::lifecycle::{
    choose_winners, finalize_giveaway, format_winners, load_giveaway, sync_message,
};
use crate::render::{build_embed, entry_button};
use crate::settings::load_settings;
use crate::validation::parse_role_ids;

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
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_start(
    ctx: Context<'_>,
    #[description = "Duration like 30m, 1h, or 2d"] duration: String,
    #[description = "Prize shown in the giveaway message"] prize: String,
    #[description = "Number of winners"] winners: i32,
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
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    let target_channel = channel
        .map(|value| value.get())
        .or(settings.default_channel)
        .ok_or_else(|| anyhow::anyhow!("Giveaway channel not configured."))?;
    let winner_count = winners.max(1) as u64;
    let duration = humantime::parse_duration(&duration)
        .map_err(|error| anyhow::anyhow!("Invalid duration: {error}"))?;
    let allowed_role_ids = parse_role_ids(allowed_roles.as_deref())?;
    let start_at = Utc::now();
    let ends_at = start_at + ChronoDuration::from_std(duration)?;
    let host_user_id = host.unwrap_or_else(|| ctx.author().clone()).id.get();
    let button_label = if settings.button_label.trim().is_empty() {
        DEFAULT_BUTTON_LABEL.to_string()
    } else {
        settings.button_label.clone()
    };
    let draft = GiveawayRecord {
        guild_id: guild_id.get(),
        channel_id: target_channel,
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
    let message = ChannelId::new(target_channel)
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
    ctx.say(format!(
        "Giveaway started in <#{}> with message `{}`.",
        target_channel,
        message.id.get()
    ))
    .await?;
    Ok(())
}

/// List the giveaways currently tracked in this guild.
#[poise::command(
    slash_command,
    guild_only,
    rename = "list",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_list(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let mut giveaways = repo.list_by_guild(guild_id.get()).await?;
    giveaways.sort_by_key(|record| std::cmp::Reverse(record.created_at.timestamp()));
    if giveaways.is_empty() {
        ctx.say("There are no giveaways tracked in this guild.")
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
        poise::CreateReply::default().embed(
            CreateEmbed::new()
                .title("Tracked Giveaways")
                .description(description),
        ),
    )
    .await?;
    Ok(())
}

/// End an active giveaway immediately.
#[poise::command(
    slash_command,
    guild_only,
    rename = "end",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_end(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };
    if record.status == GiveawayStatus::Ended {
        ctx.say("That giveaway has already ended.").await?;
        return Ok(());
    }
    finalize_giveaway(ctx.serenity_context(), ctx.data(), &mut record).await?;
    ctx.say("The giveaway has been ended.").await?;
    Ok(())
}

/// Pause an active giveaway so no new entries can be accepted.
#[poise::command(
    slash_command,
    guild_only,
    rename = "pause",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_pause(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };
    if record.status != GiveawayStatus::Active {
        ctx.say("Only active giveaways can be paused.").await?;
        return Ok(());
    }
    record.status = GiveawayStatus::Paused;
    record.paused_at = Some(Utc::now());
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.serenity_context().http.as_ref(), &record).await?;
    ctx.say("The giveaway has been paused.").await?;
    Ok(())
}

/// Resume a paused giveaway and extend its end time by the paused duration.
#[poise::command(
    slash_command,
    guild_only,
    rename = "resume",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_resume(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };
    if record.status != GiveawayStatus::Paused {
        ctx.say("Only paused giveaways can be resumed.").await?;
        return Ok(());
    }
    let paused_at = record.paused_at.unwrap_or(record.updated_at);
    record.ends_at += Utc::now() - paused_at;
    record.status = GiveawayStatus::Active;
    record.paused_at = None;
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.serenity_context().http.as_ref(), &record).await?;
    ctx.say("The giveaway has been resumed.").await?;
    Ok(())
}

/// Pick a fresh set of winners for an already ended giveaway.
#[poise::command(
    slash_command,
    guild_only,
    rename = "reroll",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_reroll(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
    #[description = "Optional override winner count"] winners: Option<i32>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };
    if record.status != GiveawayStatus::Ended {
        ctx.say("Only ended giveaways can be rerolled.").await?;
        return Ok(());
    }
    if let Some(winner_count) = winners {
        record.winner_count = winner_count.max(1) as u64;
    }
    record.winner_ids = choose_winners(&record.entries, record.winner_count);
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.serenity_context().http.as_ref(), &record).await?;
    ctx.say(format!(
        "Giveaway rerolled. Winners: {}",
        format_winners(&record.winner_ids)
    ))
    .await?;
    Ok(())
}

/// Edit the schedule, prize, or winner count of an active giveaway.
#[poise::command(
    slash_command,
    guild_only,
    rename = "edit",
    required_permissions = "MANAGE_MESSAGES"
)]
async fn giveaway_edit(
    ctx: Context<'_>,
    #[description = "Giveaway message ID"] message_id: String,
    #[description = "Optional minutes to add"] add_minutes: Option<i64>,
    #[description = "Optional new prize"] new_prize: Option<String>,
    #[description = "Optional new winner count"] new_winners: Option<i32>,
) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(repo) = ctx.data().persistence.giveaways.clone() else {
        ctx.say("Giveaway repository is not configured.").await?;
        return Ok(());
    };
    let Some(mut record) = load_giveaway(ctx.data(), guild_id.get(), &message_id).await? else {
        ctx.say("Unable to find that giveaway.").await?;
        return Ok(());
    };
    if record.status == GiveawayStatus::Ended {
        ctx.say("Ended giveaways cannot be edited.").await?;
        return Ok(());
    }
    if let Some(minutes) = add_minutes {
        record.ends_at += ChronoDuration::minutes(minutes);
    }
    if let Some(prize) = new_prize {
        record.prize = prize;
    }
    if let Some(winners) = new_winners {
        record.winner_count = winners.max(1) as u64;
    }
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.serenity_context().http.as_ref(), &record).await?;
    ctx.say("Giveaway updated.").await?;
    Ok(())
}
