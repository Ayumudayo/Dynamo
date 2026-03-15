use dynamo_core::{
    Context, DiscordCommand, Error, GatewayIntents, Module, ModuleCategory, ModuleManifest,
    SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
use poise::serenity_prelude::CreateEmbed;
use serde::{Deserialize, Serialize};

const MODULE_ID: &str = "gameinfo";
const DEFAULT_WT_LINK: &str = "http://warthunder.com/en/registration?r=userinvite_18945695";
const DEFAULT_WOT_LINK: &str =
    "https://worldoftanks.asia/referral/9ed8df012d204670b04c1cc1c88d98d5";
const DEFAULT_THUMBNAIL_URL: &str = "https://media.discordapp.net/attachments/1138398345065414657/1329005700730585118/png-clipart-war-thunder-playstation-4-aircraft-airplane-macchi-c-202-thunder-game-video-game-removebg-preview.png?ex=6788c482&is=67877302&hm=31b9ed755040306ea8d1c9db258ffaa590df7e3bfa6139d875c62915d46c1b73&=&format=webp&quality=lossless";

pub struct GameInfoModule;

impl Module for GameInfoModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Game Info",
            "Game utility commands and referral links.",
            ModuleCategory::GameInfo,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![wtinv()]
    }

    fn settings_schema(&self) -> SettingsSchema {
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct GameInfoSettings {
    title: String,
    wt_link: String,
    wot_link: String,
    thumbnail_url: String,
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

#[poise::command(slash_command, guild_only, category = "Game Info")]
async fn wtinv(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_disable_reason(ctx).await? {
        ctx.say(reason).await?;
        return Ok(());
    }

    let settings = load_settings(ctx).await?;
    let mut embed = CreateEmbed::new()
        .title(settings.title)
        .field(
            "War Thunder",
            format!("[Open referral link]({})", settings.wt_link),
            false,
        )
        .field(
            "World of Tanks",
            format!("[Open referral link]({})", settings.wot_link),
            false,
        );

    if !settings.thumbnail_url.trim().is_empty() {
        embed = embed.thumbnail(settings.thumbnail_url);
    }

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

async fn load_settings(ctx: Context<'_>) -> Result<GameInfoSettings, Error> {
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

async fn module_disable_reason(ctx: Context<'_>) -> Result<Option<String>, Error> {
    let deployment = ctx
        .data()
        .persistence
        .deployment_settings_or_default()
        .await?;
    if let Some(module) = deployment.modules.get(MODULE_ID) {
        if !module.installed {
            return Ok(Some(
                "The `gameinfo` module is not installed for this deployment.".to_string(),
            ));
        }
        if !module.enabled {
            return Ok(Some(
                "The `gameinfo` module is disabled for this deployment.".to_string(),
            ));
        }
    }

    let Some(guild_id) = ctx.guild_id() else {
        return Ok(None);
    };

    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;
    if let Some(module) = guild_settings.modules.get(MODULE_ID) {
        if !module.enabled {
            return Ok(Some(
                "The `gameinfo` module is disabled for this guild.".to_string(),
            ));
        }
    }

    Ok(None)
}
