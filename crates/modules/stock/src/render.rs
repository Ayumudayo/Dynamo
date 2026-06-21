use crate::constants::{
    BOT_EMBED_COLOR, DEFAULT_EMBED_COLOR, DOWN_EMOJI, DOWNWARD_EMBED_COLOR, STOCK_THUMBNAIL_URL,
    UP_EMOJI, UPWARD_EMBED_COLOR,
};
use dynamo_domain_stock::StockQuote;
use dynamo_runtime_api::Error;
use dynamo_service_stock::StockQuoteService;
use poise::serenity_prelude::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter,
};

const STOCK_PROVIDER_FOOTER_LABEL: &str = "Toss Invest";
const ACTIVE_MARKET_PHASES: [&str; 4] =
    ["Day Market", "Pre Market", "Regular Market", "After Market"];
const REPRESENTATIVE_PHASE_PRIORITY: [&str; 5] = [
    "Regular Market",
    "Pre Market",
    "Day Market",
    "After Market",
    "Closed",
];

#[derive(Debug, Clone)]
pub(crate) struct StockResponse {
    pub(crate) embed: CreateEmbed,
    pub(crate) stop_reason: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub(crate) struct CurrentMarketData {
    pub(crate) price: Option<f64>,
    pub(crate) change: Option<f64>,
    pub(crate) change_percent: Option<f64>,
}

pub(crate) async fn build_stock_response(
    service: &dyn StockQuoteService,
    symbol: &str,
    update_count: u32,
    total_updates: u32,
) -> Result<Option<StockResponse>, Error> {
    let Some(snapshot) = service.fetch_quote(symbol).await? else {
        return Ok(None);
    };
    let stop_reason = stop_reason_for_phase(&snapshot.phase);

    Ok(Some(StockResponse {
        embed: build_stock_embed(
            &snapshot,
            &refresh_footer_text(update_count, total_updates, stop_reason),
        ),
        stop_reason,
    }))
}

pub(crate) async fn build_etf_response(
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
    let stop_reason = stop_reason_for_phase(&phase);
    Ok(Some(StockResponse {
        embed: build_etf_embed(
            tickers,
            &snapshots,
            &phase,
            &refresh_footer_text(update_count, total_updates, stop_reason),
        ),
        stop_reason,
    }))
}

pub(crate) fn representative_phase(snapshots: &[Result<StockQuote, String>]) -> String {
    let valid = snapshots.iter().filter_map(|entry| entry.as_ref().ok());
    let phases = valid.map(|quote| quote.phase.as_str()).collect::<Vec<_>>();

    for phase in REPRESENTATIVE_PHASE_PRIORITY {
        if phases.contains(&phase) {
            return phase.to_string();
        }
    }

    phases.first().copied().unwrap_or("Unknown").to_string()
}

pub(crate) fn stop_reason_for_phase(phase: &str) -> Option<&'static str> {
    if ACTIVE_MARKET_PHASES.contains(&phase) {
        return None;
    }

    match phase {
        "Closed" => Some("market_closed"),
        _ => Some("market_state_unknown"),
    }
}

pub(crate) fn refresh_footer_text(
    update_count: u32,
    total_updates: u32,
    stop_reason: Option<&'static str>,
) -> String {
    if let Some(reason) = stop_reason {
        return format!(
            "{STOCK_PROVIDER_FOOTER_LABEL} · {}",
            refresh_stop_reason_text(reason)
        );
    }

    if update_count == 0 {
        return format!("{STOCK_PROVIDER_FOOTER_LABEL} · Active");
    }

    if update_count >= total_updates {
        return format!("{STOCK_PROVIDER_FOOTER_LABEL} · Done {total_updates}/{total_updates}");
    }

    format!("{STOCK_PROVIDER_FOOTER_LABEL} · {update_count}/{total_updates}")
}

