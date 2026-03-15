use dynamo_core::{
    AppState, Context, DiscordCommand, Error, GatewayIntents, GuildModuleSettings, Module,
    ModuleCategory, ModuleManifest, SettingsField, SettingsFieldKind, SettingsSchema,
    SettingsSection, module_access_for_context,
};
use poise::serenity_prelude::{
    ButtonStyle, Channel, ChannelId, ChannelType, ComponentInteraction, CreateActionRow,
    CreateAttachment, CreateButton, CreateChannel, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu,
    CreateSelectMenuKind, CreateSelectMenuOption, EditInteractionResponse, GetMessages,
    GuildChannel, GuildId, Interaction, Message, PermissionOverwrite, PermissionOverwriteType,
    Permissions, RoleId, UserId,
};
use serde::{Deserialize, Deserializer, Serialize};

const MODULE_ID: &str = "ticket";
const TICKET_CREATE_BUTTON_ID: &str = "ticket_create";
const TICKET_CLOSE_BUTTON_ID: &str = "ticket_close";
const TICKET_CATEGORY_SELECT_ID: &str = "ticket_category_select";
const DEFAULT_LIMIT: usize = 10;
const MIN_LIMIT: usize = 5;
const DEFAULT_SETUP_TITLE: &str = "Support Ticket";
const DEFAULT_SETUP_DESCRIPTION: &str = "Please use the button below to create a ticket.";
const DEFAULT_SETUP_FOOTER: &str = "You can only have one open ticket at a time.";
const CREATE_EMBED_COLOR: u32 = 0x068ADD;
const CLOSE_EMBED_COLOR: u32 = 0x068ADD;

pub struct TicketModule;

