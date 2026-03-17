use dynamo_domain_invite::InviteMemberRecord;
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime::{AppState, Context, Error, module_access_for_app, module_access_for_context};
use poise::serenity_prelude::{
    ChannelId, CreateEmbed, CreateEmbedFooter, CreateMessage, GuildId, Member, Mentionable, User,
    UserId,
};
use serde::{Deserialize, Serialize};

const MODULE_ID: &str = "greeting";
const VANITY_MEMBER_ID: &str = "VANITY";

pub struct GreetingModule;

impl Module<AppState, Error> for GreetingModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Greeting",
            "Welcome and farewell messages with inviter placeholders.",
            ModuleCategory::Utility,
            true,
            GatewayIntents::GUILDS | GatewayIntents::GUILD_MEMBERS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![greeting()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![
                greeting_section("welcome", "Welcome"),
                greeting_section("farewell", "Farewell"),
            ],
        }
    }
}

fn greeting_section(prefix: &'static str, title: &'static str) -> SettingsSection {
    SettingsSection {
        id: prefix,
        title,
        description: Some("Use dashboard fields or advanced JSON to configure greeting output."),
        fields: vec![
            SettingsField {
                key: Box::leak(format!("{prefix}.enabled").into_boxed_str()),
                label: "Enabled",
                help_text: Some("Whether this greeting should be sent."),
                required: false,
                kind: SettingsFieldKind::Toggle,
            },
            SettingsField {
                key: Box::leak(format!("{prefix}.channel").into_boxed_str()),
                label: "Channel ID",
                help_text: Some("Target channel for the greeting."),
                required: false,
                kind: SettingsFieldKind::Text,
            },
            SettingsField {
                key: Box::leak(format!("{prefix}.content").into_boxed_str()),
                label: "Content",
                help_text: Some("Optional plain-text content template."),
                required: false,
                kind: SettingsFieldKind::Text,
            },
            SettingsField {
                key: Box::leak(format!("{prefix}.embed.description").into_boxed_str()),
                label: "Embed Description",
                help_text: Some("Optional embed description template."),
                required: false,
                kind: SettingsFieldKind::Text,
            },
            SettingsField {
                key: Box::leak(format!("{prefix}.embed.color").into_boxed_str()),
                label: "Embed Color",
                help_text: Some("Hex color like `#068ADD`."),
                required: false,
                kind: SettingsFieldKind::Text,
            },
            SettingsField {
                key: Box::leak(format!("{prefix}.embed.thumbnail").into_boxed_str()),
                label: "Embed Thumbnail",
                help_text: Some("Use the member avatar as the embed thumbnail."),
                required: false,
                kind: SettingsFieldKind::Toggle,
            },
            SettingsField {
                key: Box::leak(format!("{prefix}.embed.footer").into_boxed_str()),
                label: "Embed Footer",
                help_text: Some("Optional embed footer template."),
                required: false,
                kind: SettingsFieldKind::Text,
            },
            SettingsField {
                key: Box::leak(format!("{prefix}.embed.image").into_boxed_str()),
                label: "Embed Image",
                help_text: Some("Optional embed image URL template."),
                required: false,
                kind: SettingsFieldKind::Text,
            },
        ],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct GreetingSettings {
    welcome: GreetingSideConfig,
    farewell: GreetingSideConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct GreetingSideConfig {
    enabled: bool,
    #[serde(alias = "channel")]
    channel_id: Option<u64>,
    content: Option<String>,
    embed: GreetingEmbedConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct GreetingEmbedConfig {
    description: Option<String>,
    color: Option<String>,
    thumbnail: bool,
    footer: Option<String>,
    image: Option<String>,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
enum GreetingKind {
    Welcome,
    Farewell,
}

#[derive(Debug, Clone)]
struct GreetingSubject {
    guild_name: String,
    guild_icon_url: Option<String>,
    member_count: u64,
    display_name: String,
    username: String,
    tag: String,
    mention: String,
    avatar_url: String,
    is_bot: bool,
}

/// Manage greeting previews for the configured welcome and farewell templates.
#[poise::command(
    slash_command,
    guild_only,
    category = "Utility",
    subcommands("greeting_preview")
)]
async fn greeting(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Preview a welcome or farewell message using the current guild settings.
#[poise::command(
    slash_command,
    guild_only,
    rename = "preview",
    required_permissions = "MANAGE_GUILD"
)]
async fn greeting_preview(
    ctx: Context<'_>,
    #[description = "Which greeting to preview"] kind: GreetingKind,
    #[description = "Optional member to preview with"] user: Option<User>,
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

    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let user = user.unwrap_or_else(|| ctx.author().clone());
    let member = guild_id.member(ctx, user.id).await?;
    let subject = subject_from_member(ctx.serenity_context(), &member);
    let settings = load_settings(ctx.data(), guild_id.get()).await?;
    let config = match kind {
        GreetingKind::Welcome => &settings.welcome,
        GreetingKind::Farewell => &settings.farewell,
    };
    let inviter_data = if let Some(repo) = ctx.data().persistence.invites.as_ref() {
        Some(
            repo.get_or_create(guild_id.get(), &user.id.get().to_string())
                .await?,
        )
    } else {
        None
    };

    let message = build_greeting_message(
        ctx.serenity_context(),
        &subject,
        config,
        inviter_data.as_ref(),
    )
    .await?;
    let mut reply = poise::CreateReply::default().ephemeral(true);
    if let Some(content) = message.content {
        reply = reply.content(content);
    }
    if let Some(embed) = message.embed {
        reply = reply.embed(embed);
    }
    ctx.send(reply).await?;
    Ok(())
}

pub async fn send_welcome(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    member: &Member,
    inviter_data: Option<&InviteMemberRecord>,
) -> Result<(), Error> {
    if module_access_for_app(data, MODULE_ID, Some(member.guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(());
    }

    let settings = load_settings(data, member.guild_id.get()).await?;
    if !settings.welcome.enabled {
        return Ok(());
    }

    let subject = subject_from_member(ctx, member);
    dispatch_greeting(ctx, &subject, &settings.welcome, inviter_data).await
}

pub async fn send_farewell(
    ctx: &poise::serenity_prelude::Context,
    data: &AppState,
    guild_id: GuildId,
    user: &User,
    member_data: Option<&Member>,
    inviter_data: Option<&InviteMemberRecord>,
) -> Result<(), Error> {
    if module_access_for_app(data, MODULE_ID, Some(guild_id.get()))
        .await?
        .denial_reason
        .is_some()
    {
        return Ok(());
    }

    let settings = load_settings(data, guild_id.get()).await?;
    if !settings.farewell.enabled {
        return Ok(());
    }

    let subject = subject_from_departed_user(ctx, guild_id, user, member_data);
    dispatch_greeting(ctx, &subject, &settings.farewell, inviter_data).await
}

async fn load_settings(data: &AppState, guild_id: u64) -> Result<GreetingSettings, Error> {
    let guild_settings = data.persistence.guild_settings_or_default(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| serde_json::from_value::<GreetingSettings>(module.configuration.clone()))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

async fn dispatch_greeting(
    ctx: &poise::serenity_prelude::Context,
    subject: &GreetingSubject,
    config: &GreetingSideConfig,
    inviter_data: Option<&InviteMemberRecord>,
) -> Result<(), Error> {
    let Some(channel_id) = config.channel_id else {
        return Ok(());
    };

    let message = build_greeting_message(ctx, subject, config, inviter_data).await?;
    let mut builder = CreateMessage::new();
    if let Some(content) = message.content {
        builder = builder.content(content);
    }
    if let Some(embed) = message.embed {
        builder = builder.embed(embed);
    }

    ChannelId::new(channel_id)
        .send_message(ctx, builder)
        .await?;
    Ok(())
}

struct BuiltGreetingMessage {
    content: Option<String>,
    embed: Option<CreateEmbed>,
}

async fn build_greeting_message(
    ctx: &poise::serenity_prelude::Context,
    subject: &GreetingSubject,
    config: &GreetingSideConfig,
    inviter_data: Option<&InviteMemberRecord>,
) -> Result<BuiltGreetingMessage, Error> {
    let inviter = resolve_inviter(ctx, subject.is_bot, inviter_data).await?;
    let content = config
        .content
        .as_deref()
        .map(|template| render_template(template, subject, &inviter));

    let has_embed = config.embed.description.is_some()
        || config.embed.footer.is_some()
        || config.embed.image.is_some();

    if !has_embed && content.is_none() {
        let default_content = if subject.is_bot {
            format!("Welcome to the server, {}.", subject.display_name)
        } else {
            format!("Welcome to the server, {} 🎉", subject.display_name)
        };
        return Ok(BuiltGreetingMessage {
            content: Some(default_content),
            embed: None,
        });
    }

    let mut embed = CreateEmbed::new();
    if let Some(description) = &config.embed.description {
        embed = embed.description(render_template(description, subject, &inviter));
    }
    if let Some(color) = config.embed.color.as_deref().and_then(parse_hex_color) {
        embed = embed.color(color);
    }
    if config.embed.thumbnail {
        embed = embed.thumbnail(&subject.avatar_url);
    }
    if let Some(footer) = &config.embed.footer {
        embed = embed.footer(CreateEmbedFooter::new(render_template(
            footer, subject, &inviter,
        )));
    }
    if let Some(image) = &config.embed.image {
        embed = embed.image(render_template(image, subject, &inviter));
    }
    if let Some(icon) = &subject.guild_icon_url {
        embed = embed.author(
            poise::serenity_prelude::CreateEmbedAuthor::new(&subject.guild_name).icon_url(icon),
        );
    } else {
        embed = embed.author(poise::serenity_prelude::CreateEmbedAuthor::new(
            &subject.guild_name,
        ));
    }

    Ok(BuiltGreetingMessage {
        content,
        embed: Some(embed),
    })
}

struct InviterRenderData {
    name: String,
    tag: String,
    invites: i64,
}

async fn resolve_inviter(
    ctx: &poise::serenity_prelude::Context,
    subject_is_bot: bool,
    inviter_data: Option<&InviteMemberRecord>,
) -> Result<InviterRenderData, Error> {
    let Some(inviter_data) = inviter_data else {
        return Ok(if subject_is_bot {
            InviterRenderData {
                name: "OAuth".to_string(),
                tag: "OAuth".to_string(),
                invites: 0,
            }
        } else {
            InviterRenderData {
                name: "NA".to_string(),
                tag: "NA".to_string(),
                invites: 0,
            }
        });
    };

    let inviter_key = inviter_data.member_id.as_str();
    if inviter_key == VANITY_MEMBER_ID {
        return Ok(InviterRenderData {
            name: VANITY_MEMBER_ID.to_string(),
            tag: VANITY_MEMBER_ID.to_string(),
            invites: inviter_data.invite_data.effective(),
        });
    }

    if let Ok(user_id) = inviter_key.parse::<u64>() {
        if let Ok(user) = UserId::new(user_id).to_user(ctx).await {
            return Ok(InviterRenderData {
                name: user.name.clone(),
                tag: user.tag(),
                invites: inviter_data.invite_data.effective(),
            });
        }
    }

    Ok(InviterRenderData {
        name: "NA".to_string(),
        tag: "NA".to_string(),
        invites: inviter_data.invite_data.effective(),
    })
}

fn render_template(
    template: &str,
    subject: &GreetingSubject,
    inviter: &InviterRenderData,
) -> String {
    template
        .replace("\\n", "\n")
        .replace("{server}", &subject.guild_name)
        .replace("{count}", &subject.member_count.to_string())
        .replace("{member:nick}", &subject.display_name)
        .replace("{member:name}", &subject.username)
        .replace("{member:tag}", &subject.tag)
        .replace("{member:mention}", &subject.mention)
        .replace("{member:avatar}", &subject.avatar_url)
        .replace("{inviter:name}", &inviter.name)
        .replace("{inviter:tag}", &inviter.tag)
        .replace("{invites}", &inviter.invites.to_string())
}

fn parse_hex_color(value: &str) -> Option<u32> {
    let trimmed = value.trim().trim_start_matches('#');
    u32::from_str_radix(trimmed, 16).ok()
}

fn subject_from_member(ctx: &poise::serenity_prelude::Context, member: &Member) -> GreetingSubject {
    let guild = ctx.cache.guild(member.guild_id);
    let guild_name = guild
        .as_ref()
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| member.guild_id.to_string());
    let guild_icon_url = guild.as_ref().and_then(|guild| guild.icon_url());
    let member_count = guild.as_ref().map(|guild| guild.member_count).unwrap_or(0);

    GreetingSubject {
        guild_name,
        guild_icon_url,
        member_count,
        display_name: member.display_name().to_string(),
        username: member.user.name.clone(),
        tag: member.user.tag(),
        mention: member.user.mention().to_string(),
        avatar_url: member.face(),
        is_bot: member.user.bot,
    }
}

fn subject_from_departed_user(
    ctx: &poise::serenity_prelude::Context,
    guild_id: GuildId,
    user: &User,
    member_data: Option<&Member>,
) -> GreetingSubject {
    let guild = ctx.cache.guild(guild_id);
    let guild_name = guild
        .as_ref()
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| guild_id.to_string());
    let guild_icon_url = guild.as_ref().and_then(|guild| guild.icon_url());
    let member_count = guild.as_ref().map(|guild| guild.member_count).unwrap_or(0);

    GreetingSubject {
        guild_name,
        guild_icon_url,
        member_count,
        display_name: member_data
            .map(|member| member.display_name().to_string())
            .unwrap_or_else(|| user.name.clone()),
        username: user.name.clone(),
        tag: user.tag(),
        mention: user.mention().to_string(),
        avatar_url: user.face(),
        is_bot: user.bot,
    }
}
