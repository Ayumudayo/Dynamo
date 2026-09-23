use dynamo_access::command_access_for_context;
use dynamo_runtime_api::{AppState, Error};
use poise::{CreateReply, FrameworkError, serenity_prelude as serenity};
use tracing::{error, warn};

pub(super) fn event_handler<'a>(
    ctx: &'a serenity::Context,
    event: &'a serenity::FullEvent,
    _framework: poise::FrameworkContext<'a, AppState, Error>,
    data: &'a AppState,
) -> poise::BoxFuture<'a, Result<(), Error>> {
    Box::pin(async move { dynamo_app::handle_framework_event(ctx, event, data).await })
}

pub(super) fn framework_on_error(
    error: FrameworkError<'_, AppState, Error>,
) -> poise::BoxFuture<'_, ()> {
    Box::pin(async move {
        match error {
            FrameworkError::Command { ctx, error, .. } => {
                error!(
                    command = ctx.command().qualified_name,
                    ?error,
                    "command execution failed"
                );

                let user_message = format!("Command failed: {error}");
                if let Err(send_error) = ctx
                    .send(CreateReply::default().content(user_message).ephemeral(true))
                    .await
                {
                    if send_error.to_string().contains("Unknown interaction") {
                        warn!(
                            command = ctx.command().qualified_name,
                            ?send_error,
                            "failed to deliver command error because the interaction expired"
                        );
                    } else {
                        error!(?send_error, "failed to send command failure");
                    }
                }
            }
            FrameworkError::CommandCheckFailed {
                ctx,
                error: Some(error),
                ..
            } => {
                if let Err(send_error) = ctx
                    .send(
                        CreateReply::default()
                            .content(error.to_string())
                            .ephemeral(true),
                    )
                    .await
                {
                    error!(?send_error, "failed to send command check failure");
                }
            }
            other => {
                if let Err(error) = poise::builtins::on_error(other).await {
                    error!(?error, "framework error handler failed");
                }
            }
        }
    })
}

pub(super) fn command_check(
    ctx: poise::Context<'_, AppState, Error>,
) -> poise::BoxFuture<'_, Result<bool, Error>> {
    Box::pin(async move {
        let access = command_access_for_context(ctx).await?;
        if access.allowed() {
            Ok(true)
        } else {
            Err(anyhow::anyhow!(
                access
                    .denial_reason
                    .unwrap_or_else(|| "This command is disabled.".to_string())
            ))
        }
    })
}