impl Module for TicketModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Ticket",
            "Ticket setup, channel creation, and close workflow.",
            ModuleCategory::Ticket,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![ticket()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "ticketing",
                title: "Ticketing",
                description: Some(
                    "Configure ticket panel text, log channel, open-ticket limit, and category roles.",
                ),
                fields: vec![
                    SettingsField {
                        key: "setup_title",
                        label: "Panel title",
                        help_text: Some("Embed title used in the ticket creation panel."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "setup_description",
                        label: "Panel description",
                        help_text: Some("Embed description used in the ticket creation panel."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "setup_footer",
                        label: "Panel footer",
                        help_text: Some("Embed footer used in the ticket creation panel."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "log_channel_id",
                        label: "Log channel ID",
                        help_text: Some(
                            "Optional channel that receives ticket closed notifications and transcripts.",
                        ),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "limit",
                        label: "Open ticket limit",
                        help_text: Some("Maximum number of concurrently open ticket channels."),
                        required: false,
                        kind: SettingsFieldKind::Integer,
                    },
                    SettingsField {
                        key: "categories",
                        label: "Categories",
                        help_text: Some(
                            "Array of category objects with `name` and `staff_roles`/`staff_role_ids`.",
                        ),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                ],
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct TicketSettings {
    setup_title: String,
    setup_description: String,
    setup_footer: String,
    #[serde(
        alias = "log_channel",
        deserialize_with = "deserialize_optional_snowflake"
    )]
    log_channel_id: Option<u64>,
    limit: usize,
    categories: Vec<TicketCategory>,
}

impl Default for TicketSettings {
    fn default() -> Self {
        Self {
            setup_title: DEFAULT_SETUP_TITLE.to_string(),
            setup_description: DEFAULT_SETUP_DESCRIPTION.to_string(),
            setup_footer: DEFAULT_SETUP_FOOTER.to_string(),
            log_channel_id: None,
            limit: DEFAULT_LIMIT,
            categories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct TicketCategory {
    name: String,
    #[serde(alias = "staff_roles", deserialize_with = "deserialize_snowflake_vec")]
    staff_role_ids: Vec<u64>,
}

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
async fn ticket(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

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

#[poise::command(slash_command, guild_only, rename = "close")]
async fn ticket_close(
    ctx: Context<'_>,
    #[description = "Optional close reason"] reason: Option<String>,
) -> Result<(), Error> {
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

#[poise::command(
    slash_command,
    guild_only,
    rename = "closeall",
    required_permissions = "MANAGE_GUILD"
)]
async fn ticket_closeall(ctx: Context<'_>) -> Result<(), Error> {
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

async fn load_settings(data: &AppState, guild_id: Option<u64>) -> Result<TicketSettings, Error> {
    let Some(guild_id) = guild_id else {
        return Ok(TicketSettings::default());
    };

    let guild_settings = data.persistence.guild_settings_or_default(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(parse_ticket_settings)
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

fn parse_ticket_settings(module: &GuildModuleSettings) -> Result<TicketSettings, Error> {
    Ok(serde_json::from_value::<TicketSettings>(
        module.configuration.clone(),
    )?)
}

async fn save_settings(ctx: Context<'_>, settings: &TicketSettings) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let Some(repo) = ctx.data().persistence.guild_settings.clone() else {
        return Err(anyhow::anyhow!(
            "guild settings repository is not configured for this deployment"
        ));
    };

    let current = repo.get_or_create(guild_id.get()).await?;
    let enabled = current
        .modules
        .get(MODULE_ID)
        .map(|module| module.enabled)
        .unwrap_or(true);

    repo.upsert_module_settings(
        guild_id.get(),
        MODULE_ID,
        GuildModuleSettings {
            enabled,
            configuration: serde_json::to_value(settings)?,
        },
    )
    .await?;

    Ok(())
}

pub async fn handle_interaction(
    ctx: &poise::serenity_prelude::Context,
    interaction: &Interaction,
    data: &AppState,
) -> Result<bool, Error> {
    match interaction {
        Interaction::Component(component)
            if component.data.custom_id == TICKET_CREATE_BUTTON_ID =>
        {
            handle_ticket_open(ctx, component, data).await?;
            Ok(true)
        }
        Interaction::Component(component) if component.data.custom_id == TICKET_CLOSE_BUTTON_ID => {
            handle_ticket_close(ctx, component, data).await?;
            Ok(true)
        }
        Interaction::Component(component)
            if component.data.custom_id == TICKET_CATEGORY_SELECT_ID =>
        {
            handle_ticket_category_select(ctx, component, data).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn handle_ticket_open(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
    data: &AppState,
) -> Result<(), Error> {
    let settings = load_settings(data, component.guild_id.map(|id| id.get())).await?;
    if settings.categories.len() > 1 {
        let options = settings
            .categories
            .iter()
            .map(|category| CreateSelectMenuOption::new(&category.name, &category.name))
            .collect::<Vec<_>>();

        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Please choose a ticket category.")
                        .ephemeral(true)
                        .components(vec![CreateActionRow::SelectMenu(CreateSelectMenu::new(
                            TICKET_CATEGORY_SELECT_ID,
                            CreateSelectMenuKind::String { options },
                        ))]),
                ),
            )
            .await?;
        return Ok(());
    }

    component.defer_ephemeral(ctx).await?;
    let category_name = settings
        .categories
        .first()
        .map(|category| category.name.clone());
    let result = create_ticket_channel(ctx, component, &settings, category_name.as_deref()).await?;
    component
        .edit_response(ctx, EditInteractionResponse::new().content(result))
        .await?;
    Ok(())
}

async fn handle_ticket_category_select(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
    data: &AppState,
) -> Result<(), Error> {
    let settings = load_settings(data, component.guild_id.map(|id| id.get())).await?;
    let category_name = match &component.data.kind {
        poise::serenity_prelude::ComponentInteractionDataKind::StringSelect { values } => {
            values.first().cloned()
        }
        _ => None,
    };
    let Some(category_name) = category_name else {
        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Please choose a valid category.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    };

    component.defer_ephemeral(ctx).await?;
    let result = create_ticket_channel(ctx, component, &settings, Some(&category_name)).await?;
    component
        .edit_response(ctx, EditInteractionResponse::new().content(result))
        .await?;
    Ok(())
}

async fn handle_ticket_close(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
    data: &AppState,
) -> Result<(), Error> {
    component.defer_ephemeral(ctx).await?;
    let Some(channel) = current_guild_channel(ctx, component.channel_id).await? else {
        component
            .edit_response(
                ctx,
                EditInteractionResponse::new()
                    .content("This action can only be used inside a guild text channel."),
            )
            .await?;
        return Ok(());
    };

    if !is_ticket_channel(&channel) {
        component
            .edit_response(
                ctx,
                EditInteractionResponse::new()
                    .content("This action can only be used in ticket channels."),
            )
            .await?;
        return Ok(());
    }

    let settings = load_settings(data, Some(channel.guild_id.get())).await?;
    let result = close_ticket_channel(ctx, &channel, &component.user.name, &settings, None).await?;
    component
        .edit_response(ctx, EditInteractionResponse::new().content(result))
        .await?;
    Ok(())
}

async fn create_ticket_channel(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
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

async fn close_ticket_channel(
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

fn render_transcript_message(message: &Message) -> String {
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

async fn resolve_permission_target(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
    input: &str,
) -> Result<PermissionOverwriteType, Error> {
    let target_id = extract_target_id(input)?;
    let roles = guild_id.roles(&ctx.http).await?;
    if roles.contains_key(&RoleId::new(target_id)) {
        return Ok(PermissionOverwriteType::Role(RoleId::new(target_id)));
    }

    Ok(PermissionOverwriteType::Member(UserId::new(target_id)))
}

fn extract_target_id(input: &str) -> Result<u64, Error> {
    let trimmed = input.trim();
    let digits = trimmed
        .trim_start_matches("<@")
        .trim_start_matches("&")
        .trim_start_matches("!")
        .trim_end_matches('>');
    Ok(digits
        .parse::<u64>()
        .map_err(|error| anyhow::anyhow!("Invalid user/role identifier `{input}`: {error}"))?)
}

fn ticket_create_components() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(TICKET_CREATE_BUTTON_ID)
            .label("Open a ticket")
            .style(ButtonStyle::Success),
    ])]
}

fn ticket_close_components() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(TICKET_CLOSE_BUTTON_ID)
            .label("Close Ticket")
            .style(ButtonStyle::Primary)
            .emoji('🔒'),
    ])]
}

fn is_ticket_channel(channel: &GuildChannel) -> bool {
    channel.kind == ChannelType::Text
        && (channel.name.starts_with("ticket-") || channel.name.starts_with("tіcket-"))
        && channel
            .topic
            .as_deref()
            .is_some_and(|topic| topic.starts_with("ticket|") || topic.starts_with("tіcket|"))
}

fn parse_ticket_details(channel: &GuildChannel) -> Option<(u64, String)> {
    parse_ticket_topic(channel.topic.as_deref()?)
}

fn parse_ticket_topic(topic: &str) -> Option<(u64, String)> {
    let normalized = topic.replace("tіcket|", "ticket|");
    let mut split = normalized.split('|');
    let prefix = split.next()?;
    if prefix != "ticket" {
        return None;
    }
    let user_id = split.next()?.parse().ok()?;
    let category_name = split.next().unwrap_or("Default").to_string();
    Some((user_id, category_name))
}

async fn ticket_channels(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
) -> Result<Vec<GuildChannel>, Error> {
    Ok(guild_id
        .channels(ctx)
        .await?
        .into_values()
        .filter(is_ticket_channel)
        .collect())
}

fn existing_ticket_channel(channels: &[GuildChannel], user_id: u64) -> Option<GuildChannel> {
    channels
        .iter()
        .find(|channel| {
            parse_ticket_details(channel).is_some_and(|(opened_by, _)| opened_by == user_id)
        })
        .cloned()
}

async fn current_guild_channel(
    ctx: &poise::serenity_prelude::Context,
    channel_id: ChannelId,
) -> Result<Option<GuildChannel>, Error> {
    let channel = channel_id.to_channel(ctx).await?;
    Ok(match channel {
        Channel::Guild(channel) => Some(channel),
        _ => None,
    })
}

fn deserialize_optional_snowflake<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    parse_optional_snowflake_value(value).map_err(serde::de::Error::custom)
}

fn deserialize_snowflake_vec<'de, D>(deserializer: D) -> Result<Vec<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    parse_snowflake_vec_value(value).map_err(serde::de::Error::custom)
}

fn parse_optional_snowflake_value(value: Option<serde_json::Value>) -> Result<Option<u64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };

    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(value) if value.trim().is_empty() => Ok(None),
        serde_json::Value::String(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|error| format!("invalid snowflake `{value}`: {error}")),
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| "snowflake number must be an unsigned integer".to_string())
            .map(Some),
        other => Err(format!("snowflake must be a string or number, got {other}")),
    }
}

