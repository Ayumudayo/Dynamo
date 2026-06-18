use crate::{
    constants::{MAX_MANUAL_REFRESHES, STOCK_REFRESH_BUTTON_ID},
    state::{
        edit_message, fetch_response_for_kind, initialize_session_loop, session_for_message,
        total_updates,
    },
};
use dynamo_runtime_api::Error;
use poise::serenity_prelude::{
    ComponentInteraction, CreateInteractionResponse, CreateInteractionResponseMessage,
    EditInteractionResponse, Interaction,
};

pub async fn handle(
    ctx: &poise::serenity_prelude::Context,
    interaction: &Interaction,
) -> Result<bool, Error> {
    let Interaction::Component(component) = interaction else {
        return Ok(false);
    };

    if component.data.custom_id != STOCK_REFRESH_BUTTON_ID {
        return Ok(false);
    }

    handle_refresh_button(ctx, component).await?;
    Ok(true)
}

async fn handle_refresh_button(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
) -> Result<(), Error> {
    let message_id = component.message.id.get();
    let session = session_for_message(message_id).await;

    let Some(session) = session else {
        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This refresh session has expired. Please run `/stock` or `/etf` again.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    };

    {
        let mut state = session.lock().await;
        if state.active {
            component
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("The default refresh loop is still running, so this button is not available yet.")
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }

        if state.manual_restart_in_progress {
            component
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(
                                "A refresh restart is already being prepared for this message.",
                            )
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }

        if state.manual_refresh_count >= MAX_MANUAL_REFRESHES {
            component
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(format!(
                                "You can manually restart this refresh loop up to {} times.",
                                MAX_MANUAL_REFRESHES
                            ))
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }

        state.manual_restart_in_progress = true;
    }

    component.defer_ephemeral(ctx).await?;

    let (kind, service) = {
        let state = session.lock().await;
        (state.kind.clone(), state.service.clone())
    };
    let response = fetch_response_for_kind(service.as_ref(), &kind, 0, total_updates()).await?;

    let Some(response) = response else {
        {
            let mut state = session.lock().await;
            state.manual_restart_in_progress = false;
            state.last_stop_reason = Some("fetch_failed");
            state.active = false;
        }

        component
            .edit_response(
                ctx,
                EditInteractionResponse::new()
                    .content("Failed to refresh quote data. Please try again later."),
            )
            .await?;
        return Ok(());
    };

    edit_message(
        &ctx.http,
        component.channel_id,
        message_id,
        response.embed.clone(),
    )
    .await?;

    {
        let mut state = session.lock().await;
        state.manual_restart_in_progress = false;
        state.manual_refresh_count += 1;
    }

    initialize_session_loop(
        ctx.http.clone(),
        component.channel_id,
        message_id,
        session,
        response.stop_reason,
    )
    .await;

    component
        .edit_response(
            ctx,
            EditInteractionResponse::new().content("Quote refresh updated."),
        )
        .await?;

    Ok(())
}
