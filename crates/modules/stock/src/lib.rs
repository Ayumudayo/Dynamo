use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};

use dynamo_core::{
    Context, DiscordCommand, Error, GatewayIntents, GuildModuleSettings, Module, ModuleCategory,
    ModuleManifest, SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection, StockQuote,
    StockQuoteService,
};
use poise::serenity_prelude::{
    ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateButton, CreateEmbed,
    CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, EditMessage,
    Interaction,
};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{Mutex, RwLock},
    time::sleep,
};

const MODULE_ID: &str = "stock";
const DEFAULT_SYMBOL: &str = "NVDA";
const DEFAULT_ETF_TICKERS: [&str; 3] = ["SOXL", "TQQQ", "VOO"];
const DEFAULT_EMBED_COLOR: u32 = 0x4F545C;
const UPWARD_EMBED_COLOR: u32 = 0x43B581;
const DOWNWARD_EMBED_COLOR: u32 = 0xF04747;
const REFRESH_INTERVAL_MS: u32 = 5_000;
const MAX_REFRESH_TIME_MS: u32 = 60_000;
const MAX_MANUAL_REFRESHES: u32 = 5;
const MAX_STORED_SESSIONS: usize = 200;
pub const STOCK_REFRESH_BUTTON_ID: &str = "stock_refresh";

#[derive(Debug, Clone)]
enum SessionKind {
    Stock { symbol: String },
    Etf { tickers: Vec<String> },
}

struct StockSession {
    kind: SessionKind,
    service: Arc<dyn StockQuoteService>,
    active: bool,
    generation: u64,
    manual_restart_in_progress: bool,
    manual_refresh_count: u32,
    last_stop_reason: Option<&'static str>,
}

impl StockSession {
    fn new(kind: SessionKind, service: Arc<dyn StockQuoteService>) -> Self {
        Self {
            kind,
            service,
            active: false,
            generation: 0,
            manual_restart_in_progress: false,
            manual_refresh_count: 0,
            last_stop_reason: None,
        }
    }
}

#[derive(Debug, Clone)]
struct StockResponse {
    embed: CreateEmbed,
    stop_reason: Option<&'static str>,
}

fn stock_sessions() -> &'static RwLock<HashMap<u64, Arc<Mutex<StockSession>>>> {
    static SESSIONS: OnceLock<RwLock<HashMap<u64, Arc<Mutex<StockSession>>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub struct StockModule;

