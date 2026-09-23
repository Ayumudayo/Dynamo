use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude::{
    ActionRowComponent, ComponentInteraction, CreateActionRow, CreateInputText,
    CreateInteractionResponse, CreateModal, InputTextStyle, Interaction, ModalInteraction,
};

use crate::{
    constants::{
        APPROVE_BUTTON_ID, APPROVE_MODAL_ID, DELETE_BUTTON_ID, DELETE_MODAL_ID, REASON_INPUT_ID,
        REJECT_BUTTON_ID, REJECT_MODAL_ID,
    },
    settings::load_settings,
    workflow::{delete_suggestion, has_moderation_permissions, transition_suggestion},
};

pub async fn handle(
    ctx: &poise::serenity_prelude::Context,
    interaction: &Interaction,
    data: &AppState,
) -> Result<bool, Error> {
    match interaction {
        Interaction::Component(component) if is_suggestion_button(&component.data.custom_id) => {
            handle_button_interaction(ctx, component).await?;
            Ok(true)
        }
        Interaction::Modal(modal) if is_suggestion_modal(&modal.data.custom_id) => {
            handle_modal_interaction(ctx, modal, data).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn is_suggestion_button(custom_id: &str) -> bool {
    matches!(
        custom_id,
        APPROVE_BUTTON_ID | REJECT_BUTTON_ID | DELETE_BUTTON_ID
    )
}

fn is_suggestion_modal(custom_id: &str) -> bool {
    matches!(
        custom_id,
        APPROVE_MODAL_ID | REJECT_MODAL_ID | DELETE_MODAL_ID
    )
}

async fn handle_button_interaction(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
) -> Result<(), Error> {
    let (modal_id, title) = match component.data.custom_id.as_str() {
        APPROVE_BUTTON_ID => (APPROVE_MODAL_ID, "Approve Suggestion"),
        REJECT_BUTTON_ID => (REJECT_MODAL_ID, "Reject Suggestion"),
        DELETE_BUTTON_ID => (DELETE_MODAL_ID, "Delete Suggestion"),
        _ => return Ok(()),
    };

    component
        .create_response(
            ctx,
            CreateInteractionResponse::Modal(CreateModal::new(modal_id, title).components(vec![
                CreateActionRow::InputText(
                    CreateInputText::new(InputTextStyle::Paragraph, "Reason", REASON_INPUT_ID)
                        .placeholder("Optional reason")
                        .required(false),
                ),
            ])),
        )
        .await?;

    Ok(())
}

async fn handle_modal_interaction(
    ctx: &poise::serenity_prelude::Context,
    modal: &ModalInteraction,
    data: &AppState,
) -> Result<(), Error> {
    modal.defer_ephemeral(ctx).await?;

    let Some(member) = modal.member.as_ref() else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("This action can only be used in a guild."),
            )
            .await?;
        return Ok(());
    };

    let Some(guild_id) = modal.guild_id else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("This action can only be used in a guild."),
            )
            .await?;
        return Ok(());
    };

    let Some(source_message) = modal.message.as_deref() else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("The original suggestion message is no longer available."),
            )
            .await?;
        return Ok(());
    };

    let settings = load_settings(data, Some(guild_id.get())).await?;
    if !has_moderation_permissions(member, &settings) {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("You don't have permission to moderate suggestions."),
            )
            .await?;
        return Ok(());
    }

    let Some(repo) = data.persistence.suggestions.clone() else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("The suggestion repository is not available in this deployment."),
            )
            .await?;
        return Ok(());
    };

    let Some(record) = repo
        .get_by_message(guild_id.get(), source_message.id.get())
        .await?
    else {
        modal
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("Suggestion not found."),
            )
            .await?;
        return Ok(());
    };

    let reason = modal_reason(modal).map(|value| value.trim().to_string());
    let reason = reason.filter(|value| !value.is_empty());
    let response = match modal.data.custom_id.as_str() {
        APPROVE_MODAL_ID => {
            transition_suggestion(
                ctx,
                data,
                repo,
                record,
                source_message,
                member,
                &settings,
                dynamo_domain_suggestion::SuggestionStatus::Approved,
                reason,
            )
            .await?
        }
        REJECT_MODAL_ID => {
            transition_suggestion(
                ctx,
                data,
                repo,
                record,
                source_message,
                member,
                &settings,
                dynamo_domain_suggestion::SuggestionStatus::Rejected,
                reason,
            )
            .await?
        }
        DELETE_MODAL_ID => {
            delete_suggestion(ctx, repo, record, source_message, member, reason).await?
        }
        _ => "Not a valid moderation action.".to_string(),
    };

    modal
        .edit_response(
            ctx,
            poise::serenity_prelude::EditInteractionResponse::new().content(response),
        )
        .await?;
    Ok(())
}

fn modal_reason(modal: &ModalInteraction) -> Option<String> {
    modal
        .data
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|component| match component {
            ActionRowComponent::InputText(input) if input.custom_id == REASON_INPUT_ID => {
                input.value.clone()
            }
            _ => None,
        })
}
