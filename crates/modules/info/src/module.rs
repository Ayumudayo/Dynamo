use std::{collections::BTreeMap, time::Instant};

use dynamo_enablement::resolve_command_states;
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsSchema,
};
use dynamo_runtime_api::{AppState, Context, Error};
use poise::{
    CreateReply,
    serenity_prelude::{CreateEmbed, CreateEmbedFooter},
};

const INFO_EMBED_COLOR: u32 = 0x5865F2;

pub struct InfoModule;

impl Module<AppState, Error> for InfoModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            "info",
            "Info",
            "Read-only commands that describe the bot and runtime.",
            ModuleCategory::Info,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![ping(), about(), help()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::empty()
    }
}

fn format_uptime(total_seconds: u64) -> String {
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m {seconds}s")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// Check gateway and REST response latency.
#[poise::command(slash_command, category = "Info")]
async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
    let shard_latency = ctx.ping().await;
    let started = Instant::now();
    let handle = ctx
        .send(
            CreateReply::default().embed(
                CreateEmbed::new()
                    .title("Pong!")
                    .description(if ko {
                        "지연 시간 측정 중..."
                    } else {
                        "Measuring latency..."
                    })
                    .color(INFO_EMBED_COLOR),
            ),
        )
        .await?;
    let round_trip_ms = started.elapsed().as_millis();
    let shard_text = if shard_latency.is_zero() {
        if ko { "준비 중" } else { "Warming up" }.to_string()
    } else {
        format!("{} ms", shard_latency.as_millis())
    };
    handle
        .edit(
            ctx,
            CreateReply::default().embed(
                CreateEmbed::new()
                    .title("Pong!")
                    .color(INFO_EMBED_COLOR)
                    .field(
                        if ko {
                            "REST 왕복 지연"
                        } else {
                            "REST Round-Trip"
                        },
                        format!("{round_trip_ms} ms"),
                        true,
                    )
                    .field(
                        if ko {
                            "게이트웨이 하트비트"
                        } else {
                            "Gateway Heartbeat"
                        },
                        shard_text,
                        true,
                    ),
            ),
        )
        .await?;
    Ok(())
}

/// Show runtime details such as version, loaded modules, uptime, and persistence status.
#[poise::command(slash_command, category = "Info")]
async fn about(ctx: Context<'_>) -> Result<(), Error> {
    let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
    let data = ctx.data();
    let uptime = format_uptime(data.started_at.elapsed().as_secs());
    let module_names = data
        .module_catalog
        .entries
        .iter()
        .map(|entry| entry.module.display_name)
        .collect::<Vec<_>>()
        .join(", ");
    let persistence_status = if data.persistence.database_name.is_some() {
        if ko { "연결됨" } else { "Connected" }
    } else if ko {
        "인메모리 / 비활성"
    } else {
        "In-memory / Disabled"
    };

    let embed = CreateEmbed::new()
        .title("Dynamo Bot")
        .description(if ko {
            "Poise와 Serenity로 구동되는 모듈형 Rust Discord 봇입니다."
        } else {
            "Modular Rust Discord bot built with Poise and Serenity."
        })
        .color(INFO_EMBED_COLOR)
        .field(
            if ko { "버전" } else { "Version" },
            env!("CARGO_PKG_VERSION"),
            true,
        )
        .field(if ko { "가동 시간" } else { "Uptime" }, uptime, true)
        .field(
            if ko { "저장소" } else { "Persistence" },
            persistence_status,
            true,
        )
        .field(
            if ko {
                format!("모듈 ({})", data.module_catalog.entries.len())
            } else {
                format!("Modules ({})", data.module_catalog.entries.len())
            },
            module_names,
            false,
        )
        .field(
            if ko { "명령어" } else { "Commands" },
            data.command_catalog.entries.len().to_string(),
            true,
        )
        .footer(CreateEmbedFooter::new("Dynamo Runtime"));

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}

async fn autocomplete_module(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let needle = partial.trim().to_ascii_lowercase();
    ctx.data()
        .module_catalog
        .entries
        .iter()
        .filter_map(|entry| {
            let id = entry.module.id;
            let name = entry.module.display_name;
            if needle.is_empty()
                || id.to_ascii_lowercase().contains(&needle)
                || name.to_ascii_lowercase().contains(&needle)
            {
                Some(id.to_string())
            } else {
                None
            }
        })
        .take(25)
        .collect()
}

/// List commands currently enabled for this server.
#[poise::command(slash_command, category = "Info")]
async fn help(
    ctx: Context<'_>,
    #[description = "Optional module ID to filter commands"]
    #[autocomplete = "autocomplete_module"]
    module: Option<String>,
) -> Result<(), Error> {
    let ko = dynamo_runtime_api::is_korean_locale(ctx.locale());
    let data = ctx.data();
    let deployment = data.deployment_settings_cached().await?;
    let guild_settings = if let Some(guild_id) = ctx.guild_id() {
        Some(data.guild_settings_cached(guild_id.get()).await?)
    } else {
        None
    };

    let filter = module
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);

    let resolved = resolve_command_states(
        &data.module_catalog,
        &data.command_catalog,
        &deployment,
        guild_settings.as_ref(),
    );

    let mut grouped: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for state in resolved {
        if !state.effective_enabled {
            continue;
        }
        if let Some(ref filter) = filter
            && state.command.module_id.to_ascii_lowercase() != *filter
            && state.command.module_display_name.to_ascii_lowercase() != *filter
        {
            continue;
        }
        let desc = state.command.description.as_deref().unwrap_or(if ko {
            "설명 없음."
        } else {
            "No description."
        });
        grouped
            .entry(state.command.module_display_name)
            .or_default()
            .push(format!("`/{}` — {}", state.command.qualified_name, desc));
    }

    if grouped.is_empty() {
        ctx.send(
            CreateReply::default()
                .content(if ko {
                    "조건에 맞는 활성 명령어가 없습니다."
                } else {
                    "No enabled commands matched that filter."
                })
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let mut embed = CreateEmbed::new()
        .title(if ko {
            "사용 가능한 명령어"
        } else {
            "Available Commands"
        })
        .color(INFO_EMBED_COLOR);
    for (module_name, lines) in grouped.into_iter().take(25) {
        let mut value = lines.join("\n");
        if value.chars().count() > 1024 {
            value = value.chars().take(1021).collect::<String>() + "...";
        }
        embed = embed.field(module_name, value, false);
    }

    ctx.send(CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}
