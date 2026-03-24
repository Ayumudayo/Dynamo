use crate::constants::{MODULE_ID, SUCCESS_EMBED_COLOR};
use dynamo_module_kit::{SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection};
use dynamo_runtime_api::{Context, Error};
use serde::{Deserialize, Serialize};

const DEFAULT_WT_LINK: &str = "http://warthunder.com/en/registration?r=userinvite_18945695";
const DEFAULT_WOT_LINK: &str =
    "https://worldoftanks.asia/referral/9ed8df012d204670b04c1cc1c88d98d5";
const DEFAULT_THUMBNAIL_URL: &str = "https://media.discordapp.net/attachments/1138398345065414657/1329005700730585118/png-clipart-war-thunder-playstation-4-aircraft-airplane-macchi-c-202-thunder-game-video-game-removebg-preview.png?ex=6788c482&is=67877302&hm=31b9ed755040306ea8d1c9db258ffaa590df7e3bfa6139d875c62915d46c1b73&=&format=webp&quality=lossless";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct GameInfoSettings {
    pub(crate) title: String,
    pub(crate) wt_link: String,
    pub(crate) wot_link: String,
    pub(crate) thumbnail_url: String,
}

impl Default for GameInfoSettings {
    fn default() -> Self {
        Self {
            title: "Join War Thunder / World of Tanks Now!".to_string(),
            wt_link: DEFAULT_WT_LINK.to_string(),
            wot_link: DEFAULT_WOT_LINK.to_string(),
            thumbnail_url: DEFAULT_THUMBNAIL_URL.to_string(),
        }
    }
}

pub(crate) fn settings_schema() -> SettingsSchema {
    SettingsSchema {
        sections: vec![SettingsSection {
            id: "referrals",
            title: "Referral Links",
            description: Some("Customize the links and artwork used by /wtinv."),
            fields: vec![
                SettingsField {
                    key: "title",
                    label: "Embed title",
                    help_text: Some("Displayed at the top of the /wtinv embed."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "wt_link",
                    label: "War Thunder link",
                    help_text: Some("Referral URL for the War Thunder button."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "wot_link",
                    label: "World of Tanks link",
                    help_text: Some("Referral URL for the World of Tanks button."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
                SettingsField {
                    key: "thumbnail_url",
                    label: "Thumbnail URL",
                    help_text: Some("Thumbnail image shown in the /wtinv embed."),
                    required: false,
                    kind: SettingsFieldKind::Text,
                },
            ],
        }],
    }
}

pub(crate) async fn load_settings(ctx: Context<'_>) -> Result<GameInfoSettings, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(GameInfoSettings::default());
    };

    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;

    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(|module| serde_json::from_value::<GameInfoSettings>(module.configuration.clone()))
        .transpose()?
        .unwrap_or_default();

    Ok(settings)
}

pub(crate) fn build_referral_reply(
    settings: &GameInfoSettings,
) -> poise::CreateReply {
    use poise::serenity_prelude::{CreateActionRow, CreateButton, CreateEmbed, Timestamp};

    let embed = CreateEmbed::new()
        .title(settings.title.clone())
        .color(SUCCESS_EMBED_COLOR)
        .timestamp(Timestamp::now());

    let embed = if settings.thumbnail_url.trim().is_empty() {
        embed
    } else {
        embed.thumbnail(settings.thumbnail_url.clone())
    };

    let mut buttons = Vec::new();
    if let Some(url) = to_valid_url(&settings.wt_link) {
        buttons.push(CreateButton::new_link(url).label("War Thunder"));
    }
    if let Some(url) = to_valid_url(&settings.wot_link) {
        buttons.push(CreateButton::new_link(url).label("World of Tanks"));
    }

    if buttons.is_empty() {
        poise::CreateReply::default().embed(embed.description("No invite links are currently configured."))
    } else {
        poise::CreateReply::default()
            .embed(embed)
            .components(vec![CreateActionRow::Buttons(buttons)])
    }
}

pub(crate) fn to_valid_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    url::Url::parse(trimmed).ok().map(|value| value.to_string())
}