impl Module for StockModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest::new(
            MODULE_ID,
            "Stock",
            "Stock and ETF quote commands with refresh sessions.",
            ModuleCategory::Stocks,
            true,
            GatewayIntents::GUILDS,
        )
    }

    fn commands(&self) -> Vec<DiscordCommand> {
        vec![stock(), etf()]
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema {
            sections: vec![SettingsSection {
                id: "quotes",
                title: "Quotes",
                description: Some("Customize stock defaults and ETF ticker groups."),
                fields: vec![
                    SettingsField {
                        key: "default_symbol",
                        label: "Default stock symbol",
                        help_text: Some("Used by /stock when no symbol is supplied."),
                        required: false,
                        kind: SettingsFieldKind::Text,
                    },
                    SettingsField {
                        key: "etf_tickers",
                        label: "ETF tickers",
                        help_text: Some("Array of ETF tickers used by /etf."),
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
struct StockSettings {
    default_symbol: String,
    etf_tickers: Vec<String>,
}

impl Default for StockSettings {
    fn default() -> Self {
        Self {
            default_symbol: DEFAULT_SYMBOL.to_string(),
            etf_tickers: DEFAULT_ETF_TICKERS
                .iter()
                .map(|value| value.to_string())
                .collect(),
        }
    }
}

#[poise::command(slash_command, guild_only, category = "Stock")]
async fn stock(
    ctx: Context<'_>,
    #[description = "Symbol of the stock"] symbol: Option<String>,
) -> Result<(), Error> {
    if let Some(reason) = module_disable_reason(ctx).await? {
        ctx.say(reason).await?;
        return Ok(());
    }

    let Some(service) = ctx.data().services.stock_quotes.clone() else {
        ctx.say("The stock data service is not available in this deployment.")
            .await?;
        return Ok(());
    };

    let settings = load_settings(ctx).await?;
    let symbol = normalize_symbol(symbol.unwrap_or(settings.default_symbol));
    let total_updates = total_updates();
    let response = build_stock_response(service.as_ref(), &symbol, 0, total_updates).await?;
    let Some(response) = response else {
        ctx.say("Failed to fetch stock data. Please try again later.")
            .await?;
        return Ok(());
    };

    let session = Arc::new(Mutex::new(StockSession::new(
        SessionKind::Stock {
            symbol: symbol.clone(),
        },
        service,
    )));

    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(response.embed.clone())
                .components(refresh_components()),
        )
        .await?;
    let message = reply.message().await?.into_owned();

    register_session(message.id.get(), session.clone()).await;
    initialize_session_loop(
        ctx.serenity_context().http.clone(),
        message.channel_id,
        message.id.get(),
        session,
        response.stop_reason,
    )
    .await;
    Ok(())
}

#[poise::command(slash_command, guild_only, category = "Stock")]
async fn etf(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_disable_reason(ctx).await? {
        ctx.say(reason).await?;
        return Ok(());
    }

    let Some(service) = ctx.data().services.stock_quotes.clone() else {
        ctx.say("The stock data service is not available in this deployment.")
            .await?;
        return Ok(());
    };

    let settings = load_settings(ctx).await?;
    let tickers = normalize_symbols(settings.etf_tickers);
    if tickers.is_empty() {
        ctx.say(
            "No stock tickers configured for this server. Please configure them in the dashboard.",
        )
        .await?;
        return Ok(());
    }

    let total_updates = total_updates();
    let response = build_etf_response(service.as_ref(), &tickers, 0, total_updates).await?;
    let Some(response) = response else {
        ctx.say("Failed to fetch ETF data. Please try again later.")
            .await?;
        return Ok(());
    };

    let session = Arc::new(Mutex::new(StockSession::new(
        SessionKind::Etf {
            tickers: tickers.clone(),
        },
        service,
    )));

    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(response.embed.clone())
                .components(refresh_components()),
        )
        .await?;
    let message = reply.message().await?.into_owned();

    register_session(message.id.get(), session.clone()).await;
    initialize_session_loop(
        ctx.serenity_context().http.clone(),
        message.channel_id,
        message.id.get(),
        session,
        response.stop_reason,
    )
    .await;
    Ok(())
}

async fn load_settings(ctx: Context<'_>) -> Result<StockSettings, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(StockSettings::default());
    };

    let guild_settings = ctx
        .data()
        .persistence
        .guild_settings_or_default(guild_id.get())
        .await?;

    let settings = guild_settings
        .modules
        .get(MODULE_ID)
        .map(parse_stock_settings)
        .transpose()?
        .unwrap_or_default();

    Ok(settings)
}

fn parse_stock_settings(module: &GuildModuleSettings) -> Result<StockSettings, Error> {
    Ok(serde_json::from_value::<StockSettings>(
        module.configuration.clone(),
    )?)
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
                "The `stock` module is not installed for this deployment.".to_string(),
            ));
        }
        if !module.enabled {
            return Ok(Some(
                "The `stock` module is disabled for this deployment.".to_string(),
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
                "The `stock` module is disabled for this guild.".to_string(),
            ));
        }
    }

    Ok(None)
}

fn normalize_symbol(symbol: String) -> String {
    let trimmed = symbol.trim().to_ascii_uppercase();
    if trimmed.is_empty() {
        DEFAULT_SYMBOL.to_string()
    } else {
        trimmed
    }
}

fn normalize_symbols(symbols: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    symbols
        .into_iter()
        .map(normalize_symbol)
        .filter(|symbol| seen.insert(symbol.clone()))
        .collect()
}

fn total_updates() -> u32 {
    (MAX_REFRESH_TIME_MS / REFRESH_INTERVAL_MS).max(1)
}

async fn build_stock_response(
    service: &dyn StockQuoteService,
    symbol: &str,
    update_count: u32,
    total_updates: u32,
) -> Result<Option<StockResponse>, Error> {
    let Some(snapshot) = service.fetch_quote(symbol).await? else {
        return Ok(None);
    };

    Ok(Some(StockResponse {
        embed: build_stock_embed(&snapshot, update_count, total_updates),
        stop_reason: stop_reason_for_phase(&snapshot.phase),
    }))
}

