use dynamo_access::module_access_for_app;
use dynamo_domain_invite::InviteMemberRecord;
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{
    AppState, Context, Error, decode_module_configuration, deserialize_optional_snowflake,
};
use poise::serenity_prelude::{
    ChannelId, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, CreateMessage, GuildId,
    Member, Mentionable, User, UserId,
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
                greeting_section("welcome", "Welcome", &WELCOME_SECTION_KEYS),
                greeting_section("farewell", "Farewell", &FAREWELL_SECTION_KEYS),
            ],
        }
    }
}

struct GreetingSectionKeys {
    enabled: &'static str,
    channel: &'static str,
    content: &'static str,
    description: &'static str,
    color: &'static str,
    thumbnail: &'static str,
    footer: &'static str,
    image: &'static str,
}

const WELCOME_SECTION_KEYS: GreetingSectionKeys = GreetingSectionKeys {
    enabled: "welcome.enabled",
    channel: "welcome.channel",
    content: "welcome.content",
    description: "welcome.embed.description",
    color: "welcome.embed.color",
    thumbnail: "welcome.embed.thumbnail",
    footer: "welcome.embed.footer",
    image: "welcome.embed.image",
};

const FAREWELL_SECTION_KEYS: GreetingSectionKeys = GreetingSectionKeys {
    enabled: "farewell.enabled",
    channel: "farewell.channel",
    content: "farewell.content",
    description: "farewell.embed.description",
    color: "farewell.embed.color",
    thumbnail: "farewell.embed.thumbnail",
    footer: "farewell.embed.footer",
    image: "farewell.embed.image",
};

