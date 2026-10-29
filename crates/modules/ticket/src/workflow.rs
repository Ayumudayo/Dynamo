use crate::{
    channels::{existing_ticket_channel, parse_ticket_details, ticket_channels},
    components::ticket_close_components,
    constants::{CLOSE_EMBED_COLOR, CREATE_EMBED_COLOR},
    settings::TicketSettings,
};
use anyhow::Context as _;
use dynamo_runtime_api::Error;
use poise::serenity_prelude::{
    ChannelId, ChannelType, CreateAttachment, CreateChannel, CreateEmbed, CreateEmbedFooter,
    CreateMessage, GetMessages, GuildChannel, PermissionOverwrite, PermissionOverwriteType,
    Permissions, RoleId,
};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio::sync::Mutex as AsyncMutex;

const MAX_TRANSCRIPT_MESSAGES: usize = 10_000;
const MAX_TRANSCRIPT_BYTES: usize = 8 * 1024 * 1024;

fn guild_workflow_lock(guild_id: u64) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<u64, Weak<AsyncMutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .expect("ticket lock registry poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&guild_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(guild_id, Arc::downgrade(&lock));
    lock
}

pub(crate) async fn create_ticket_channel(
    ctx: &poise::serenity_prelude::Context,
    component: &poise::serenity_prelude::ComponentInteraction,
    settings: &TicketSettings,
    requested_category: Option<&str>,
) -> Result<String, Error> {
    let Some(guild_id) = component.guild_id else {
        return Ok("This action can only be used in a guild.".to_string());
    };

    // Hold through the fresh channel lookup and creation so concurrent open
    // requests cannot both pass the per-user and per-guild limits.
    let workflow_lock = guild_workflow_lock(guild_id.get());
    let _guard = workflow_lock.lock().await;

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
        .and_then(|name| {
            settings
                .categories
                .iter()
                .find(|category| category.name == name)
        })
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
                .topic(format!(
                    "ticket|{}|{}",
                    component.user.id.get(),
                    category_name
                ))
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
    let workflow_lock = guild_workflow_lock(channel.guild_id.get());
    let _guard = workflow_lock.lock().await;
    let Some((opened_by, category_name)) = parse_ticket_details(channel) else {
        anyhow::bail!("Ticket kept open: could not parse ticket metadata.");
    };
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

    archive_then_delete(
        || async {
            if let Some(log_channel_id) = settings.log_channel_id {
                anyhow::ensure!(
                    log_channel_id != channel.id.get(),
                    "The ticket cannot be its own transcript log channel."
                );
                let transcript = fetch_ticket_transcript(ctx, channel).await?;
                ChannelId::new(log_channel_id)
                    .send_message(
                        ctx,
                        CreateMessage::new().embed(embed.clone()).add_file(
                            CreateAttachment::bytes(transcript.as_bytes(), transcript_name),
                        ),
                    )
                    .await?;
            }
            Ok(())
        },
        || async {
            channel.delete(ctx).await?;
            Ok(())
        },
    )
    .await?;
    Ok("Ticket closed.".to_string())
}

async fn archive_then_delete<A, AF, D, DF>(archive: A, delete: D) -> Result<(), Error>
where
    A: FnOnce() -> AF,
    AF: Future<Output = Result<(), Error>>,
    D: FnOnce() -> DF,
    DF: Future<Output = Result<(), Error>>,
{
    archive().await.context("Ticket kept open: the complete transcript could not be saved. Check the log channel and archive size, then retry closing the ticket.")?;
    delete().await.context("Ticket channel deletion failed. Retry closing the ticket after checking the bot's channel permissions.")?;
    Ok(())
}

async fn fetch_ticket_transcript(
    ctx: &poise::serenity_prelude::Context,
    channel: &GuildChannel,
) -> Result<String, Error> {
    let mut messages = Vec::new();
    let mut total_bytes = 0usize;
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
        for message in batch {
            let rendered = render_transcript_message(&message);
            append_transcript_entry(&mut messages, &mut total_bytes, rendered)?;
        }
    }

    messages.reverse();

    if messages.is_empty() {
        return Ok("No messages were recorded for this ticket.".to_string());
    }

    let mut transcript = String::new();
    for message in messages {
        transcript.push_str(&message);
        transcript.push('\n');
    }

    Ok(transcript)
}

fn append_transcript_entry(
    messages: &mut Vec<String>,
    total_bytes: &mut usize,
    rendered: String,
) -> Result<(), Error> {
    let next_bytes = total_bytes.saturating_add(rendered.len()).saturating_add(1);
    anyhow::ensure!(
        messages.len() < MAX_TRANSCRIPT_MESSAGES && next_bytes <= MAX_TRANSCRIPT_BYTES,
        "Complete transcript exceeds the archive limit; preserve the channel and export it before closing."
    );
    *total_bytes = next_bytes;
    messages.push(rendered);
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn archive_failure_never_attempts_channel_deletion() {
        let deleted = AtomicBool::new(false);
        let error = archive_then_delete(
            || async { anyhow::bail!("Discord refused the transcript upload") },
            || async {
                deleted.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert!(!deleted.load(Ordering::SeqCst));
        assert!(error.to_string().contains("Ticket kept open"));
    }

    #[tokio::test]
    async fn deletion_occurs_only_after_archive_completes() {
        let archived = AtomicBool::new(false);
        archive_then_delete(
            || async {
                tokio::task::yield_now().await;
                archived.store(true, Ordering::SeqCst);
                Ok(())
            },
            || async {
                assert!(archived.load(Ordering::SeqCst));
                Ok(())
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn deletion_failure_is_reported_as_failure_for_close_all() {
        let error = archive_then_delete(
            || async { Ok(()) },
            || async { anyhow::bail!("missing manage channels permission") },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("deletion failed"));
    }

    #[tokio::test]
    async fn guild_workflows_share_lock_but_other_guilds_are_independent() {
        let first = guild_workflow_lock(123456);
        let guard = first.lock().await;
        let same_guild = guild_workflow_lock(123456);
        assert!(same_guild.try_lock().is_err());
        let other_guild = guild_workflow_lock(123457);
        assert!(other_guild.try_lock().is_ok());
        drop(guard);
        assert!(same_guild.try_lock().is_ok());
    }

    #[test]
    fn transcripts_keep_more_than_one_thousand_messages_and_fail_instead_of_truncating() {
        let mut messages = Vec::new();
        let mut bytes = 0;
        for _ in 0..MAX_TRANSCRIPT_MESSAGES {
            append_transcript_entry(&mut messages, &mut bytes, "message".into()).unwrap();
        }
        assert_eq!(messages.len(), MAX_TRANSCRIPT_MESSAGES);
        assert!(append_transcript_entry(&mut messages, &mut bytes, "overflow".into()).is_err());
        assert_eq!(messages.len(), MAX_TRANSCRIPT_MESSAGES);
    }

    #[test]
    fn archive_byte_budget_rejects_incomplete_export() {
        let mut messages = Vec::new();
        let mut bytes = 0;
        assert!(
            append_transcript_entry(&mut messages, &mut bytes, "x".repeat(MAX_TRANSCRIPT_BYTES))
                .is_err()
        );
        assert!(messages.is_empty());
        assert_eq!(bytes, 0);
    }
}