async fn build_etf_response(
    service: &dyn StockQuoteService,
    tickers: &[String],
    update_count: u32,
    total_updates: u32,
) -> Result<Option<StockResponse>, Error> {
    let snapshots = service.fetch_quotes(tickers).await?;
    if snapshots.is_empty() {
        return Ok(None);
    }

    let phase = representative_phase(&snapshots);
    Ok(Some(StockResponse {
        embed: build_etf_embed(&snapshots, &phase, update_count, total_updates),
        stop_reason: stop_reason_for_phase(&phase),
    }))
}

fn representative_phase(snapshots: &[Result<StockQuote, String>]) -> String {
    let valid = snapshots.iter().filter_map(|entry| entry.as_ref().ok());
    let phases = valid.map(|quote| quote.phase.as_str()).collect::<Vec<_>>();

    if phases.iter().any(|phase| *phase == "Regular Market") {
        return "Regular Market".to_string();
    }
    if phases.iter().any(|phase| *phase == "Pre Market") {
        return "Pre Market".to_string();
    }
    if phases.iter().any(|phase| *phase == "Post Market") {
        return "Post Market".to_string();
    }
    if phases.iter().any(|phase| *phase == "Closed") {
        return "Closed".to_string();
    }

    phases.first().copied().unwrap_or("Unknown").to_string()
}

fn stop_reason_for_phase(phase: &str) -> Option<&'static str> {
    match phase {
        "Regular Market" | "Pre Market" => None,
        "Post Market" => Some("post_market"),
        "Closed" => Some("market_closed"),
        _ => Some("market_state_unknown"),
    }
}

fn build_stock_embed(snapshot: &StockQuote, update_count: u32, total_updates: u32) -> CreateEmbed {
    let current = current_market_data(snapshot, &snapshot.phase);
    let mut embed = CreateEmbed::new()
        .title(format!(
            "{} / [{}]",
            snapshot
                .long_name
                .as_deref()
                .or(snapshot.short_name.as_deref())
                .unwrap_or(&snapshot.symbol),
            snapshot.symbol
        ))
        .url(format!(
            "https://finance.yahoo.com/quote/{}",
            snapshot.symbol
        ))
        .color(embed_color(current.change))
        .footer(CreateEmbedFooter::new(format!(
            "Data from Yahoo Finance. Update {update_count}/{total_updates}."
        )))
        .field(
            "Market State",
            format!(
                "{} {}",
                snapshot.phase,
                market_status_emoji(&snapshot.phase)
            ),
            true,
        )
        .field(
            "Price",
            format_money(&snapshot.currency_label, current.price),
            true,
        )
        .field(
            "Change",
            format_change(current.change, current.change_percent),
            true,
        )
        .field(
            "Day High",
            format_money(&snapshot.currency_label, snapshot.regular_market_day_high),
            true,
        )
        .field(
            "Day Low",
            format_money(&snapshot.currency_label, snapshot.regular_market_day_low),
            true,
        )
        .field(
            "Volume",
            format_volume(snapshot.regular_market_volume),
            true,
        );

    if snapshot.pre_market_price.is_some() {
        embed = embed
            .field(
                "Pre - Price",
                format_money(&snapshot.currency_label, snapshot.pre_market_price),
                true,
            )
            .field(
                "Pre - Change",
                format_change(
                    snapshot.pre_market_change,
                    snapshot.pre_market_change_percent,
                ),
                true,
            );
    }

    if snapshot.post_market_price.is_some() {
        embed = embed
            .field(
                "Post - Price",
                format_money(&snapshot.currency_label, snapshot.post_market_price),
                true,
            )
            .field(
                "Post - Change",
                format_change(
                    snapshot.post_market_change,
                    snapshot.post_market_change_percent,
                ),
                true,
            );
    }

    if let Some(value) = snapshot.quote_type.as_deref() {
        embed = embed.field("Quote Type", value, true);
    }
    if let Some(value) = snapshot.exchange_name.as_deref() {
        embed = embed.field("Exchange", value, true);
    }
    if let Some(value) = snapshot.market_cap {
        embed = embed.field("Market Cap", format_compact_number(value), true);
    }
    if let Some(value) = snapshot.trailing_pe {
        embed = embed.field("Trailing P/E", format!("{value:.2}"), true);
    }
    if let Some(value) = snapshot.forward_pe {
        embed = embed.field("Forward P/E", format!("{value:.2}"), true);
    }
    if let Some(value) = snapshot.trailing_eps {
        embed = embed.field(
            "EPS",
            format_money(&snapshot.currency_label, Some(value)),
            true,
        );
    }
    if let Some(value) = snapshot.dividend_yield {
        embed = embed.field("Dividend Yield", format!("{:.2}%", value * 100.0), true);
    }
    if let Some(value) = snapshot.fifty_two_week_high {
        embed = embed.field(
            "52W High",
            format_money(&snapshot.currency_label, Some(value)),
            true,
        );
    }
    if let Some(value) = snapshot.fifty_two_week_low {
        embed = embed.field(
            "52W Low",
            format_money(&snapshot.currency_label, Some(value)),
            true,
        );
    }
    if let Some(value) = snapshot.sector.as_deref() {
        embed = embed.field("Sector", value, true);
    }
    if let Some(value) = snapshot.industry.as_deref() {
        embed = embed.field("Industry", value, true);
    }

    embed
}