fn greeting_section(
    id: &'static str,
    title: &'static str,
    keys: &GreetingSectionKeys,
) -> SettingsSection {
    SettingsSection {
        id,
        title,
        description: Some("Configure message text, channel, and embed preview for this event."),
        fields: vec![
            SettingsField {
                key: keys.enabled,
                label: "Enabled",
                help_text: Some("Whether this greeting should be sent."),
                required: false,
                kind: SettingsFieldKind::Toggle,
            },
            SettingsField {
                key: keys.channel,
                label: "Channel",
                help_text: Some("Target channel for the greeting."),
                required: false,
                kind: SettingsFieldKind::Channel,
            },
            SettingsField {
                key: keys.content,
                label: "Content",
                help_text: Some("Optional plain-text content template."),
                required: false,
                kind: SettingsFieldKind::Textarea,
            },
            SettingsField {
                key: keys.description,
                label: "Embed Description",
                help_text: Some("Optional embed description template."),
                required: false,
                kind: SettingsFieldKind::Textarea,
            },
            SettingsField {
                key: keys.color,
                label: "Embed Color",
                help_text: Some("Hex color such as #068ADD."),
                required: false,
                kind: SettingsFieldKind::Color,
            },
            SettingsField {
                key: keys.thumbnail,
                label: "Embed Thumbnail",
                help_text: Some("Use the member avatar as the embed thumbnail."),
                required: false,
                kind: SettingsFieldKind::Toggle,
            },
            SettingsField {
                key: keys.footer,
                label: "Embed Footer",
                help_text: Some("Optional embed footer template."),
                required: false,
                kind: SettingsFieldKind::Text,
            },
            SettingsField {
                key: keys.image,
                label: "Embed Image",
                help_text: Some("Optional embed image URL template."),
                required: false,
                kind: SettingsFieldKind::Url,
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
    #[serde(alias = "channel", deserialize_with = "deserialize_optional_snowflake")]
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

impl GreetingKind {
    fn default_message(self, display_name: &str, is_bot: bool) -> String {
        match self {
            Self::Welcome => {
                if is_bot {
                    format!("Welcome to the server, {display_name}.")
                } else {
                    format!("Welcome to the server, {display_name} 🎉")
                }
            }
            Self::Farewell => format!("Goodbye, {display_name} 👋"),
        }
    }
}

#[derive(Debug, Clone)]
struct GreetingSubject {
    user_id: Option<UserId>,
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
    subcommands("greeting_preview"),
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn greeting(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Preview a welcome or farewell message using the current guild settings.
#[poise::command(
    slash_command,
    guild_only,
    rename = "preview",
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn greeting_preview(
    ctx: Context<'_>,
    #[description = "Which greeting to preview"] kind: GreetingKind,
    #[description = "Optional member to preview with"] user: Option<User>,
) -> Result<(), Error> {
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
        let member_record = repo
            .get_or_create(guild_id.get(), &user.id.get().to_string())
            .await?;
        match member_record.invite_data.inviter.as_deref() {
            Some(inviter_id) => Some(repo.get_or_create(guild_id.get(), inviter_id).await?),
            None => None,
        }
    } else {
        None
    };

    let message = build_greeting_message(
        ctx.serenity_context(),
        kind,
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
    dispatch_greeting(
        ctx,
        GreetingKind::Welcome,
        &subject,
        &settings.welcome,
        inviter_data,
    )
    .await
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
    dispatch_greeting(
        ctx,
        GreetingKind::Farewell,
        &subject,
        &settings.farewell,
        inviter_data,
    )
    .await
}

async fn load_settings(data: &AppState, guild_id: u64) -> Result<GreetingSettings, Error> {
    let guild_settings = data.guild_settings_cached(guild_id).await?;
    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| decode_module_configuration::<GreetingSettings>(&module.configuration))
        .transpose()?
        .unwrap_or_default();
    Ok(settings)
}

async fn dispatch_greeting(
    ctx: &poise::serenity_prelude::Context,
    kind: GreetingKind,
    subject: &GreetingSubject,
    config: &GreetingSideConfig,
    inviter_data: Option<&InviteMemberRecord>,
) -> Result<(), Error> {
    let Some(channel_id) = config
        .channel_id
        .and_then(std::num::NonZeroU64::new)
        .map(ChannelId::from)
    else {
        return Ok(());
    };

    let message = build_greeting_message(ctx, kind, subject, config, inviter_data).await?;
    let mut allowed_mentions = CreateAllowedMentions::new();
    if let Some(user_id) = subject.user_id {
        allowed_mentions = allowed_mentions.users([user_id]);
    }
    let mut builder = CreateMessage::new().allowed_mentions(allowed_mentions);
    if let Some(content) = message.content {
        builder = builder.content(content);
    }
    if let Some(embed) = message.embed {
        builder = builder.embed(embed);
    }

    channel_id.send_message(ctx, builder).await?;
    Ok(())
}

struct BuiltGreetingMessage {
    content: Option<String>,
    embed: Option<CreateEmbed>,
}

async fn build_greeting_message(
    ctx: &poise::serenity_prelude::Context,
    kind: GreetingKind,
    subject: &GreetingSubject,
    config: &GreetingSideConfig,
    inviter_data: Option<&InviteMemberRecord>,
) -> Result<BuiltGreetingMessage, Error> {
    let inviter = resolve_inviter(ctx, subject.is_bot, inviter_data).await?;
    let content = config
        .content
        .as_deref()
        .map(str::trim)
        .filter(|template| !template.is_empty())
        .map(|template| render_template(template, subject, &inviter));

    let description = config
        .embed
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|template| render_template(template, subject, &inviter));
    let footer = config
        .embed
        .footer
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|template| render_template(template, subject, &inviter));
    let image = config
        .embed
        .image
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|template| render_template(template, subject, &inviter))
        .filter(|url| is_valid_http_url(url));

    let has_embed =
        description.is_some() || footer.is_some() || image.is_some() || config.embed.thumbnail;

    if !has_embed {
        return Ok(BuiltGreetingMessage {
            content: Some(
                content
                    .unwrap_or_else(|| kind.default_message(&subject.display_name, subject.is_bot)),
            ),
            embed: None,
        });
    }

    let mut embed = CreateEmbed::new();
    if let Some(description) = description {
        embed = embed.description(description);
    }
    if let Some(color) = config.embed.color.as_deref().and_then(parse_hex_color) {
        embed = embed.color(color);
    }
    if config.embed.thumbnail {
        embed = embed.thumbnail(&subject.avatar_url);
    }
    if let Some(footer) = footer {
        embed = embed.footer(CreateEmbedFooter::new(footer));
    }
    if let Some(image) = image {
        embed = embed.image(image);
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

fn is_valid_http_url(url: &str) -> bool {
    let trimmed = url.trim();
    (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
        && !trimmed.chars().any(char::is_whitespace)
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

    if let Some(user_id) = inviter_key
        .parse::<u64>()
        .ok()
        .and_then(std::num::NonZeroU64::new)
        .map(UserId::from)
        && let Ok(user) = user_id.to_user(ctx).await
    {
        return Ok(InviterRenderData {
            name: user.name.clone(),
            tag: user.tag(),
            invites: inviter_data.invite_data.effective(),
        });
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
    let member_count = subject.member_count.to_string();
    let invites = inviter.invites.to_string();
    let replacements = [
        ("\\n", "\n"),
        ("{server}", subject.guild_name.as_str()),
        ("{count}", member_count.as_str()),
        ("{member:nick}", subject.display_name.as_str()),
        ("{member:name}", subject.username.as_str()),
        ("{member:tag}", subject.tag.as_str()),
        ("{member:mention}", subject.mention.as_str()),
        ("{member:avatar}", subject.avatar_url.as_str()),
        ("{inviter:name}", inviter.name.as_str()),
        ("{inviter:tag}", inviter.tag.as_str()),
        ("{invites}", invites.as_str()),
    ];
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while !rest.is_empty() {
        if let Some((token, value)) = replacements
            .iter()
            .find(|(token, _)| rest.starts_with(token))
        {
            output.push_str(value);
            rest = &rest[token.len()..];
        } else if let Some(ch) = rest.chars().next() {
            output.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    output
}

fn parse_hex_color(value: &str) -> Option<u32> {
    let trimmed = value.trim().trim_start_matches('#');
    if trimmed.is_empty() || trimmed.len() > 6 {
        return None;
    }
    u32::from_str_radix(trimmed, 16)
        .ok()
        .filter(|&color| color <= 0xFF_FFFF)
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
        user_id: Some(member.user.id),
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
        user_id: Some(user.id),
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

#[cfg(test)]
mod tests {
    use super::GreetingSettings;

    #[test]
    fn greeting_settings_deserialize_string_and_null_channel_ids() {
        let settings: GreetingSettings = serde_json::from_value(serde_json::json!({
            "welcome": {
                "enabled": true,
                "channel": "123456789012345678"
            },
            "farewell": {
                "enabled": false,
                "channel": ""
            }
        }))
        .expect("greeting settings should deserialize");

        assert_eq!(settings.welcome.channel_id, Some(123_456_789_012_345_678));
        assert_eq!(settings.farewell.channel_id, None);
    }

    #[test]
    fn render_template_is_single_pass_for_member_placeholders() {
        let subject = super::GreetingSubject {
            user_id: None,
            guild_name: "SecretGuild".to_string(),
            guild_icon_url: None,
            member_count: 10,
            display_name: "Alice".to_string(),
            username: "{server}".to_string(),
            tag: "{server}#0001".to_string(),
            mention: "<@1>".to_string(),
            avatar_url: "https://example.com/a.png".to_string(),
            is_bot: false,
        };
        let inviter = super::InviterRenderData {
            name: "Bob".to_string(),
            tag: "Bob#0001".to_string(),
            invites: 2,
        };

        assert_eq!(
            super::render_template("Hello {member:name} in {server}", &subject, &inviter),
            "Hello {server} in SecretGuild"
        );
    }

    #[test]
    fn parse_hex_color_and_farewell_default_behave_safely() {
        assert_eq!(super::parse_hex_color("#FFFFFF"), Some(0xFF_FFFF));
        assert_eq!(super::parse_hex_color("068ADD"), Some(0x06_8ADD));
        assert_eq!(super::parse_hex_color("#1234567"), None);
        assert!(
            super::GreetingKind::Farewell
                .default_message("Alice", false)
                .starts_with("Goodbye")
        );
    }
}