fn refresh_stop_reason_text(reason: &'static str) -> &'static str {
    match reason {
        "post_market" | "market_closed" | "market_state_unknown" => "Stopped",
        "max_refresh_reached" => "Done",
        _ => "Stopped",
    }
}

pub(crate) fn build_stock_embed(snapshot: &StockQuote, footer_text: &str) -> CreateEmbed {
    let current = primary_stock_market_data(snapshot);
    CreateEmbed::new()
        .title(format!(
            "{} / [{}]",
            snapshot
                .long_name
                .as_deref()
                .or(snapshot.short_name.as_deref())
                .unwrap_or(&snapshot.symbol),
            snapshot.symbol
        ))
        .thumbnail(STOCK_THUMBNAIL_URL)
        .color(embed_color(stock_embed_color_change(snapshot)))
        .footer(CreateEmbedFooter::new(footer_text.to_string()))
        .timestamp(poise::serenity_prelude::Timestamp::now())
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
        )
        .field(" ", " ", false)
}

pub(crate) fn build_etf_embed(
    tickers: &[String],
    snapshots: &[Result<StockQuote, String>],
    phase: &str,
    footer_text: &str,
) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title("ETFs")
        .thumbnail(STOCK_THUMBNAIL_URL)
        .color(BOT_EMBED_COLOR)
        .footer(CreateEmbedFooter::new(footer_text.to_string()))
        .timestamp(poise::serenity_prelude::Timestamp::now())
        .field(
            "Market State",
            format!("{phase} {}", market_status_emoji(phase)),
            false,
        );

    for (index, snapshot) in snapshots.iter().enumerate() {
        match snapshot {
            Ok(snapshot) => {
                let current = current_market_data(snapshot, &snapshot.phase);
                embed = embed.field(
                    snapshot.symbol.clone(),
                    format_money(&snapshot.currency_label, current.price),
                    true,
                );
                embed = embed.field(
                    "Change",
                    format_change(current.change, current.change_percent),
                    true,
                );
                embed = embed.field(" ", " ", false);
            }
            Err(error) => {
                let name = tickers
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| "Ticker".to_string());
                embed = embed.field(name, error.clone(), false);
            }
        }
    }

    embed
}

pub(crate) fn primary_stock_market_data(snapshot: &StockQuote) -> CurrentMarketData {
    current_market_data(snapshot, &snapshot.phase)
}

pub(crate) fn stock_embed_color_change(snapshot: &StockQuote) -> Option<f64> {
    match snapshot.phase.as_str() {
        "Day Market" | "Pre Market" => snapshot
            .pre_market_change
            .or(snapshot.regular_market_change),
        "After Market" => snapshot
            .post_market_change
            .or(snapshot.regular_market_change),
        _ => snapshot.regular_market_change,
    }
}

pub(crate) fn current_market_data(snapshot: &StockQuote, phase: &str) -> CurrentMarketData {
    match phase {
        "Day Market" | "Pre Market" if snapshot.pre_market_price.is_some() => CurrentMarketData {
            price: snapshot.pre_market_price,
            change: snapshot.pre_market_change,
            change_percent: snapshot.pre_market_change_percent,
        },
        "After Market" if snapshot.post_market_price.is_some() => CurrentMarketData {
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
        "Day Market" => ":blue_circle:",
        "Regular Market" => ":green_circle:",
        "Pre Market" => ":orange_circle:",
        "After Market" => ":purple_circle:",
        "Closed" => ":red_circle:",
        _ => ":black_circle:",
    }
}

pub(crate) fn format_money(label: &str, value: Option<f64>) -> String {
    match value {
        Some(value) if label.is_empty() => format!("{value:.2}"),
        Some(value) if label.eq_ignore_ascii_case("USD") => format!("${value:.2}"),
        Some(value) if label.len() == 3 && label.chars().all(|ch| ch.is_ascii_uppercase()) => {
            format!("{label} {value:.2}")
        }
        Some(value) => format!("{label}{value:.2}"),
        None => "N/A".to_string(),
    }
}

fn format_change(change: Option<f64>, change_percent: Option<f64>) -> String {
    match (change, change_percent) {
        (Some(change), Some(percent)) => format!(
            "{change:.2} ({:.2}%){}",
            percent * 100.0,
            if change > 0.0 {
                format!(" {UP_EMOJI}")
            } else if change < 0.0 {
                format!(" {DOWN_EMOJI}")
            } else {
                String::new()
            }
        ),
        _ => "N/A".to_string(),
    }
}

pub(crate) fn refresh_components(button_id: &str) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(button_id)
            .label("Refresh")
            .style(ButtonStyle::Secondary),
    ])]
}
