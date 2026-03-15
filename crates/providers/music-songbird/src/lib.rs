use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use dynamo_core::{
    Error, MusicBackendConfig, MusicBackendKind, MusicBackendStatus, MusicEnqueueResult,
    MusicQueueSnapshot, MusicService, MusicTrack,
};
use poise::serenity_prelude::{ChannelId, Context, GuildId};
use songbird::{
    input::{Compose, YoutubeDl},
    serenity::get as get_songbird,
    tracks::{PlayMode, Track},
};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Default)]
struct GuildVoiceContext {
    voice_channel_id: Option<u64>,
    text_channel_id: Option<u64>,
}

fn guild_contexts() -> &'static Mutex<HashMap<u64, GuildVoiceContext>> {
    static CONTEXTS: OnceLock<Mutex<HashMap<u64, GuildVoiceContext>>> = OnceLock::new();
    CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct SongbirdMusicService {
    client: reqwest::Client,
}

impl SongbirdMusicService {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    fn ensure_backend(config: &MusicBackendConfig) -> Result<(), Error> {
        if config.backend == MusicBackendKind::Songbird {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "The configured music backend is not implemented in this build yet."
            ))
        }
    }

    async fn manager(
        &self,
        ctx: &Context,
        config: &MusicBackendConfig,
    ) -> Result<Arc<songbird::Songbird>, Error> {
        Self::ensure_backend(config)?;
        get_songbird(ctx)
            .await
            .ok_or_else(|| anyhow::anyhow!("Songbird is not registered on this client."))
    }

    async fn snapshot_for_guild(
        &self,
        ctx: &Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error> {
        Self::ensure_backend(config)?;
        let manager = self.manager(ctx, config).await?;
        let Some(call) = manager.get(GuildId::new(guild_id)) else {
            let context = guild_contexts()
                .lock()
                .await
                .get(&guild_id)
                .cloned()
                .unwrap_or_default();
            return Ok(MusicQueueSnapshot {
                backend: MusicBackendKind::Songbird,
                connected: false,
                voice_channel_id: context.voice_channel_id,
                text_channel_id: context.text_channel_id,
                paused: false,
                current: None,
                upcoming: Vec::new(),
            });
        };

        let queue_handles = {
            let handler = call.lock().await;
            handler.queue().current_queue()
        };
        let context = guild_contexts()
            .lock()
            .await
            .get(&guild_id)
            .cloned()
            .unwrap_or_default();

        let mut current = None;
        let mut upcoming = Vec::new();
        let mut paused = false;

        for (index, handle) in queue_handles.into_iter().enumerate() {
            let track = (*handle.data::<MusicTrack>()).clone();
            if index == 0 {
                if let Ok(info) = handle.get_info().await {
                    paused = matches!(info.playing, PlayMode::Pause);
                }
                current = Some(track);
            } else {
                upcoming.push(track);
            }
        }

        Ok(MusicQueueSnapshot {
            backend: MusicBackendKind::Songbird,
            connected: true,
            voice_channel_id: context.voice_channel_id,
            text_channel_id: context.text_channel_id,
            paused,
            current,
            upcoming,
        })
    }

    fn build_lazy_input(&self, query: &str) -> YoutubeDl<'static> {
        let is_url = query.starts_with("http://") || query.starts_with("https://");

        if is_url {
            YoutubeDl::new(self.client.clone(), query.to_string())
        } else {
            YoutubeDl::new_search(self.client.clone(), query.to_string())
        }
        .user_args(vec!["--no-playlist".to_string()])
    }
}

impl Default for SongbirdMusicService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MusicService for SongbirdMusicService {
    async fn status(&self, config: &MusicBackendConfig) -> Result<MusicBackendStatus, Error> {
        Ok(match config.backend {
            MusicBackendKind::Songbird => MusicBackendStatus {
                backend: MusicBackendKind::Songbird,
                healthy: true,
                summary: "Songbird backend is available and ready for in-process playback."
                    .to_string(),
            },
            MusicBackendKind::Lavalink => MusicBackendStatus {
                backend: MusicBackendKind::Lavalink,
                healthy: false,
                summary: "Lavalink is configured but not wired into the runtime yet.".to_string(),
            },
        })
    }

    async fn join(
        &self,
        ctx: &Context,
        guild_id: u64,
        voice_channel_id: u64,
        text_channel_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error> {
        let manager = self.manager(ctx, config).await?;
        tokio::time::timeout(
            Duration::from_secs(15),
            manager.join(GuildId::new(guild_id), ChannelId::new(voice_channel_id)),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Timed out while joining the voice channel. Check that the bot can view, connect, and speak in the target channel."
            )
        })?
        .map_err(|error| {
            anyhow::anyhow!(
                "Failed to join the voice channel. Check that the bot can view, connect, and speak in the target channel: {error}"
            )
        })?;

