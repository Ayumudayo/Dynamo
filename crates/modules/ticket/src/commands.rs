use crate::{
    channels::{current_guild_channel, is_ticket_channel, ticket_channels},
    components::ticket_create_components,
    constants::{CREATE_EMBED_COLOR, MIN_LIMIT, MODULE_ID},
    settings::{load_settings, save_settings},
    workflow::close_ticket_channel,
};
use dynamo_access::module_access_for_context;
use dynamo_runtime_api::{Context, Error};
use poise::serenity_prelude::{
    ChannelId, CreateEmbed, CreateEmbedFooter, CreateMessage, PermissionOverwrite,
    PermissionOverwriteType, Permissions, RoleId, UserId,
};

/// Manage the guild's ticket panel, open tickets, and ticket access controls.
#[poise::command(
    slash_command,
    guild_only,
    category = "Ticket",
    subcommands(
        "ticket_setup",
        "ticket_log",
        "ticket_limit",
        "ticket_close",
        "ticket_closeall",
        "ticket_add",
        "ticket_remove"
    )
)]
pub(crate) async fn ticket(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Post or refresh the ticket creation panel in a target channel.
#[poise::command(
    slash_command,
    guild_only,
    rename = "setup",
    required_permissions = "MANAGE_GUILD"
)]
async fn ticket_setup(
    ctx: Context<'_>,
    #[description = "Channel where the ticket message should be posted"] channel: ChannelId,
    #[description = "Optional embed title"] title: Option<String>,
    #[description = "Optional embed description"] description: Option<String>,
    #[description = "Optional embed footer"] footer: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.send(
            poise::CreateReply::default()
                .content(reason)
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let mut settings = load_settings(ctx.data(), ctx.guild_id().map(|id| id.get())).await?;
    settings.setup_title = title.unwrap_or_else(|| settings.setup_title.clone());
    settings.setup_description = description.unwrap_or_else(|| settings.setup_description.clone());
    settings.setup_footer = footer.unwrap_or_else(|| settings.setup_footer.clone());

    let embed = CreateEmbed::new()
        .title(&settings.setup_title)
        .description(&settings.setup_description)
        .color(CREATE_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(&settings.setup_footer));

    channel
        .send_message(
            ctx.serenity_context(),
            CreateMessage::new()
                .embed(embed)
                .components(ticket_create_components()),
        )
        .await?;

    save_settings(ctx, &settings).await?;
    ctx.send(
        poise::CreateReply::default()
            .content("Ticket setup message created and ticket settings saved.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Set or clear the channel that receives ticket transcripts and close logs.
#[poise::command(
    slash_command,
    guild_only,
    rename = "log",
    required_permissions = "MANAGE_GUILD"
)]
async fn ticket_log(
    ctx: Context<'_>,
    #[description = "Optional log channel; omit to disable ticket logs"] channel: Option<ChannelId>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let mut settings = load_settings(ctx.data(), ctx.guild_id().map(|id| id.get())).await?;
    settings.log_channel_id = channel.map(|id| id.get());
    save_settings(ctx, &settings).await?;

    let message = match channel {
        Some(channel) => format!("Ticket logs will now be sent to <#{}>.", channel.get()),
        None => "Ticket log channel disabled.".to_string(),
    };

    ctx.send(
        poise::CreateReply::default()
            .content(message)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Set the maximum number of open ticket channels for this guild.
#[poise::command(
    slash_command,
    guild_only,
    rename = "limit",
    required_permissions = "MANAGE_GUILD"
)]
async fn ticket_limit(
    ctx: Context<'_>,
    #[description = "Maximum number of concurrently open ticket channels"] amount: i32,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    if amount < MIN_LIMIT as i32 {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("Ticket limit cannot be less than {MIN_LIMIT}."))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let mut settings = load_settings(ctx.data(), ctx.guild_id().map(|id| id.get())).await?;
    settings.limit = amount as usize;
    save_settings(ctx, &settings).await?;

    ctx.send(
        poise::CreateReply::default()
            .content(format!(
                "Configuration saved. The open ticket limit is now `{}`.",
                settings.limit
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Close the current ticket channel and archive its transcript.
#[poise::command(slash_command, guild_only, rename = "close")]
async fn ticket_close(
    ctx: Context<'_>,
    #[description = "Optional close reason"] reason: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let Some(guild_channel) =
        current_guild_channel(ctx.serenity_context(), ctx.channel_id()).await?
    else {
        ctx.send(
            poise::CreateReply::default()
                .content("This command can only be used inside a guild text channel.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    if !is_ticket_channel(&guild_channel) {
        ctx.send(
            poise::CreateReply::default()
                .content("This command can only be used in ticket channels.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let settings = load_settings(ctx.data(), Some(guild_channel.guild_id.get())).await?;
    let result = close_ticket_channel(
        ctx.serenity_context(),
        &guild_channel,
        &ctx.author().name,
        &settings,
        reason.as_deref(),
    )
    .await?;

    ctx.send(
        poise::CreateReply::default()
            .content(result)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Close every open ticket channel in the current guild.
#[poise::command(
    slash_command,
    guild_only,
    rename = "closeall",
    required_permissions = "MANAGE_GUILD"
)]
async fn ticket_closeall(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let settings = load_settings(ctx.data(), Some(guild_id.get())).await?;
    let channels = ticket_channels(ctx.serenity_context(), guild_id).await?;
    let mut closed = 0usize;
    let mut failed = 0usize;

    for channel in channels {
        match close_ticket_channel(
            ctx.serenity_context(),
            &channel,
            &ctx.author().name,
            &settings,
            Some("Force close all open tickets"),
        )
        .await
        {
            Ok(_) => closed += 1,
            Err(_) => failed += 1,
        }
    }

    ctx.send(
        poise::CreateReply::default()
            .content(format!("Completed. Success: `{closed}` Failed: `{failed}`"))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Grant a user or role access to the current ticket channel.
#[poise::command(
    slash_command,
    guild_only,
    rename = "add",
    required_permissions = "MANAGE_GUILD"
)]
async fn ticket_add(
    ctx: Context<'_>,
    #[description = "User ID, role ID, or mention to add to the current ticket"] target_id: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let Some(channel) = current_guild_channel(ctx.serenity_context(), ctx.channel_id()).await?
    else {
        ctx.send(
            poise::CreateReply::default()
                .content("This command can only be used inside a guild text channel.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    if !is_ticket_channel(&channel) {
        ctx.send(
            poise::CreateReply::default()
                .content("This command can only be used in ticket channels.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let overwrite =
        resolve_permission_target(ctx.serenity_context(), channel.guild_id, &target_id).await?;
    channel
        .create_permission(
            ctx.serenity_context(),
            PermissionOverwrite {
                allow: Permissions::VIEW_CHANNEL
                    | Permissions::SEND_MESSAGES
                    | Permissions::READ_MESSAGE_HISTORY,
                deny: Permissions::empty(),
                kind: overwrite,
            },
        )
        .await?;

    ctx.send(
        poise::CreateReply::default()
            .content("Ticket access updated.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Remove a user or role from the current ticket channel.
#[poise::command(
    slash_command,
    guild_only,
    rename = "remove",
    required_permissions = "MANAGE_GUILD"
)]
async fn ticket_remove(
    ctx: Context<'_>,
    #[description = "User ID, role ID, or mention to remove from the current ticket"]
    target_id: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let Some(channel) = current_guild_channel(ctx.serenity_context(), ctx.channel_id()).await?
    else {
        ctx.send(
            poise::CreateReply::default()
                .content("This command can only be used inside a guild text channel.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    if !is_ticket_channel(&channel) {
        ctx.send(
            poise::CreateReply::default()
                .content("This command can only be used in ticket channels.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let overwrite =
        resolve_permission_target(ctx.serenity_context(), channel.guild_id, &target_id).await?;
    channel
        .create_permission(
            ctx.serenity_context(),
            PermissionOverwrite {
                allow: Permissions::empty(),
                deny: Permissions::VIEW_CHANNEL
                    | Permissions::SEND_MESSAGES
                    | Permissions::READ_MESSAGE_HISTORY,
                kind: overwrite,
            },
        )
        .await?;

    ctx.send(
        poise::CreateReply::default()
            .content("Ticket access updated.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

async fn resolve_permission_target(
    ctx: &poise::serenity_prelude::Context,
    guild_id: poise::serenity_prelude::GuildId,
    input: &str,
) -> Result<PermissionOverwriteType, Error> {
    let target_id = extract_target_id(input)?;
    let roles = guild_id.roles(&ctx.http).await?;
    if roles.contains_key(&RoleId::new(target_id)) {
        return Ok(PermissionOverwriteType::Role(RoleId::new(target_id)));
    }

    Ok(PermissionOverwriteType::Member(UserId::new(target_id)))
}

pub(crate) fn extract_target_id(input: &str) -> Result<u64, Error> {
    let trimmed = input.trim();
    let digits = trimmed
        .trim_start_matches("<@")
        .trim_start_matches("&")
        .trim_start_matches("!")
        .trim_end_matches('>');
    digits
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("Invalid user/role identifier `{input}`: {error}"))
}