fn build_etf_embed(
    snapshots: &[Result<StockQuote, String>],
    phase: &str,
    update_count: u32,
    total_updates: u32,
) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title("ETFs")
        .color(DEFAULT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(format!(
            "Data from Yahoo Finance. Update {update_count}/{total_updates}."
        )))
        .field(
            "Market State",
            format!("{phase} {}", market_status_emoji(phase)),
            false,
        );

    for snapshot in snapshots {
        match snapshot {
            Ok(snapshot) => {
                let current = current_market_data(snapshot, phase);
                embed = embed.field(
                    snapshot.symbol.clone(),
                    format!(
                        "{}\n{}\nVolume: {}",
                        format_money(&snapshot.currency_label, current.price),
                        format_change(current.change, current.change_percent),
                        format_volume(snapshot.regular_market_volume),
                    ),
                    true,
                );
            }
            Err(error) => {
                embed = embed.field("Ticker", error.clone(), false);
            }
        }
    }

    embed
}

struct CurrentMarketData {
    price: Option<f64>,
    change: Option<f64>,
    change_percent: Option<f64>,
}

fn current_market_data(snapshot: &StockQuote, phase: &str) -> CurrentMarketData {
    match phase {
        "Pre Market" if snapshot.pre_market_price.is_some() => CurrentMarketData {
            price: snapshot.pre_market_price,
            change: snapshot.pre_market_change,
            change_percent: snapshot.pre_market_change_percent,
        },
        "Post Market" if snapshot.post_market_price.is_some() => CurrentMarketData {
            price: snapshot.post_market_price,
            change: snapshot.post_market_change,
            change_percent: snapshot.post_market_change_percent,
        },
        _ => CurrentMarketData {
            price: snapshot.regular_market_price,
            change: snapshot.regular_market_change,
            change_percent: snapshot.regular_market_change_percent,
        },
    }
}

fn embed_color(change: Option<f64>) -> u32 {
    match change {
        Some(value) if value > 0.0 => UPWARD_EMBED_COLOR,
        Some(value) if value < 0.0 => DOWNWARD_EMBED_COLOR,
        _ => DEFAULT_EMBED_COLOR,
    }
}

fn market_status_emoji(phase: &str) -> &'static str {
    match phase {
        "Regular Market" => ":green_circle:",
        "Pre Market" => ":orange_circle:",
        "Post Market" | "Closed" => ":red_circle:",
        _ => ":black_circle:",
    }
}

fn format_money(label: &str, value: Option<f64>) -> String {
    match value {
        Some(value) if !label.is_empty() => format!("{label} {value:.2}"),
        Some(value) => format!("{value:.2}"),
        None => "N/A".to_string(),
    }
}

fn format_change(change: Option<f64>, change_percent: Option<f64>) -> String {
    match (change, change_percent) {
        (Some(change), Some(percent)) => format!(
            "{change:.2} ({:.2}%){}",
            percent * 100.0,
            if change > 0.0 {
                " 📈"
            } else if change < 0.0 {
                " 📉"
            } else {
                ""
            }
        ),
        _ => "N/A".to_string(),
    }
}

fn format_volume(volume: Option<f64>) -> String {
    volume
        .map(format_compact_number)
        .unwrap_or_else(|| "N/A".to_string())
}