        guild_contexts().lock().await.insert(
            guild_id,
            GuildVoiceContext {
                voice_channel_id: Some(voice_channel_id),
                text_channel_id: Some(text_channel_id),
            },
        );

        self.snapshot_for_guild(ctx, guild_id, config).await
    }

    async fn leave(
        &self,
        ctx: &Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<(), Error> {
        let manager = self.manager(ctx, config).await?;
        manager.remove(GuildId::new(guild_id)).await?;
        guild_contexts().lock().await.remove(&guild_id);
        Ok(())
    }

    async fn play(
        &self,
        ctx: &Context,
        guild_id: u64,
        voice_channel_id: u64,
        text_channel_id: u64,
        query: &str,
        requested_by: &str,
        config: &MusicBackendConfig,
    ) -> Result<MusicEnqueueResult, Error> {
        let manager = self.manager(ctx, config).await?;
        let call = tokio::time::timeout(
            Duration::from_secs(15),
            manager.join(GuildId::new(guild_id), ChannelId::new(voice_channel_id)),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Timed out while joining the voice channel. Check that the bot can view, connect, and speak in the target channel."
            )
        })?
        .map_err(|error| {
            anyhow::anyhow!(
                "Failed to join the voice channel. Check that the bot can view, connect, and speak in the target channel: {error}"
            )
        })?;

        guild_contexts().lock().await.insert(
            guild_id,
            GuildVoiceContext {
                voice_channel_id: Some(voice_channel_id),
                text_channel_id: Some(text_channel_id),
            },
        );

        let mut lazy = self.build_lazy_input(query);
        let aux_metadata = tokio::time::timeout(Duration::from_secs(20), lazy.aux_metadata())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Timed out while resolving media metadata. Check that yt-dlp is installed and the requested source is reachable."
                )
            })?
            .map_err(|error| anyhow::anyhow!("Failed to resolve media metadata: {error}"))?;

        let track = MusicTrack {
            title: aux_metadata
                .title
                .clone()
                .or(aux_metadata.track.clone())
                .unwrap_or_else(|| query.to_string()),
            url: aux_metadata.source_url.clone(),
            duration_seconds: aux_metadata
                .duration
                .map(|duration: std::time::Duration| duration.as_secs()),
            requested_by: requested_by.to_string(),
            source: config.default_source.clone(),
        };

        let started_immediately = {
            let mut handler = call.lock().await;
            let started_now = handler.queue().current_queue().is_empty();
            let songbird_track =
                Track::new_with_data(lazy.into(), Arc::new(track.clone()) as Arc<_>);
            handler.enqueue(songbird_track).await;
            started_now
        };

        let snapshot = self.snapshot_for_guild(ctx, guild_id, config).await?;
        Ok(MusicEnqueueResult {
            started_immediately,
            track,
            snapshot,
        })
    }

    async fn pause(
        &self,
        ctx: &Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error> {
        let manager = self.manager(ctx, config).await?;
        let call = manager
            .get(GuildId::new(guild_id))
            .ok_or_else(|| anyhow::anyhow!("There is no active music session in this guild."))?;
        call.lock().await.queue().pause()?;
        self.snapshot_for_guild(ctx, guild_id, config).await
    }

    async fn resume(
        &self,
        ctx: &Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error> {
        let manager = self.manager(ctx, config).await?;
        let call = manager
            .get(GuildId::new(guild_id))
            .ok_or_else(|| anyhow::anyhow!("There is no active music session in this guild."))?;
        call.lock().await.queue().resume()?;
        self.snapshot_for_guild(ctx, guild_id, config).await
    }

    async fn skip(
        &self,
        ctx: &Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error> {
        let manager = self.manager(ctx, config).await?;
        let call = manager
            .get(GuildId::new(guild_id))
            .ok_or_else(|| anyhow::anyhow!("There is no active music session in this guild."))?;
        call.lock().await.queue().skip()?;
        self.snapshot_for_guild(ctx, guild_id, config).await
    }

    async fn stop(
        &self,
        ctx: &Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error> {
        let manager = self.manager(ctx, config).await?;
        let call = manager
            .get(GuildId::new(guild_id))
            .ok_or_else(|| anyhow::anyhow!("There is no active music session in this guild."))?;
        call.lock().await.queue().stop();
        self.snapshot_for_guild(ctx, guild_id, config).await
    }

    async fn queue(
        &self,
        ctx: &Context,
        guild_id: u64,
        config: &MusicBackendConfig,
    ) -> Result<MusicQueueSnapshot, Error> {
        self.snapshot_for_guild(ctx, guild_id, config).await
    }
}
