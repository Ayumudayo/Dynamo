use crate::{
    channels::{existing_ticket_channel, parse_ticket_details, ticket_channels},
    components::ticket_close_components,
    constants::{CLOSE_EMBED_COLOR, CREATE_EMBED_COLOR},
    settings::TicketSettings,
};
use dynamo_runtime_api::Error;
use poise::serenity_prelude::{
    ChannelId, ChannelType, CreateAttachment, CreateChannel, CreateEmbed, CreateEmbedFooter,
    CreateMessage, GetMessages, GuildChannel, PermissionOverwrite, PermissionOverwriteType,
    Permissions, RoleId,
};

pub(crate) async fn create_ticket_channel(
    ctx: &poise::serenity_prelude::Context,
    component: &poise::serenity_prelude::ComponentInteraction,
    settings: &TicketSettings,
    requested_category: Option<&str>,
) -> Result<String, Error> {
    let Some(guild_id) = component.guild_id else {
        return Ok("This action can only be used in a guild.".to_string());
    };

    let channels = ticket_channels(ctx, guild_id).await?;
    if let Some(channel) = existing_ticket_channel(&channels, component.user.id.get()) {
        return Ok(format!(
            "You already have an open ticket: <#{}>",
            channel.id.get()
        ));
    }

    if channels.len() >= settings.limit {
        return Ok("There are too many open tickets. Try again later.".to_string());
    }

    let category = requested_category
        .and_then(|name| settings.categories.iter().find(|category| category.name == name))
        .or_else(|| settings.categories.first());
    let category_name = category
        .map(|value| value.name.clone())
        .unwrap_or_else(|| "Default".to_string());

    let mut permission_overwrites = vec![
        PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::VIEW_CHANNEL,
            kind: PermissionOverwriteType::Role(guild_id.everyone_role()),
        },
        PermissionOverwrite {
            allow: Permissions::VIEW_CHANNEL
                | Permissions::SEND_MESSAGES
                | Permissions::READ_MESSAGE_HISTORY,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(component.user.id),
        },
        PermissionOverwrite {
            allow: Permissions::VIEW_CHANNEL
                | Permissions::SEND_MESSAGES
                | Permissions::READ_MESSAGE_HISTORY
                | Permissions::MANAGE_CHANNELS,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(ctx.cache.current_user().id),
        },
    ];

    if let Some(category) = category {
        for role_id in &category.staff_role_ids {
            permission_overwrites.push(PermissionOverwrite {
                allow: Permissions::VIEW_CHANNEL
                    | Permissions::SEND_MESSAGES
                    | Permissions::READ_MESSAGE_HISTORY,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Role(RoleId::new(*role_id)),
            });
        }
    }

    let ticket_number = channels.len() + 1;
    let channel = guild_id
        .create_channel(
            ctx,
            CreateChannel::new(format!("ticket-{ticket_number}"))
                .kind(ChannelType::Text)
                .topic(format!("ticket|{}|{}", component.user.id.get(), category_name))
                .permissions(permission_overwrites),
        )
        .await?;

    let welcome_embed = CreateEmbed::new()
        .title(format!("Ticket #{ticket_number}"))
        .description(format!(
            "Hello <@{}>\nSupport will be with you shortly.{}",
            component.user.id.get(),
            if category_name != "Default" {
                format!("\n**Category:** {category_name}")
            } else {
                String::new()
            }
        ))
        .color(CREATE_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(
            "You may close your ticket at any time using the button below.",
        ));

    let sent = channel
        .id
        .send_message(
            ctx,
            CreateMessage::new()
                .content(format!("<@{}>", component.user.id.get()))
                .embed(welcome_embed)
                .components(ticket_close_components()),
        )
        .await?;

    Ok(format!("Ticket created: {}", sent.link()))
}

pub(crate) async fn close_ticket_channel(
    ctx: &poise::serenity_prelude::Context,
    channel: &GuildChannel,
    closed_by: &str,
    settings: &TicketSettings,
    reason: Option<&str>,
) -> Result<String, Error> {
    let Some((opened_by, category_name)) = parse_ticket_details(channel) else {
        return Ok("Could not parse ticket metadata.".to_string());
    };

    let transcript = fetch_ticket_transcript(ctx, channel).await?;
    let transcript_name = format!("{}-transcript.txt", channel.name);

    let mut embed = CreateEmbed::new()
        .title("Ticket Closed")
        .color(CLOSE_EMBED_COLOR)
        .field("Opened By", format!("<@{opened_by}>"), true)
        .field("Closed By", closed_by, true)
        .field("Category", category_name, false);

    if let Some(reason) = reason {
        embed = embed.field("Reason", reason, false);
    }

    if let Some(log_channel_id) = settings.log_channel_id {
        let _ = ChannelId::new(log_channel_id)
            .send_message(
                ctx,
                CreateMessage::new()
                    .embed(embed.clone())
                    .add_file(CreateAttachment::bytes(
                        transcript.as_bytes(),
                        transcript_name,
                    )),
            )
            .await;
    }

    channel.delete(ctx).await?;
    Ok("Ticket closed.".to_string())
}

async fn fetch_ticket_transcript(
    ctx: &poise::serenity_prelude::Context,
    channel: &GuildChannel,
) -> Result<String, Error> {
    let mut messages = Vec::new();
    let mut before = None;

    loop {
        let mut builder = GetMessages::new().limit(100);
        if let Some(before_id) = before {
            builder = builder.before(before_id);
        }

        let batch = channel.messages(ctx, builder).await?;
        if batch.is_empty() {
            break;
        }

        before = batch.last().map(|message| message.id);
        messages.extend(batch);
        if messages.len() >= 1_000 {
            break;
        }
    }

    messages.reverse();

    if messages.is_empty() {
        return Ok("No messages were recorded for this ticket.".to_string());
    }

    let mut transcript = String::new();
    for message in messages {
        transcript.push_str(&render_transcript_message(&message));
        transcript.push('\n');
    }

    Ok(transcript)
}

fn render_transcript_message(message: &poise::serenity_prelude::Message) -> String {
    let timestamp = message.timestamp.to_string();
    let mut line = format!(
        "[{timestamp}] {} ({})",
        message.author.name, message.author.id
    );

    if !message.content.is_empty() {
        line.push('\n');
        line.push_str(&message.content);
    }

    if !message.attachments.is_empty() {
        let attachments = message
            .attachments
            .iter()
            .map(|attachment| attachment.url.clone())
            .collect::<Vec<_>>()
            .join(", ");
        line.push('\n');
        line.push_str("Attachments: ");
        line.push_str(&attachments);
    }

    line
}
