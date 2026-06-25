use crate::{
    constants::{MAX_MANUAL_REFRESHES, STOCK_REFRESH_BUTTON_ID},
    state::{
        ManualRestartStart, edit_message, edit_refresh_components, fetch_response_for_session,
        initialize_session_loop, session_for_message, try_begin_manual_restart,
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

    let start = {
        let mut state = session.lock().await;
        try_begin_manual_restart(&mut state)
    };

    match start {
        ManualRestartStart::Started => {}
        ManualRestartStart::ActiveLoop => {
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
        ManualRestartStart::AlreadyInProgress => {
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
        ManualRestartStart::LimitReached => {
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
    }

    component.defer_ephemeral(ctx).await?;

    if let Err(error) =
        edit_refresh_components(&ctx.http, component.channel_id, message_id, true).await
    {
        let mut state = session.lock().await;
        state.manual_restart_in_progress = false;
        return Err(error);
    }

    let response = match fetch_response_for_session(&session, 0).await {
        Ok(value) => value,
        Err(error) => {
            {
                let mut state = session.lock().await;
                state.manual_restart_in_progress = false;
            }
            let _ =
                edit_refresh_components(&ctx.http, component.channel_id, message_id, false).await;
            return Err(error);
        }
    };

    let Some(response) = response else {
        {
            let mut state = session.lock().await;
            state.manual_restart_in_progress = false;
            state.last_stop_reason = Some("fetch_failed");
            state.active = false;
        }

        let _ = edit_refresh_components(&ctx.http, component.channel_id, message_id, false).await;

        component
            .edit_response(
                ctx,
                EditInteractionResponse::new()
                    .content("Failed to refresh quote data. Please try again later."),
            )
            .await?;
        return Ok(());
    };

    if let Err(error) = edit_message(
        &ctx.http,
        component.channel_id,
        message_id,
        response.embed.clone(),
        response.stop_reason.is_none(),
    )
    .await
    {
        let mut state = session.lock().await;
        state.manual_restart_in_progress = false;
        drop(state);
        let _ = edit_refresh_components(&ctx.http, component.channel_id, message_id, false).await;
        return Err(error);
    }

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
