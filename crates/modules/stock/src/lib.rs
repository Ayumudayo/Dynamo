use dynamo_core::{
    Context, DiscordCommand, Error, GatewayIntents, GuildModuleSettings, Module, ModuleCategory,
    ModuleManifest, SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection,
};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const MODULE_ID: &str = "stock";
const DEFAULT_SYMBOL: &str = "NVDA";
const DEFAULT_ETF_TICKERS: [&str; 3] = ["SOXL", "TQQQ", "VOO"];
const DEFAULT_EMBED_COLOR: u32 = 0x4F545C;
const UPWARD_EMBED_COLOR: u32 = 0x43B581;
const DOWNWARD_EMBED_COLOR: u32 = 0xF04747;
const REFRESH_INTERVAL_MS: u32 = 5_000;
const MAX_REFRESH_TIME_MS: u32 = 60_000;

#[derive(Debug, Deserialize)]
struct QuoteEnvelope {
    #[serde(rename = "quoteResponse")]
    quote_response: QuoteResponse,
}

#[derive(Debug, Deserialize)]
struct QuoteResponse {
    result: Vec<YahooQuote>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct YahooQuote {
    symbol: String,
    short_name: Option<String>,
    long_name: Option<String>,
    currency: Option<String>,
    market_state: Option<String>,
    regular_market_price: Option<f64>,
    regular_market_change: Option<f64>,
    regular_market_change_percent: Option<f64>,
    pre_market_price: Option<f64>,
    pre_market_change: Option<f64>,
    pre_market_change_percent: Option<f64>,
    post_market_price: Option<f64>,
    post_market_change: Option<f64>,
    post_market_change_percent: Option<f64>,
    regular_market_day_high: Option<f64>,
    regular_market_day_low: Option<f64>,
    regular_market_volume: Option<f64>,
}

#[derive(Debug, Clone)]
struct StockSnapshot {
    symbol: String,
    short_name: Option<String>,
    long_name: Option<String>,
    currency_label: String,
    phase: String,
    regular_market_price: Option<f64>,
    regular_market_change: Option<f64>,
    regular_market_change_percent: Option<f64>,
    pre_market_price: Option<f64>,
    pre_market_change: Option<f64>,
    pre_market_change_percent: Option<f64>,
    post_market_price: Option<f64>,
    post_market_change: Option<f64>,
    post_market_change_percent: Option<f64>,
    regular_market_day_high: Option<f64>,
    regular_market_day_low: Option<f64>,
    regular_market_volume: Option<f64>,
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

    let settings = load_settings(ctx).await?;
    let symbol = normalize_symbol(symbol.unwrap_or(settings.default_symbol));
    let total_updates = total_updates();
    let response = build_stock_response(&symbol, 0, total_updates).await?;
    let Some(embed) = response else {
        ctx.say("Failed to fetch stock data. Please try again later.")
            .await?;
        return Ok(());
    };

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(slash_command, guild_only, category = "Stock")]
async fn etf(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_disable_reason(ctx).await? {
        ctx.say(reason).await?;
        return Ok(());
    }

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
    let response = build_etf_response(&tickers, 0, total_updates).await?;
    let Some(embed) = response else {
        ctx.say("Failed to fetch ETF data. Please try again later.")
            .await?;
        return Ok(());
    };

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
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
    symbol: &str,
    update_count: u32,
    total_updates: u32,
) -> Result<Option<CreateEmbed>, Error> {
    let snapshots = fetch_quotes(&[symbol.to_string()]).await?;
    let Some(snapshot) = snapshots.into_iter().next().and_then(|entry| entry.ok()) else {
        return Ok(None);
    };

    Ok(Some(build_stock_embed(
        &snapshot,
        update_count,
        total_updates,
    )))
}

async fn build_etf_response(
    tickers: &[String],
    update_count: u32,
    total_updates: u32,
) -> Result<Option<CreateEmbed>, Error> {
    let snapshots = fetch_quotes(tickers).await?;
    if snapshots.is_empty() {
        return Ok(None);
    }

    let phase = representative_phase(&snapshots);
    Ok(Some(build_etf_embed(
        &snapshots,
        &phase,
        update_count,
        total_updates,
    )))
}

async fn fetch_quotes(symbols: &[String]) -> Result<Vec<Result<StockSnapshot, String>>, Error> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let url = format!(
        "https://query1.finance.yahoo.com/v7/finance/quote?symbols={}",
        symbols.join(",")
    );
    let response = client.get(url).send().await?.error_for_status()?;
    let envelope = response.json::<QuoteEnvelope>().await?;

    let mut by_symbol = std::collections::HashMap::new();
    for quote in envelope.quote_response.result {
        by_symbol.insert(quote.symbol.to_ascii_uppercase(), quote);
    }

    Ok(symbols
        .iter()
        .map(|symbol| {
            by_symbol
                .get(&symbol.to_ascii_uppercase())
                .cloned()
                .map(normalize_quote)
                .ok_or_else(|| "Invalid Ticker".to_string())
        })
        .collect())
}

fn normalize_quote(quote: YahooQuote) -> StockSnapshot {
    StockSnapshot {
        symbol: quote.symbol,
        short_name: quote.short_name,
        long_name: quote.long_name,
        currency_label: quote.currency.unwrap_or_default(),
        phase: normalize_market_state(quote.market_state.as_deref()),
        regular_market_price: quote.regular_market_price,
        regular_market_change: quote.regular_market_change,
        regular_market_change_percent: quote.regular_market_change_percent,
        pre_market_price: quote.pre_market_price,
        pre_market_change: quote.pre_market_change,
        pre_market_change_percent: quote.pre_market_change_percent,
        post_market_price: quote.post_market_price,
        post_market_change: quote.post_market_change,
        post_market_change_percent: quote.post_market_change_percent,
        regular_market_day_high: quote.regular_market_day_high,
        regular_market_day_low: quote.regular_market_day_low,
        regular_market_volume: quote.regular_market_volume,
    }
}

fn normalize_market_state(state: Option<&str>) -> String {
    match state.unwrap_or_default() {
        "REGULAR" => "Regular Market".to_string(),
        "PRE" | "PREPRE" => "Pre Market".to_string(),
        "POST" | "POSTPOST" => "Post Market".to_string(),
        "CLOSED" => "Closed".to_string(),
        _ => "Unknown".to_string(),
    }
}

fn representative_phase(snapshots: &[Result<StockSnapshot, String>]) -> String {
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

fn build_stock_embed(
    snapshot: &StockSnapshot,
    update_count: u32,
    total_updates: u32,
) -> CreateEmbed {
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
            false,
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
        );

    if snapshot.phase == "Pre Market" {
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
    } else if snapshot.phase == "Post Market" {
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

    embed
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
        )
}

fn build_etf_embed(
    snapshots: &[Result<StockSnapshot, String>],
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
                embed = embed
                    .field(
                        snapshot.symbol.clone(),
                        format_money(&snapshot.currency_label, current.price),
                        true,
                    )
                    .field(
                        "Change",
                        format_change(current.change, current.change_percent),
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

fn current_market_data(snapshot: &StockSnapshot, phase: &str) -> CurrentMarketData {
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
        .map(|value| (value.round() as u64).to_string())
        .unwrap_or_else(|| "N/A".to_string())
}

#[cfg(test)]
mod tests {
    use super::{normalize_symbol, normalize_symbols, total_updates};

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
}
