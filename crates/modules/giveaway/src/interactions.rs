use chrono::Utc;
use dynamo_access::module_access_for_app;
use dynamo_domain_giveaway::GiveawayStatus;
use dynamo_runtime_api::{AppState, Error};
use poise::serenity_prelude::{ComponentInteraction, EditInteractionResponse, Interaction};

use crate::constants::{GIVEAWAY_ENTER_BUTTON_ID, MODULE_ID};
use crate::lifecycle::{deliver_saved_message, giveaway_lock, toggle_entry};

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
    component.defer_ephemeral(ctx).await?;
    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        component
            .edit_response(
                ctx,
                EditInteractionResponse::new()
                    .content("The giveaway module is currently disabled."),
            )
            .await?;
        return Ok(());
    }
    let Some(repo) = data.persistence.giveaways.clone() else {
        component
            .edit_response(
                ctx,
                EditInteractionResponse::new().content("Giveaway storage is unavailable."),
            )
            .await?;
        return Ok(());
    };
    let lock = giveaway_lock(guild_id.get(), component.message.id.get());
    let _guard = lock.lock().await;
    let Some(record) = repo
        .get_by_message(guild_id.get(), component.message.id.get())
        .await?
    else {
        component
            .edit_response(
                ctx,
                EditInteractionResponse::new().content("Giveaway not found."),
            )
            .await?;
        return Ok(());
    };
    if record.status != GiveawayStatus::Active || record.ends_at <= Utc::now() {
        component
            .edit_response(
                ctx,
                EditInteractionResponse::new()
                    .content("This giveaway is no longer accepting entries."),
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
                .edit_response(
                    ctx,
                    EditInteractionResponse::new()
                        .content("You do not meet the role requirements for this giveaway."),
                )
                .await?;
            return Ok(());
        }
    }
    let user_id = component.user.id.get();
    let (record, entered) = toggle_entry(repo.as_ref(), record, user_id).await?;
    let response = if entered {
        "You entered the giveaway."
    } else {
        "You left the giveaway."
    };
    // The entry is durable even if Discord temporarily cannot refresh the message.
    if let Err(error) = deliver_saved_message(repo.as_ref(), ctx.http.as_ref(), record).await {
        tracing::warn!(%error, "giveaway entry saved; message refresh will be retried");
    }
    component
        .edit_response(ctx, EditInteractionResponse::new().content(response))
        .await?;
    Ok(())
}
