use std::env;

use crate::Error;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub discord: DiscordConfig,
    pub commands: CommandSyncConfig,
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
            commands: CommandSyncConfig {
                sync_interval_seconds: parse_u64_env("DISCORD_COMMAND_SYNC_INTERVAL_SECONDS", 15)?,
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

#[derive(Debug, Clone)]
pub struct CommandSyncConfig {
    pub sync_interval_seconds: u64,
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

fn parse_u64_env(key: &str, default: u64) -> Result<u64, Error> {
    match env::var(key) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|error| anyhow::anyhow!("{key} must be a valid u64: {error}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(anyhow::anyhow!("{key} could not be read: {error}")),
    }
}
