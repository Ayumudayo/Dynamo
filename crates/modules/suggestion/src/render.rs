use dynamo_domain_suggestion::{SuggestionRecord, SuggestionStats, SuggestionStatus};
use poise::serenity_prelude::{
    CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter, Message, ReactionType,
};

use crate::constants::{
    APPROVE_BUTTON_ID, APPROVED_EMBED_COLOR, DEFAULT_EMBED_COLOR, DELETE_BUTTON_ID, DOWNVOTE_EMOJI,
    REJECT_BUTTON_ID, REJECTED_EMBED_COLOR, UPVOTE_EMOJI,
};

pub(crate) fn moderation_buttons(status: SuggestionStatus) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(APPROVE_BUTTON_ID)
            .label("Approve")
            .style(poise::serenity_prelude::ButtonStyle::Success)
            .disabled(status == SuggestionStatus::Approved),
        CreateButton::new(REJECT_BUTTON_ID)
            .label("Reject")
            .style(poise::serenity_prelude::ButtonStyle::Danger)
            .disabled(status == SuggestionStatus::Rejected),
        CreateButton::new(DELETE_BUTTON_ID)
            .label("Delete")
            .style(poise::serenity_prelude::ButtonStyle::Secondary),
    ])]
}

pub(crate) fn create_pending_embed(
    suggestion: &str,
    user_id: u64,
    username: &str,
    avatar_url: String,
) -> CreateEmbed {
    CreateEmbed::new()
        .title("New Suggestion")
        .description(suggestion)
        .color(DEFAULT_EMBED_COLOR)
        .thumbnail(avatar_url)
        .field("Submitter", format!("{username} [<@{user_id}>]"), false)
        .footer(CreateEmbedFooter::new("Pending review"))
        .timestamp(poise::serenity_prelude::Timestamp::now())
}

pub(crate) fn create_reviewed_embed(
    record: &SuggestionRecord,
    moderator_name: &str,
    moderator_avatar: String,
    reason: Option<&str>,
) -> CreateEmbed {
    let status_name = match record.status {
        SuggestionStatus::Approved => "Suggestion Approved",
        SuggestionStatus::Rejected => "Suggestion Rejected",
        SuggestionStatus::Deleted => "Suggestion Deleted",
        SuggestionStatus::Pending => "Suggestion Pending",
    };

    let mut embed = CreateEmbed::new()
        .title(status_name)
        .description(&record.suggestion)
        .color(status_color(record.status))
        .thumbnail(moderator_avatar)
        .field("Submitter", format!("<@{}>", record.user_id), false)
        .field("Stats", vote_message(&record.stats), false)
        .footer(CreateEmbedFooter::new(format!(
            "{} by {}",
            match record.status {
                SuggestionStatus::Approved => "Approved",
                SuggestionStatus::Rejected => "Rejected",
                SuggestionStatus::Deleted => "Deleted",
                SuggestionStatus::Pending => "Updated",
            },
            moderator_name
        )))
        .timestamp(poise::serenity_prelude::Timestamp::now());

    if let Some(reason) = reason {
        embed = embed.field("Reason", format!("```{reason}```"), false);
    }

    embed
}

fn status_color(status: SuggestionStatus) -> u32 {
    match status {
        SuggestionStatus::Approved => APPROVED_EMBED_COLOR,
        SuggestionStatus::Rejected | SuggestionStatus::Deleted => REJECTED_EMBED_COLOR,
        SuggestionStatus::Pending => DEFAULT_EMBED_COLOR,
    }
}

pub(crate) fn vote_stats(message: &Message) -> SuggestionStats {
    let upvotes = reaction_count(message, UPVOTE_EMOJI);
    let downvotes = reaction_count(message, DOWNVOTE_EMOJI);
    SuggestionStats { upvotes, downvotes }
}

fn reaction_count(message: &Message, emoji: &str) -> u64 {
    message
        .reactions
        .iter()
        .find(|reaction| matches!(&reaction.reaction_type, ReactionType::Unicode(value) if value == emoji))
        .map(|reaction| reaction.count.saturating_sub(1))
        .unwrap_or(0)
}

pub(crate) fn vote_message(stats: &SuggestionStats) -> String {
    let total = stats.upvotes + stats.downvotes;
    if total == 0 {
        return "_Upvotes: NA_\n_Downvotes: NA_".to_string();
    }

    let upvote_percent = ((stats.upvotes as f64 / total as f64) * 100.0).round() as u64;
    let downvote_percent = ((stats.downvotes as f64 / total as f64) * 100.0).round() as u64;
    format!(
        "_Upvotes: {} [{}%]_\n_Downvotes: {} [{}%]_",
        stats.upvotes, upvote_percent, stats.downvotes, downvote_percent
    )
}
