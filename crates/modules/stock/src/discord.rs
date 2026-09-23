use crate::{constants::STOCK_REFRESH_BUTTON_ID, render::refresh_components};
use dynamo_runtime_api::Error;
use poise::serenity_prelude::{ChannelId, CreateEmbed, EditMessage, Http};

pub(crate) async fn edit_message(
    http: &Http,
    channel_id: ChannelId,
    message_id: u64,
    embed: CreateEmbed,
    refresh_disabled: bool,
) -> Result<(), Error> {
    channel_id
        .edit_message(
            http,
            message_id,
            EditMessage::new()
                .embed(embed)
                .components(refresh_components(
                    STOCK_REFRESH_BUTTON_ID,
                    refresh_disabled,
                )),
        )
        .await?;
    Ok(())
}

pub(crate) async fn edit_refresh_components(
    http: &Http,
    channel_id: ChannelId,
    message_id: u64,
    disabled: bool,
) -> Result<(), Error> {
    channel_id
        .edit_message(
            http,
            message_id,
            EditMessage::new().components(refresh_components(STOCK_REFRESH_BUTTON_ID, disabled)),
        )
        .await?;
    Ok(())
}
