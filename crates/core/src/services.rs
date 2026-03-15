use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Error;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StockQuote {
    pub symbol: String,
    pub short_name: Option<String>,
    pub long_name: Option<String>,
    pub quote_type: Option<String>,
    pub exchange_name: Option<String>,
    pub currency_label: String,
    pub phase: String,
    pub regular_market_price: Option<f64>,
    pub regular_market_change: Option<f64>,
    pub regular_market_change_percent: Option<f64>,
    pub pre_market_price: Option<f64>,
    pub pre_market_change: Option<f64>,
    pub pre_market_change_percent: Option<f64>,
    pub post_market_price: Option<f64>,
    pub post_market_change: Option<f64>,
    pub post_market_change_percent: Option<f64>,
    pub regular_market_day_high: Option<f64>,
    pub regular_market_day_low: Option<f64>,
    pub regular_market_volume: Option<f64>,
    pub market_cap: Option<f64>,
    pub trailing_pe: Option<f64>,
    pub forward_pe: Option<f64>,
    pub trailing_eps: Option<f64>,
    pub dividend_yield: Option<f64>,
    pub fifty_two_week_high: Option<f64>,
    pub fifty_two_week_low: Option<f64>,
    pub sector: Option<String>,
    pub industry: Option<String>,
}

#[async_trait]
pub trait StockQuoteService: Send + Sync {
    async fn fetch_quote(&self, symbol: &str) -> Result<Option<StockQuote>, Error>;
    async fn fetch_quotes(
        &self,
        symbols: &[String],
    ) -> Result<Vec<Result<StockQuote, String>>, Error>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MusicBackendKind {
    #[default]
    Songbird,
    Lavalink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicBackendStatus {
    pub backend: MusicBackendKind,
    pub healthy: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MusicBackendConfig {
    pub backend: MusicBackendKind,
    pub default_source: String,
    pub auto_leave_seconds: u64,
    pub songbird_ytdlp_program: Option<String>,
    pub lavalink_host: Option<String>,
    pub lavalink_port: Option<u16>,
    pub lavalink_password: Option<String>,
    pub lavalink_secure: bool,
    pub lavalink_resume_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MusicTrack {
    pub title: String,
    pub url: Option<String>,
    pub duration_seconds: Option<u64>,
    pub requested_by: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MusicQueueSnapshot {
    pub backend: MusicBackendKind,
    pub connected: bool,
    pub voice_channel_id: Option<u64>,
    pub text_channel_id: Option<u64>,
    pub paused: bool,
    pub current: Option<MusicTrack>,
    pub upcoming: Vec<MusicTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicEnqueueResult {
    pub started_immediately: bool,
    pub track: MusicTrack,
    pub snapshot: MusicQueueSnapshot,
}

#[async_trait]
pub trait MusicService: Send + Sync {
    async fn status(&self, config: &MusicBackendConfig) -> Result<MusicBackendStatus, Error>;
    async fn join(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        voice_channel_id: u64,
        text_channel_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn leave(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<(), Error>;
    async fn play(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        voice_channel_id: u64,
        text_channel_id: u64,
        query: &str,
        requested_by: &str,
        config: &MusicBackendConfig,
    ) -> Result<MusicEnqueueResult, Error>;
    async fn pause(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn resume(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn skip(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn stop(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
    async fn queue(
        &self,
        ctx: &poise::serenity_prelude::Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error>;
}

#[derive(Clone, Default)]
pub struct ServiceRegistry {
    pub stock_quotes: Option<Arc<dyn StockQuoteService>>,
    pub music: Option<Arc<dyn MusicService>>,
}

impl ServiceRegistry {
    pub fn new(
        stock_quotes: Option<Arc<dyn StockQuoteService>>,
        music: Option<Arc<dyn MusicService>>,
    ) -> Self {
        Self {
            stock_quotes,
            music,
        }
    }
}
