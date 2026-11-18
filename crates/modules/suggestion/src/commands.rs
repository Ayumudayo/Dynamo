use std::num::NonZeroU64;

use chrono::Utc;
use dynamo_domain_suggestion::{SuggestionRecord, SuggestionStats, SuggestionStatus};
use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{ChannelId, CreateMessage, ReactionType};

use crate::{
    constants::{DOWNVOTE_EMOJI, UPVOTE_EMOJI},
    render::{create_pending_embed, moderation_buttons},
    settings::load_settings,
};

const MAX_SUGGESTION_CHARS: usize = 3500;

/// Submit a new suggestion to the guild suggestion board.
#[poise::command(slash_command, guild_only, category = "Suggestion")]
pub(crate) async fn suggest(
    ctx: Context<'_>,
    #[description = "Suggestion text"] suggestion: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let trimmed = suggestion.trim();
    if trimmed.is_empty() {
        ctx.send(
            poise::CreateReply::default()
                .content("Suggestion text cannot be empty.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    if trimmed.chars().count() > MAX_SUGGESTION_CHARS {
        ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "Suggestion text must be {MAX_SUGGESTION_CHARS} characters or fewer."
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let suggestion = trimmed.to_string();

    let Some(repo) = ctx.data().persistence.suggestions.clone() else {
        ctx.send(
            poise::CreateReply::default()
                .content("The suggestion repository is not available in this deployment.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    let settings = load_settings(ctx.data(), ctx.guild_id().map(|id| id.get())).await?;
    let Some(channel_id) = settings
        .channel_id
        .and_then(NonZeroU64::new)
        .map(ChannelId::from)
    else {
        ctx.send(
            poise::CreateReply::default()
                .content("Suggestion channel not configured.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let author = ctx.author();
    let embed = create_pending_embed(&suggestion, author.id.get(), &author.name, author.face());

    let message = channel_id
        .send_message(
            ctx.serenity_context(),
            CreateMessage::new()
                .embed(embed)
                .components(moderation_buttons(SuggestionStatus::Pending)),
        )
        .await?;

    message
        .react(
            ctx.serenity_context(),
            ReactionType::Unicode(UPVOTE_EMOJI.to_string()),
        )
        .await?;
    message
        .react(
            ctx.serenity_context(),
            ReactionType::Unicode(DOWNVOTE_EMOJI.to_string()),
        )
        .await?;

    let now = Utc::now();
    repo.create(SuggestionRecord {
        guild_id: guild_id.get(),
        channel_id: message.channel_id.get(),
        message_id: message.id.get(),
        user_id: author.id.get(),
        suggestion,
        status: SuggestionStatus::Pending,
        stats: SuggestionStats::default(),
        status_updates: Vec::new(),
        created_at: now,
        updated_at: now,
    })
    .await?;

    ctx.send(
        poise::CreateReply::default()
            .content("Your suggestion has been submitted.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}