fn format_compact_number(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000_000.0 {
        return format!("{:.2}T", value / 1_000_000_000_000.0);
    }
    if abs >= 1_000_000_000.0 {
        return format!("{:.2}B", value / 1_000_000_000.0);
    }
    if abs >= 1_000_000.0 {
        return format!("{:.2}M", value / 1_000_000.0);
    }
    if abs >= 1_000.0 {
        return format!("{:.2}K", value / 1_000.0);
    }

    format!("{:.0}", value)
}

fn refresh_components() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(STOCK_REFRESH_BUTTON_ID)
            .label("Refresh")
            .style(ButtonStyle::Secondary),
    ])]
}

async fn register_session(message_id: u64, session: Arc<Mutex<StockSession>>) {
    let mut sessions = stock_sessions().write().await;
    if sessions.len() >= MAX_STORED_SESSIONS {
        if let Some(oldest) = sessions.keys().next().copied() {
            sessions.remove(&oldest);
        }
    }
    sessions.insert(message_id, session);
}

async fn remove_session(message_id: u64) {
    stock_sessions().write().await.remove(&message_id);
}

async fn initialize_session_loop(
    http: Arc<poise::serenity_prelude::Http>,
    channel_id: ChannelId,
    message_id: u64,
    session: Arc<Mutex<StockSession>>,
    stop_reason: Option<&'static str>,
) {
    let mut state = session.lock().await;
    state.last_stop_reason = stop_reason;
    state.manual_restart_in_progress = false;

    if stop_reason.is_some() {
        state.active = false;
        return;
    }

    state.active = true;
    state.generation += 1;
    let generation = state.generation;
    drop(state);

    tokio::spawn(async move {
        let mut update_count = 0u32;
        let mut consecutive_failures = 0u32;

        loop {
            sleep(Duration::from_millis(REFRESH_INTERVAL_MS as u64)).await;

            {
                let state = session.lock().await;
                if !state.active || state.generation != generation {
                    break;
                }
            }

            update_count += 1;
            if update_count >= total_updates() {
                let mut state = session.lock().await;
                if state.generation == generation {
                    state.active = false;
                    state.last_stop_reason = Some("max_refresh_reached");
                }
                break;
            }

            let (kind, service) = {
                let state = session.lock().await;
                (state.kind.clone(), state.service.clone())
            };

            let response = match fetch_response_for_kind(
                service.as_ref(),
                &kind,
                update_count,
                total_updates(),
            )
            .await
            {
                Ok(value) => value,
                Err(_) => {
                    consecutive_failures += 1;
                    if consecutive_failures >= 3 {
                        let mut state = session.lock().await;
                        if state.generation == generation {
                            state.active = false;
                            state.last_stop_reason = Some("fetch_error_threshold");
                        }
                        break;
                    }
                    continue;
                }
            };

            let Some(response) = response else {
                consecutive_failures += 1;
                if consecutive_failures >= 3 {
                    let mut state = session.lock().await;
                    if state.generation == generation {
                        state.active = false;
                        state.last_stop_reason = Some("fetch_error_threshold");
                    }
                    break;
                }
                continue;
            };

            consecutive_failures = 0;

            if edit_message(&http, channel_id, message_id, response.embed.clone())
                .await
                .is_err()
            {
                let mut state = session.lock().await;
                if state.generation == generation {
                    state.active = false;
                    state.last_stop_reason = Some("interaction_edit_failed");
                }
                remove_session(message_id).await;
                break;
            }

            if let Some(reason) = response.stop_reason {
                let mut state = session.lock().await;
                if state.generation == generation {
                    state.active = false;
                    state.last_stop_reason = Some(reason);
                }
                break;
            }
        }
    });
}

async fn fetch_response_for_kind(
    service: &dyn StockQuoteService,
    kind: &SessionKind,
    update_count: u32,
    total_updates: u32,
) -> Result<Option<StockResponse>, Error> {
    match kind {
        SessionKind::Stock { symbol } => {
            build_stock_response(service, symbol, update_count, total_updates).await
        }
        SessionKind::Etf { tickers } => {
            build_etf_response(service, tickers, update_count, total_updates).await
        }
    }
}

