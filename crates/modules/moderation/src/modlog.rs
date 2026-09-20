use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, User};

use crate::settings::load_settings;

pub(crate) async fn send_modlog(
    ctx: Context<'_>,
    action: &str,
    user: &User,
    reason: Option<&str>,
) -> Result<(), Error> {
    let settings = load_settings(ctx).await?;
    let Some(channel_id) = settings.modlog_channel_id else {
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
    poise::serenity_prelude::ChannelId::new(channel_id)
        .send_message(
            ctx,
            poise::serenity_prelude::CreateMessage::new().embed(embed),
        )
        .await?;
    Ok(())
}
