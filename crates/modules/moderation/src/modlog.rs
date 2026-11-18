use std::num::NonZeroU64;

use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{ChannelId, CreateEmbed, CreateEmbedFooter, CreateMessage, User};
use tracing::warn;

use crate::settings::load_settings;

pub(crate) async fn send_modlog(
    ctx: Context<'_>,
    action: &str,
    user: &User,
    reason: Option<&str>,
) -> Result<(), Error> {
    let settings = load_settings(ctx).await?;
    let Some(channel_id) = settings
        .modlog_channel_id
        .and_then(NonZeroU64::new)
        .map(ChannelId::from)
    else {
        return Ok(());
    };
    let embed = CreateEmbed::new()
        .title(format!("Moderation - {action}"))
        .description(format!("{} [{}]", user.name, user.id))
        .field("Reason", reason.unwrap_or("No reason provided"), false)
        .footer(CreateEmbedFooter::new(format!(
            "By {} • {}",
            ctx.author().name,
            ctx.author().id
        )));
    if let Err(error) = channel_id
        .send_message(ctx, CreateMessage::new().embed(embed))
        .await
    {
        warn!(
            guild_id = ctx.guild_id().map(|id| id.get()),
            channel_id = channel_id.get(),
            action,
            ?error,
            "failed to send moderation log"
        );
    }
    Ok(())
}
