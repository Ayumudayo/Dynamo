use chrono::Utc;
use dynamo_domain_suggestion::{SuggestionRecord, SuggestionStatus, SuggestionStatusUpdate};
use dynamo_repositories::SuggestionsRepository;
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude::{ChannelId, CreateMessage, Member, Message, Permissions};
use std::sync::Arc;

use crate::{
    constants::MODULE_ID,
    render::{create_reviewed_embed, moderation_buttons, vote_stats},
    settings::SuggestionSettings,
};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn transition_suggestion(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    repo: Arc<dyn SuggestionsRepository>,
    mut record: SuggestionRecord,
    source_message: &Message,
    moderator: &Member,
    settings: &SuggestionSettings,
    next_status: SuggestionStatus,
    reason: Option<String>,
) -> Result<String, Error> {
    if record.status == next_status {
        return Ok(match next_status {
            SuggestionStatus::Approved => "Suggestion already approved.".to_string(),
            SuggestionStatus::Rejected => "Suggestion already rejected.".to_string(),
            _ => "Suggestion already updated.".to_string(),
        });
    }

    let stats = vote_stats(source_message);
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

    // These Discord changes intentionally precede persistence, matching the existing
    // outcome-unknown behavior if persistence fails after a successful Discord mutation.
    if let Some(target_channel_id) = target_channel_id {
        let sent = ChannelId::new(target_channel_id)
            .send_message(
                ctx,
                CreateMessage::new()
                    .embed(embed)
                    .components(buttons.clone()),
            )
            .await?;
        source_message.delete(ctx).await?;
        record.channel_id = sent.channel_id.get();
        record.message_id = sent.id.get();
    } else {
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
        source_message.delete_reactions(ctx).await?;
    }

    repo.save(record).await?;

    let deployment = data.persistence.deployment_settings_or_default().await?;
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
    mut record: SuggestionRecord,
    source_message: &Message,
    moderator: &Member,
    reason: Option<String>,
) -> Result<String, Error> {
    // Preserve the existing Discord-before-repository order for deletion as well.
    source_message.delete(ctx).await?;

    record.status = SuggestionStatus::Deleted;
    record.updated_at = Utc::now();
    record.status_updates.push(SuggestionStatusUpdate {
        user_id: moderator.user.id.get(),
        status: SuggestionStatus::Deleted,
        reason,
        timestamp: Utc::now(),
    });
    repo.save(record).await?;

    Ok("Suggestion deleted.".to_string())
}

pub(crate) fn has_moderation_permissions(member: &Member, settings: &SuggestionSettings) -> bool {
    if member
        .permissions
        .unwrap_or_else(Permissions::empty)
        .manage_guild()
    {
        return true;
    }

    member
        .roles
        .iter()
        .any(|role_id| settings.staff_role_ids.contains(&role_id.get()))
}