fn parse_snowflake_vec_value(value: Option<serde_json::Value>) -> Result<Vec<u64>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| parse_optional_snowflake_value(Some(value)))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| values.into_iter().flatten().collect()),
        serde_json::Value::String(values) if values.trim().is_empty() => Ok(Vec::new()),
        serde_json::Value::String(values) => values
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid snowflake `{value}`: {error}"))
            })
            .collect(),
        other => Err(format!(
            "snowflake array must be a string or array, got {other}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{TicketSettings, extract_target_id, parse_ticket_topic};

    #[test]
    fn ticket_settings_accepts_legacy_keys() {
        let settings: TicketSettings = serde_json::from_value(serde_json::json!({
            "log_channel": "123",
            "limit": 25,
            "categories": [
                { "name": "Billing", "staff_roles": ["11", 22] }
            ]
        }))
        .expect("settings should deserialize");

        assert_eq!(settings.log_channel_id, Some(123));
        assert_eq!(settings.limit, 25);
        assert_eq!(settings.categories[0].staff_role_ids, vec![11, 22]);
        assert_eq!(settings.setup_title, "Support Ticket");
    }

    #[test]
    fn parses_ascii_ticket_topic() {
        assert_eq!(
            parse_ticket_topic("ticket|42|Billing"),
            Some((42, "Billing".to_string()))
        );
    }

    #[test]
    fn extracts_mentions_to_numeric_ids() {
        assert_eq!(extract_target_id("<@123>").expect("user mention"), 123);
        assert_eq!(extract_target_id("<@&456>").expect("role mention"), 456);
        assert_eq!(extract_target_id("<@!789>").expect("nickname mention"), 789);
    }
}
