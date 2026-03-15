use std::{env, time::Instant};

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, AppState, Error>;
pub type DiscordCommand = poise::Command<AppState, Error>;
pub type GatewayIntents = poise::serenity_prelude::GatewayIntents;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub discord: DiscordConfig,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, Error> {
        let token = env::var("DISCORD_TOKEN")
            .or_else(|_| env::var("BOT_TOKEN"))
            .map_err(|_| anyhow::anyhow!("DISCORD_TOKEN or BOT_TOKEN must be set"))?;

        let register_globally = parse_bool_env("DISCORD_REGISTER_GLOBALLY", true)?;
        let dev_guild_id = env::var("DISCORD_DEV_GUILD_ID")
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|error| {
                anyhow::anyhow!("DISCORD_DEV_GUILD_ID must be a valid u64: {error}")
            })?;

        if !register_globally && dev_guild_id.is_none() {
            anyhow::bail!(
                "DISCORD_DEV_GUILD_ID is required when DISCORD_REGISTER_GLOBALLY is false"
            );
        }

        Ok(Self {
            discord: DiscordConfig {
                token,
                register_globally,
                dev_guild_id,
            },
        })
    }
}

#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub token: String,
    pub register_globally: bool,
    pub dev_guild_id: Option<u64>,
}

fn parse_bool_env(key: &str, default: bool) -> Result<bool, Error> {
    match env::var(key) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => anyhow::bail!("{key} must be one of true/false/1/0/yes/no/on/off"),
        },
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(anyhow::anyhow!("{key} could not be read: {error}")),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModuleManifest {
    pub id: &'static str,
    pub display_name: &'static str,
    pub enabled_by_default: bool,
    pub required_intents: GatewayIntents,
}

impl ModuleManifest {
    pub const fn new(
        id: &'static str,
        display_name: &'static str,
        enabled_by_default: bool,
        required_intents: GatewayIntents,
    ) -> Self {
        Self {
            id,
            display_name,
            enabled_by_default,
            required_intents,
        }
    }
}

pub trait Module: Send + Sync {
    fn manifest(&self) -> ModuleManifest;
    fn commands(&self) -> Vec<DiscordCommand>;
}

#[derive(Debug)]
pub struct AppState {
    pub started_at: Instant,
    pub modules: Vec<ModuleManifest>,
}

impl AppState {
    pub fn new(modules: Vec<ModuleManifest>) -> Self {
        Self {
            started_at: Instant::now(),
            modules,
        }
    }
}

pub fn aggregate_intents(manifests: impl IntoIterator<Item = ModuleManifest>) -> GatewayIntents {
    manifests
        .into_iter()
        .fold(GatewayIntents::GUILDS, |intents, manifest| {
            intents | manifest.required_intents
        })
}