async fn edit_message(
    http: &poise::serenity_prelude::Http,
    channel_id: ChannelId,
    message_id: u64,
    embed: CreateEmbed,
) -> Result<(), Error> {
    channel_id
        .edit_message(
            http,
            message_id,
            EditMessage::new()
                .embed(embed)
                .components(refresh_components()),
        )
        .await?;
    Ok(())
}

pub async fn handle_component_interaction(
    ctx: &poise::serenity_prelude::Context,
    interaction: &Interaction,
) -> Result<bool, Error> {
    let Interaction::Component(component) = interaction else {
        return Ok(false);
    };

    if component.data.custom_id != STOCK_REFRESH_BUTTON_ID {
        return Ok(false);
    }

    handle_refresh_button(ctx, component).await?;
    Ok(true)
}

async fn handle_refresh_button(
    ctx: &poise::serenity_prelude::Context,
    component: &ComponentInteraction,
) -> Result<(), Error> {
    let message_id = component.message.id.get();
    let session = {
        let sessions = stock_sessions().read().await;
        sessions.get(&message_id).cloned()
    };

    let Some(session) = session else {
        component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This refresh session has expired. Please run `/stock` or `/etf` again.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    };

    {
        let mut state = session.lock().await;
        if state.active {
            component
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("The default refresh loop is still running, so this button is not available yet.")
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }

        if state.manual_restart_in_progress {
            component
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(
                                "A refresh restart is already being prepared for this message.",
                            )
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }

        if state.manual_refresh_count >= MAX_MANUAL_REFRESHES {
            component
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(format!(
                                "You can manually restart this refresh loop up to {} times.",
                                MAX_MANUAL_REFRESHES
                            ))
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }

        state.manual_restart_in_progress = true;
    }

    component.defer_ephemeral(ctx).await?;

    let (kind, service) = {
        let state = session.lock().await;
        (state.kind.clone(), state.service.clone())
    };
    let response = fetch_response_for_kind(service.as_ref(), &kind, 0, total_updates()).await?;

    let Some(response) = response else {
        {
            let mut state = session.lock().await;
            state.manual_restart_in_progress = false;
            state.last_stop_reason = Some("fetch_failed");
            state.active = false;
        }

        component
            .edit_response(
                ctx,
                poise::serenity_prelude::EditInteractionResponse::new()
                    .content("Failed to refresh stock data. Please try again later."),
            )
            .await?;
        return Ok(());
    };

    edit_message(
        &ctx.http,
        component.channel_id,
        message_id,
        response.embed.clone(),
    )
    .await?;

    {
        let mut state = session.lock().await;
        state.manual_restart_in_progress = false;
        state.manual_refresh_count += 1;
    }

    initialize_session_loop(
        ctx.http.clone(),
        component.channel_id,
        message_id,
        session,
        response.stop_reason,
    )
    .await;

    component
        .edit_response(
            ctx,
            poise::serenity_prelude::EditInteractionResponse::new()
                .content("Stock refresh updated."),
        )
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{current_market_data, normalize_symbol, normalize_symbols, total_updates};
    use dynamo_core::StockQuote;

    #[test]
    fn normalizes_symbols_to_uppercase() {
        assert_eq!(normalize_symbol(" nvda ".to_string()), "NVDA");
    }

    #[test]
    fn removes_duplicate_tickers() {
        let normalized = normalize_symbols(vec![
            "soxl".to_string(),
            "SOXL".to_string(),
            "tqqq".to_string(),
        ]);
        assert_eq!(normalized, vec!["SOXL".to_string(), "TQQQ".to_string()]);
    }

    #[test]
    fn computes_total_updates_from_refresh_window() {
        assert_eq!(total_updates(), 12);
    }

    #[test]
    fn prefers_pre_market_values_when_active() {
        let quote = StockQuote {
            phase: "Pre Market".to_string(),
            pre_market_price: Some(101.0),
            pre_market_change: Some(1.0),
            pre_market_change_percent: Some(0.01),
            regular_market_price: Some(100.0),
            regular_market_change: Some(0.5),
            regular_market_change_percent: Some(0.005),
            ..StockQuote::default()
        };

        let current = current_market_data(&quote, &quote.phase);
        assert_eq!(current.price, Some(101.0));
        assert_eq!(current.change, Some(1.0));
        assert_eq!(current.change_percent, Some(0.01));
    }
}
