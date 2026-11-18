use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use dynamo_module_kit::{CommandCatalog, ModuleCatalog};
use dynamo_persistence_api::Persistence;
use dynamo_services_api::ServiceRegistry;
use dynamo_settings::{DeploymentSettings, GuildSettings};
pub use dynamo_settings::{
    decode_module_configuration, deserialize_optional_snowflake, deserialize_snowflake_vec,
    serialize_optional_snowflake, serialize_snowflake_vec,
};
use poise::Context as PoiseContext;
use tokio::sync::RwLock;

pub type Error = anyhow::Error;
pub type Context<'a> = PoiseContext<'a, AppState, Error>;

pub fn is_korean_locale(locale: Option<&str>) -> bool {
    locale
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case("ko") || value.starts_with("ko-"))
}

const SETTINGS_CACHE_TTL: Duration = Duration::from_secs(5);
const MAX_CACHED_GUILDS: usize = 2048;

#[derive(Clone)]
struct TimedEntry<T> {
    value: T,
    fetched_at: Instant,
}

#[derive(Default)]
struct SettingsCache {
    deployment: Option<TimedEntry<DeploymentSettings>>,
    guilds: HashMap<u64, TimedEntry<GuildSettings>>,
}

#[derive(Clone)]
pub struct AppState {
    pub started_at: Instant,
    pub module_catalog: ModuleCatalog,
    pub command_catalog: CommandCatalog,
    pub persistence: Persistence,
    pub services: ServiceRegistry,
    settings_cache: Arc<RwLock<SettingsCache>>,
}

impl AppState {
    pub fn new(
        module_catalog: ModuleCatalog,
        command_catalog: CommandCatalog,
        persistence: Persistence,
        services: ServiceRegistry,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            module_catalog,
            command_catalog,
            persistence,
            services,
            settings_cache: Arc::new(RwLock::new(SettingsCache::default())),
        }
    }

    pub async fn deployment_settings_cached(&self) -> Result<DeploymentSettings, Error> {
        {
            let cache = self.settings_cache.read().await;
            if let Some(entry) = &cache.deployment
                && entry.fetched_at.elapsed() < SETTINGS_CACHE_TTL
            {
                return Ok(entry.value.clone());
            }
        }

        let fresh = self.persistence.deployment_settings_or_default().await?;
        let mut cache = self.settings_cache.write().await;
        cache.deployment = Some(TimedEntry {
            value: fresh.clone(),
            fetched_at: Instant::now(),
        });
        Ok(fresh)
    }

    pub async fn guild_settings_cached(&self, guild_id: u64) -> Result<GuildSettings, Error> {
        {
            let cache = self.settings_cache.read().await;
            if let Some(entry) = cache.guilds.get(&guild_id)
                && entry.fetched_at.elapsed() < SETTINGS_CACHE_TTL
            {
                return Ok(entry.value.clone());
            }
        }

        let fresh = self.persistence.guild_settings_or_default(guild_id).await?;
        let mut cache = self.settings_cache.write().await;
        if cache.guilds.len() >= MAX_CACHED_GUILDS {
            cache
                .guilds
                .retain(|_, entry| entry.fetched_at.elapsed() < SETTINGS_CACHE_TTL);
            if cache.guilds.len() >= MAX_CACHED_GUILDS
                && let Some(oldest_key) = cache
                    .guilds
                    .iter()
                    .min_by_key(|(_, entry)| entry.fetched_at)
                    .map(|(key, _)| *key)
            {
                cache.guilds.remove(&oldest_key);
            }
        }
        cache.guilds.insert(
            guild_id,
            TimedEntry {
                value: fresh.clone(),
                fetched_at: Instant::now(),
            },
        );
        Ok(fresh)
    }

    pub async fn invalidate_guild_settings_cache(&self, guild_id: u64) {
        self.settings_cache.write().await.guilds.remove(&guild_id);
    }

    pub async fn invalidate_deployment_settings_cache(&self) {
        self.settings_cache.write().await.deployment = None;
    }
}
