use chrono::Utc;
use dynamo_access::module_access_for_app;
use dynamo_domain_giveaway::GiveawayStatus;
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude::{
    ComponentInteraction, CreateInteractionResponse, CreateInteractionResponseMessage,
    EditInteractionResponse, Interaction,
};

use crate::constants::{GIVEAWAY_ENTER_BUTTON_ID, MODULE_ID};
use crate::lifecycle::sync_message;

pub async fn handle_interaction(
    ctx: &poise::serenity_prelude::Context,
    interaction: &Interaction,
    data: &AppState,
) -> Result<bool, Error> {
    match interaction {
        Interaction::Component(component)
            if component.data.custom_id == GIVEAWAY_ENTER_BUTTON_ID =>
        {
            handle_entry_interaction(ctx, component, data).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_entry_interaction(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
    data: &AppState,
) -> Result<(), Error> {
    let Some(guild_id) = component.guild_id else {
        return Ok(());
    };
    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("The giveaway module is currently disabled.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }
    let Some(repo) = data.persistence.giveaways.clone() else {
        return Ok(());
    };
    let Some(mut record) = repo
        .get_by_message(guild_id.get(), component.message.id.get())
        .await?
    else {
        return Ok(());
    };
    if record.status != GiveawayStatus::Active {
        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This giveaway is no longer accepting entries.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }
    if !record.allowed_role_ids.is_empty() {
        let member_roles = component
            .member
            .as_ref()
            .map(|member| member.roles.clone())
            .unwrap_or_default();
        let allowed = record
            .allowed_role_ids
            .iter()
            .any(|role_id| member_roles.contains(&poise::serenity_prelude::RoleId::new(*role_id)));
        if !allowed {
            component
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("You do not meet the role requirements for this giveaway.")
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }
    }
    component.defer_ephemeral(ctx).await?;
    let user_id = component.user.id.get();
    let response = if let Some(position) = record.entries.iter().position(|entry| *entry == user_id)
    {
        record.entries.remove(position);
        "You left the giveaway."
    } else {
        record.entries.push(user_id);
        "You entered the giveaway."
    };
    record.updated_at = Utc::now();
    let record = repo.save(record).await?;
    sync_message(ctx.http.as_ref(), &record).await?;
    component
        .edit_response(ctx, EditInteractionResponse::new().content(response))
        .await?;
    Ok(())
}
