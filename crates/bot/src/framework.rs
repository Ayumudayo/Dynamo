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

async fn send_ephemeral_notice(ctx: poise::Context<'_, AppState, Error>, content: String) {
    if let Err(send_error) = ctx
        .send(CreateReply::default().content(content).ephemeral(true))
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
                let message = if dynamo_runtime_api::is_korean_locale(ctx.locale()) {
                    "명령어를 실행하는 중 예기치 않은 오류가 발생했습니다. 잠시 후 다시 시도해 주세요."
                } else {
                    "An unexpected error occurred while running this command. Please try again later."
                };
                send_ephemeral_notice(ctx, message.to_string()).await;
            }
            FrameworkError::CommandCheckFailed {
                ctx,
                error: Some(error),
                ..
            } => {
                send_ephemeral_notice(ctx, error.to_string()).await;
            }
            FrameworkError::MissingUserPermissions {
                ctx,
                missing_permissions,
                ..
            } => {
                let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
                let message = match (ko, missing_permissions) {
                    (true, Some(perms)) => {
                        format!("이 명령어를 실행하기 위한 권한이 부족합니다: `{perms}`.")
                    }
                    (true, None) => "이 명령어를 실행할 권한이 없습니다.".to_string(),
                    (false, Some(perms)) => format!(
                        "You are missing required permissions to run this command: `{perms}`."
                    ),
                    (false, None) => {
                        "You lack the required permissions to run this command.".to_string()
                    }
                };
                send_ephemeral_notice(ctx, message).await;
            }
            FrameworkError::MissingBotPermissions {
                ctx,
                missing_permissions,
                ..
            } => {
                let message = if dynamo_runtime_api::is_korean_locale(ctx.locale()) {
                    format!(
                        "봇에 이 명령어를 실행하기 위한 권한이 부족합니다: `{missing_permissions}`."
                    )
                } else {
                    format!(
                        "I lack the required permissions to run this command: `{missing_permissions}`."
                    )
                };
                send_ephemeral_notice(ctx, message).await;
            }
            FrameworkError::ArgumentParse {
                ctx, input, error, ..
            } => {
                let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
                let message = match (ko, input) {
                    (true, Some(input)) => format!("잘못된 인자입니다 (`{input}`): {error}"),
                    (true, None) => format!("잘못된 인자입니다: {error}"),
                    (false, Some(input)) => format!("Invalid argument `{input}`: {error}"),
                    (false, None) => format!("Invalid argument: {error}"),
                };
                send_ephemeral_notice(ctx, message).await;
            }
            FrameworkError::CooldownHit {
                ctx,
                remaining_cooldown,
                ..
            } => {
                let secs = remaining_cooldown.as_secs_f32().max(0.1);
                let message = if dynamo_runtime_api::is_korean_locale(ctx.locale()) {
                    format!("재사용 대기 중입니다. {secs:.1}초 후에 다시 시도해 주세요.")
                } else {
                    format!(
                        "You are on cooldown. Please wait {secs:.1}s before using this command again."
                    )
                };
                send_ephemeral_notice(ctx, message).await;
            }
            FrameworkError::GuildOnly { ctx, .. } => {
                let message = if dynamo_runtime_api::is_korean_locale(ctx.locale()) {
                    "이 명령어는 Discord 서버 안에서만 사용할 수 있습니다."
                } else {
                    "This command can only be used inside a Discord server."
                };
                send_ephemeral_notice(ctx, message.to_string()).await;
            }
            FrameworkError::DmOnly { ctx, .. } => {
                let message = if dynamo_runtime_api::is_korean_locale(ctx.locale()) {
                    "이 명령어는 다이렉트 메시지(DM)에서만 사용할 수 있습니다."
                } else {
                    "This command can only be used in direct messages."
                };
                send_ephemeral_notice(ctx, message.to_string()).await;
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
            let fallback = if dynamo_runtime_api::is_korean_locale(ctx.locale()) {
                "이 명령어는 비활성화되어 있습니다."
            } else {
                "This command is disabled."
            };
            Err(anyhow::anyhow!(
                access.denial_reason.unwrap_or_else(|| fallback.to_string())
            ))
        }
    })
}
