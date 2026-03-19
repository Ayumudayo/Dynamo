use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Asia::{Seoul, Tokyo};
use dynamo_access::module_access_for_context;
use dynamo_module_kit::{
    DiscordCommand, GatewayIntents, Module, ModuleCategory, ModuleManifest, SettingsField,
    SettingsFieldKind, SettingsSchema, SettingsSection,
};
use dynamo_runtime_api::{AppState, Context, Error};
use poise::serenity_prelude::{
    CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter, Timestamp,
};
use regex::Regex;
use reqwest::{
    Client,
    header::{ACCEPT_LANGUAGE, HeaderMap, HeaderValue, USER_AGENT},
};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use tokio::fs;

const MODULE_ID: &str = "gameinfo";
const DEFAULT_WT_LINK: &str = "http://warthunder.com/en/registration?r=userinvite_18945695";
const DEFAULT_WOT_LINK: &str =
    "https://worldoftanks.asia/referral/9ed8df012d204670b04c1cc1c88d98d5";
const DEFAULT_THUMBNAIL_URL: &str = "https://media.discordapp.net/attachments/1138398345065414657/1329005700730585118/png-clipart-war-thunder-playstation-4-aircraft-airplane-macchi-c-202-thunder-game-video-game-removebg-preview.png?ex=6788c482&is=67877302&hm=31b9ed755040306ea8d1c9db258ffaa590df7e3bfa6139d875c62915d46c1b73&=&format=webp&quality=lossless";
const GAMEINFO_THUMBNAIL_URL: &str =
    "https://cdn.discordapp.com/attachments/1138398345065414657/1138398369929244713/0001061.png";
const LODESTONE_BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";
const MAINTENANCE_LIST_URL: &str = "https://jp.finalfantasyxiv.com/lodestone/news/category/2";
const TOPICS_LIST_URL: &str = "https://jp.finalfantasyxiv.com/lodestone/topics/";
const PLL_CACHE_DURATION_SECONDS: i64 = 12 * 60 * 60;
const MAX_MAINTENANCE_CANDIDATES: usize = 6;
const MAX_PLL_PAGES: usize = 3;
const SUCCESS_EMBED_COLOR: u32 = 0x00A56A;
const ERROR_EMBED_COLOR: u32 = 0xD61A3C;

pub struct GameInfoModule;

impl Module<AppState, Error> for GameInfoModule {
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

    fn commands(&self) -> Vec<DiscordCommand<AppState, Error>> {
        vec![wtinv(), maint(), pll()]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct GameInfoCacheFile {
    #[serde(rename = "MAINTINFO", default)]
    maintinfo: serde_json::Value,
    #[serde(rename = "PLLINFO", default)]
    pllinfo: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MaintInfo {
    id: String,
    start_stamp: i64,
    end_stamp: i64,
    title_kr: String,
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PllInfo {
    #[serde(alias = "fixedTitle")]
    fixed_title: String,
    url: String,
    start_stamp: Option<i64>,
    #[serde(alias = "expireTime")]
    expire_time: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaintenanceNoticeKind {
    Regular,
    Emergency,
    FollowUp,
    EmergencyFollowUp,
}

#[derive(Debug, Clone)]
struct LodestoneLink {
    title: String,
    url: String,
}

/// Show the War Thunder and World of Tanks referral panel for this guild.
#[poise::command(slash_command, guild_only, category = "Game Info")]
async fn wtinv(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let settings = load_settings(ctx).await?;
    let embed = CreateEmbed::new()
        .title(settings.title)
        .color(SUCCESS_EMBED_COLOR)
        .timestamp(Timestamp::now());

    let embed = if settings.thumbnail_url.trim().is_empty() {
        embed
    } else {
        embed.thumbnail(settings.thumbnail_url)
    };

    let mut buttons = Vec::new();
    if let Some(url) = to_valid_url(&settings.wt_link) {
        buttons.push(CreateButton::new_link(url).label("War Thunder"));
    }
    if let Some(url) = to_valid_url(&settings.wot_link) {
        buttons.push(CreateButton::new_link(url).label("World of Tanks"));
    }

    let reply = if buttons.is_empty() {
        poise::CreateReply::default()
            .embed(embed.description("No invite links are currently configured."))
    } else {
        poise::CreateReply::default()
            .embed(embed)
            .components(vec![CreateActionRow::Buttons(buttons)])
    };

    ctx.send(reply).await?;
    Ok(())
}

/// Show the latest known FFXIV global maintenance window.
#[poise::command(slash_command, category = "Game Info")]
async fn maint(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let embed = get_maintenance_embed().await?;
    let embed = embed.unwrap_or_else(create_maintenance_error_embed);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Show the latest known Producer Letter Live schedule.
#[poise::command(slash_command, category = "Game Info")]
async fn pll(ctx: Context<'_>) -> Result<(), Error> {
    if let Some(reason) = module_access_for_context(ctx, MODULE_ID)
        .await?
        .denial_reason
    {
        ctx.say(reason).await?;
        return Ok(());
    }

    let embed = get_pll_embed().await?;
    let embed = embed.unwrap_or_else(create_pll_error_embed);
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

async fn get_maintenance_embed() -> Result<Option<CreateEmbed>, Error> {
    let info = fetch_maintenance_info().await?;
    Ok(info.map(|info| {
        CreateEmbed::new()
            .title(info.title_kr)
            .url(info.url)
            .color(SUCCESS_EMBED_COLOR)
            .thumbnail(GAMEINFO_THUMBNAIL_URL)
            .timestamp(Timestamp::now())
            .field("시작 시각", format!("<t:{}:F>", info.start_stamp), false)
            .field("종료 시각", format!("<t:{}:F>", info.end_stamp), false)
            .field(
                "종료까지 남은 시간",
                format!("<t:{}:R>", info.end_stamp),
                false,
            )
            .footer(CreateEmbedFooter::new("From Lodestone News"))
    }))
}

async fn fetch_maintenance_info() -> Result<Option<MaintInfo>, Error> {
    let cache_store = GameInfoCacheStore::new();
    let now = Utc::now().timestamp();
    let client = lodestone_client()?;

    let list_html = fetch_lodestone_html(&client, MAINTENANCE_LIST_URL).await;
    let Ok(list_html) = list_html else {
        return Ok(cache_store
            .load_maintinfo()
            .await?
            .filter(|info| info.end_stamp > now));
    };

    let candidates = parse_maintenance_candidates(&list_html);
    for candidate in candidates.into_iter().take(MAX_MAINTENANCE_CANDIDATES) {
        let detail_html = fetch_lodestone_html(&client, &candidate.url).await;
        let Ok(detail_html) = detail_html else {
            continue;
        };

        let Some(next) = parse_maintenance_detail(&candidate.url, &detail_html)? else {
            continue;
        };

        if next.end_stamp <= now {
            continue;
        }

        cache_store.save_maintinfo(&next).await?;
        return Ok(Some(next));
    }

    Ok(cache_store
        .load_maintinfo()
        .await?
        .filter(|info| info.end_stamp > now))
}

async fn get_pll_embed() -> Result<Option<CreateEmbed>, Error> {
    let info = fetch_pll_info().await?;
    Ok(info.map(|info| {
        CreateEmbed::new()
            .title(info.fixed_title)
            .url(info.url)
            .color(SUCCESS_EMBED_COLOR)
            .thumbnail(GAMEINFO_THUMBNAIL_URL)
            .timestamp(Timestamp::now())
            .field(
                "방송 시작",
                info.start_stamp
                    .map(|stamp| format!("<t:{stamp}:F>"))
                    .unwrap_or_else(|| "확인 불가".to_string()),
                false,
            )
            .field(
                "시작까지 남은 시간",
                info.start_stamp
                    .map(|stamp| format!("<t:{stamp}:R>"))
                    .unwrap_or_else(|| "확인 불가".to_string()),
                false,
            )
            .footer(CreateEmbedFooter::new("From Lodestone News"))
    }))
}

async fn fetch_pll_info() -> Result<Option<PllInfo>, Error> {
    let cache_store = GameInfoCacheStore::new();
    let now = Utc::now().timestamp();
    if let Some(cached) = cache_store.load_pllinfo().await? {
        if cached.expire_time > now {
            return Ok(Some(cached));
        }
    }

    let client = lodestone_client()?;

    for page in 1..=MAX_PLL_PAGES {
        let list_url = format!("{TOPICS_LIST_URL}?page={page}");
        let list_html = fetch_lodestone_html(&client, &list_url).await;
        let Ok(list_html) = list_html else {
            break;
        };

        let candidates = parse_pll_candidates(&list_html);
        for candidate in candidates {
            let detail_html = fetch_lodestone_html(&client, &candidate.url).await;
            let Ok(detail_html) = detail_html else {
                continue;
            };

            let Some(mut next) = parse_pll_detail(&candidate.url, &detail_html)? else {
                continue;
            };

            next.expire_time = now + PLL_CACHE_DURATION_SECONDS;
            cache_store.save_pllinfo(&next).await?;
            return Ok(Some(next));
        }
    }

    Ok(load_pll_fallback(&cache_store, now).await?)
}

async fn load_pll_fallback(
    cache_store: &GameInfoCacheStore,
    now: i64,
) -> Result<Option<PllInfo>, Error> {
    let Some(cached) = cache_store.load_pllinfo().await? else {
        return Ok(None);
    };

    if cached.expire_time > now {
        return Ok(Some(cached));
    }

    if cached.start_stamp.is_some_and(|stamp| stamp > now) {
        return Ok(Some(cached));
    }

    Ok(None)
}

fn lodestone_client() -> Result<Client, Error> {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(LODESTONE_BROWSER_USER_AGENT),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("ja,en;q=0.9"));

    Ok(Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()?)
}

async fn fetch_lodestone_html(client: &Client, url: &str) -> Result<String, Error> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

fn parse_maintenance_candidates(html: &str) -> Vec<LodestoneLink> {
    parse_detail_links(html, r#"a[href*="/lodestone/news/detail/"]"#)
        .into_iter()
        .filter(|link| is_global_maintenance_title(&link.title))
        .collect()
}

fn parse_pll_candidates(html: &str) -> Vec<LodestoneLink> {
    parse_detail_links(html, r#"a[href*="/lodestone/topics/detail/"]"#)
        .into_iter()
        .filter(|link| is_pll_title(&link.title))
        .collect()
}

fn parse_detail_links(html: &str, selector: &str) -> Vec<LodestoneLink> {
    let document = Html::parse_document(html);
    let selector = Selector::parse(selector).expect("valid selector");
    let mut links = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for node in document.select(&selector) {
        let Some(href) = node.value().attr("href") else {
            continue;
        };
        let Some(url) = absolutize_lodestone_url(href) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }

        let title = normalize_text(&node.text().collect::<Vec<_>>().join(" "));
        if title.is_empty() {
            continue;
        }

        links.push(LodestoneLink { title, url });
    }

    links
}

fn parse_maintenance_detail(url: &str, html: &str) -> Result<Option<MaintInfo>, Error> {
    let Some((title, body)) = parse_article_title_and_body(html) else {
        return Ok(None);
    };
    if !is_global_maintenance_title(&title) {
        return Ok(None);
    }

    let Some((start_stamp, end_stamp)) = parse_maintenance_schedule(&body)? else {
        return Ok(None);
    };

    let kind = classify_maintenance_notice(&title);
    Ok(Some(MaintInfo {
        id: url.to_string(),
        start_stamp,
        end_stamp,
        title_kr: format_maintenance_title(start_stamp, end_stamp, kind),
        url: url.to_string(),
    }))
}

fn parse_pll_detail(url: &str, html: &str) -> Result<Option<PllInfo>, Error> {
    let Some((title, body)) = parse_article_title_and_body(html) else {
        return Ok(None);
    };
    if !is_pll_title(&title) {
        return Ok(None);
    }

    let heading = extract_first_heading(html).unwrap_or_else(|| title.clone());
    let round_number = extract_round_number(&heading).or_else(|| extract_round_number(&title));
    let start_stamp = extract_pll_start(&body)?;

    Ok(Some(PllInfo {
        fixed_title: generate_pll_title(round_number.as_deref(), start_stamp),
        url: url.to_string(),
        start_stamp,
        expire_time: 0,
    }))
}

fn parse_article_title_and_body(html: &str) -> Option<(String, String)> {
    let document = Html::parse_document(html);
    let title_selector = Selector::parse("article h1").expect("valid selector");
    let body_selector = Selector::parse("article .news__detail__wrapper").expect("valid selector");

    let title = document
        .select(&title_selector)
        .next()
        .map(extract_text)
        .filter(|text| !text.is_empty())?;
    let body = document
        .select(&body_selector)
        .next()
        .map(extract_text)
        .filter(|text| !text.is_empty())?;

    Some((title, body))
}

fn extract_first_heading(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let h3_selector = Selector::parse("article h3").expect("valid selector");
    document
        .select(&h3_selector)
        .next()
        .map(extract_text)
        .filter(|text| !text.is_empty())
}

fn extract_text(node: ElementRef<'_>) -> String {
    normalize_text(&node.text().collect::<Vec<_>>().join(" "))
}

fn normalize_text(input: &str) -> String {
    input
        .replace('\u{3000}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn absolutize_lodestone_url(href: &str) -> Option<String> {
    if href.starts_with("https://jp.finalfantasyxiv.com/") {
        return Some(href.to_string());
    }

    if href.starts_with("/lodestone/") {
        return Some(format!("https://jp.finalfantasyxiv.com{href}"));
    }

    None
}

fn is_global_maintenance_title(input: &str) -> bool {
    input.contains("全ワールド") && input.contains("メンテナンス作業")
}

fn is_pll_title(input: &str) -> bool {
    input.contains("FFXIV PLL") || input.contains("プロデューサーレターLIVE")
}

fn extract_round_number(heading: &str) -> Option<String> {
    Regex::new(r"第(\d+)回")
        .expect("valid regex")
        .captures(heading)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_string())
}

fn parse_maintenance_schedule(body_text: &str) -> Result<Option<(i64, i64)>, Error> {
    let regex = Regex::new(
        r"日\s*時[:：]\s*(?P<sy>\d{4})年(?P<sm>\d{1,2})月(?P<sd>\d{1,2})日(?:\([^)]*\)|（[^）]*）)\s*(?P<sh>\d{1,2}):(?P<smin>\d{2})より(?:(?P<ey>\d{4})年)?(?:(?P<em>\d{1,2})月)?(?:(?P<ed>\d{1,2})日(?:\([^)]*\)|（[^）]*）)?)?\s*(?P<eh>\d{1,2}):(?P<emin>\d{2})頃?まで",
    )
    .expect("valid regex");

    let Some(captures) = regex.captures(body_text) else {
        return Ok(None);
    };

    let start_year = capture_u32(&captures, "sy")? as i32;
    let start_month = capture_u32(&captures, "sm")?;
    let start_day = capture_u32(&captures, "sd")?;
    let start_hour = capture_u32(&captures, "sh")?;
    let start_minute = capture_u32(&captures, "smin")?;

    let start_date = NaiveDate::from_ymd_opt(start_year, start_month, start_day)
        .ok_or_else(|| anyhow::anyhow!("invalid maintenance start date"))?;
    let start_naive = start_date
        .and_hms_opt(start_hour, start_minute, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid maintenance start time"))?;
    let start = Tokyo
        .from_local_datetime(&start_naive)
        .single()
        .ok_or_else(|| anyhow::anyhow!("maintenance start date was ambiguous in Asia/Tokyo"))?;

    let mut end_year = captures
        .name("ey")
        .map(|value| value.as_str().parse::<i32>())
        .transpose()?
        .unwrap_or(start_year);
    let end_month = capture_optional_u32(&captures, "em")?.unwrap_or(start_month);
    let end_day = capture_optional_u32(&captures, "ed")?.unwrap_or(start_day);
    let end_hour = capture_u32(&captures, "eh")?;
    let end_minute = capture_u32(&captures, "emin")?;

    if captures.name("ey").is_none()
        && (end_month < start_month || (end_month == start_month && end_day < start_day))
    {
        end_year += 1;
    }

    let end_date = NaiveDate::from_ymd_opt(end_year, end_month, end_day)
        .ok_or_else(|| anyhow::anyhow!("invalid maintenance end date"))?;
    let end_naive = end_date
        .and_hms_opt(end_hour, end_minute, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid maintenance end time"))?;
    let mut end = Tokyo
        .from_local_datetime(&end_naive)
        .single()
        .ok_or_else(|| anyhow::anyhow!("maintenance end date was ambiguous in Asia/Tokyo"))?;

    if captures.name("ey").is_none()
        && captures.name("em").is_none()
        && captures.name("ed").is_none()
        && end <= start
    {
        end = end + ChronoDuration::days(1);
    }

    Ok(Some((start.timestamp(), end.timestamp())))
}

fn capture_u32(captures: &regex::Captures<'_>, name: &str) -> Result<u32, Error> {
    captures
        .name(name)
        .ok_or_else(|| anyhow::anyhow!("missing capture group: {name}"))?
        .as_str()
        .parse::<u32>()
        .map_err(Into::into)
}

fn capture_optional_u32(captures: &regex::Captures<'_>, name: &str) -> Result<Option<u32>, Error> {
    captures
        .name(name)
        .map(|value| value.as_str().parse::<u32>())
        .transpose()
        .map_err(Into::into)
}

fn extract_pll_start(body_text: &str) -> Result<Option<i64>, Error> {
    let regex = Regex::new(r"(\d{4}年\d{1,2}月\d{1,2}日（[^）]+）)\s?(\d{1,2}:\d{2})頃?～")
        .expect("valid regex");

    let Some(captures) = regex.captures(body_text) else {
        return Ok(None);
    };

    let date = captures
        .get(1)
        .map(|value| value.as_str().replace(|c| c == '（' || c == '）', " "))
        .unwrap_or_default();
    let cleaned_date = date.split_whitespace().next().unwrap_or_default();
    let time = captures
        .get(2)
        .map(|value| value.as_str())
        .unwrap_or_default();
    let naive =
        NaiveDateTime::parse_from_str(&format!("{cleaned_date} {time}"), "%Y年%m月%d日 %H:%M")
            .or_else(|_| {
                NaiveDateTime::parse_from_str(
                    &format!("{cleaned_date} {time}"),
                    "%Y年%-m月%-d日 %H:%M",
                )
            })?;

    let local = Tokyo
        .from_local_datetime(&naive)
        .single()
        .ok_or_else(|| anyhow::anyhow!("PLL date was ambiguous in Asia/Tokyo"))?;

    Ok(Some(local.timestamp()))
}

fn classify_maintenance_notice(title: &str) -> MaintenanceNoticeKind {
    let is_follow_up = title.contains("続報") || title.contains("終了時間変更");
    let is_emergency = title.contains("緊急");

    match (is_follow_up, is_emergency) {
        (true, true) => MaintenanceNoticeKind::EmergencyFollowUp,
        (true, false) => MaintenanceNoticeKind::FollowUp,
        (false, true) => MaintenanceNoticeKind::Emergency,
        (false, false) => MaintenanceNoticeKind::Regular,
    }
}

fn generate_pll_title(round_number: Option<&str>, start_stamp: Option<i64>) -> String {
    let round = round_number.unwrap_or("XX");
    let Some(start_stamp) = start_stamp else {
        return format!("제 {round}회 프로듀서 레터 라이브 X월 XX일 방송 결정!");
    };

    let local = Seoul
        .timestamp_opt(start_stamp, 0)
        .single()
        .unwrap_or_else(|| Utc::now().with_timezone(&Seoul));
    format!(
        "제 {round}회 프로듀서 레터 라이브 {}월 {}일 방송 결정!",
        local.month(),
        local.day()
    )
}

fn format_maintenance_title(
    start_stamp: i64,
    end_stamp: i64,
    kind: MaintenanceNoticeKind,
) -> String {
    let start = Tokyo
        .timestamp_opt(start_stamp, 0)
        .single()
        .unwrap_or_else(|| Utc::now().with_timezone(&Tokyo));
    let end = Tokyo
        .timestamp_opt(end_stamp, 0)
        .single()
        .unwrap_or_else(|| Utc::now().with_timezone(&Tokyo));

    let range = if start.month() == end.month() {
        if start.day() == end.day() {
            format!("{}/{}", start.month(), start.day())
        } else {
            format!("{}/{}-{}", start.month(), start.day(), end.day())
        }
    } else {
        format!(
            "{}/{} - {}/{}",
            start.month(),
            start.day(),
            end.month(),
            end.day()
        )
    };

    let prefix = match kind {
        MaintenanceNoticeKind::Regular => "전 월드 유지보수 작업",
        MaintenanceNoticeKind::Emergency => "전 월드 긴급 유지보수 작업",
        MaintenanceNoticeKind::FollowUp => "전 월드 유지보수 작업 종료 시간 변경",
        MaintenanceNoticeKind::EmergencyFollowUp => "전 월드 긴급 유지보수 작업 종료 시간 변경",
    };

    format!("{prefix} ({range})")
}

fn create_maintenance_error_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("점검 정보를 불러올 수 없습니다")
        .description("현재 점검 공지가 없거나 API 업데이트가 되지 않았습니다.")
        .url("https://jp.finalfantasyxiv.com/lodestone")
        .color(ERROR_EMBED_COLOR)
        .thumbnail(GAMEINFO_THUMBNAIL_URL)
        .footer(CreateEmbedFooter::new("From Lodestone News"))
}

fn create_pll_error_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("No PLL Info")
        .description("PLL 관련 정보를 찾을 수 없습니다.")
        .url("https://jp.finalfantasyxiv.com/lodestone")
        .color(ERROR_EMBED_COLOR)
        .thumbnail(GAMEINFO_THUMBNAIL_URL)
}

fn to_valid_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    url::Url::parse(trimmed).ok().map(|value| value.to_string())
}

struct GameInfoCacheStore {
    data_path: PathBuf,
    sample_path: PathBuf,
}

impl GameInfoCacheStore {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../");
        Self {
            data_path: root.join("logs/gameinfo-cache.json"),
            sample_path: root.join("logs/gameinfo-cache.sample.json"),
        }
    }

    async fn load_maintinfo(&self) -> Result<Option<MaintInfo>, Error> {
        let cache = self.load().await?;
        Ok(parse_optional(cache.maintinfo))
    }

    async fn save_maintinfo(&self, info: &MaintInfo) -> Result<(), Error> {
        self.update(|cache| {
            cache.maintinfo = serde_json::to_value(info).expect("serializable maint info");
        })
        .await
    }

    async fn load_pllinfo(&self) -> Result<Option<PllInfo>, Error> {
        let cache = self.load().await?;
        Ok(parse_optional(cache.pllinfo))
    }

    async fn save_pllinfo(&self, info: &PllInfo) -> Result<(), Error> {
        self.update(|cache| {
            let mut value = serde_json::to_value(info).expect("serializable pll info");
            normalize_pll_value(&mut value);
            cache.pllinfo = value;
        })
        .await
    }

    async fn load(&self) -> Result<GameInfoCacheFile, Error> {
        self.ensure_file().await?;
        let text = fs::read_to_string(&self.data_path).await?;
        let value = serde_json::from_str::<GameInfoCacheFile>(&text).unwrap_or_default();
        Ok(value)
    }

    async fn update(&self, mutator: impl FnOnce(&mut GameInfoCacheFile)) -> Result<(), Error> {
        self.ensure_file().await?;
        let mut cache = self.load().await?;
        mutator(&mut cache);
        let payload = serde_json::to_string_pretty(&cache)?;
        let tmp = self.data_path.with_extension("json.tmp");
        fs::write(&tmp, format!("{payload}\n")).await?;
        fs::rename(tmp, &self.data_path).await?;
        Ok(())
    }

    async fn ensure_file(&self) -> Result<(), Error> {
        if fs::try_exists(&self.data_path).await? {
            return Ok(());
        }

        if let Some(parent) = self.data_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        if fs::try_exists(&self.sample_path).await? {
            let content = fs::read(&self.sample_path).await?;
            fs::write(&self.data_path, content).await?;
        } else {
            fs::write(
                &self.data_path,
                "{\n  \"MAINTINFO\": {},\n  \"PLLINFO\": {}\n}\n",
            )
            .await?;
        }

        Ok(())
    }
}

fn parse_optional<T>(value: serde_json::Value) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    if value.is_null() {
        return None;
    }

    match value {
        serde_json::Value::Object(ref map) if map.is_empty() => None,
        other => serde_json::from_value(other).ok(),
    }
}

fn normalize_pll_value(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = value {
        if let Some(expire_time) = map.remove("expire_time") {
            map.insert("expireTime".to_string(), expire_time);
        }
        if let Some(fixed_title) = map.remove("fixed_title") {
            map.insert("fixedTitle".to_string(), fixed_title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MaintenanceNoticeKind, classify_maintenance_notice, extract_pll_start,
        format_maintenance_title, generate_pll_title, parse_maintenance_schedule,
    };

    #[test]
    fn formats_same_day_maintenance_window() {
        assert_eq!(
            format_maintenance_title(1_750_104_000, 1_750_154_400, MaintenanceNoticeKind::Regular,),
            "전 월드 유지보수 작업 (6/17)"
        );
    }

    #[test]
    fn formats_emergency_maintenance_window() {
        assert_eq!(
            format_maintenance_title(
                1_750_104_000,
                1_750_154_400,
                MaintenanceNoticeKind::Emergency,
            ),
            "전 월드 긴급 유지보수 작업 (6/17)"
        );
    }

    #[test]
    fn generates_pll_title_with_round_and_date() {
        assert_eq!(
            generate_pll_title(Some("87"), Some(1_750_417_200)),
            "제 87회 프로듀서 레터 라이브 6월 20일 방송 결정!"
        );
    }

    #[test]
    fn generates_pll_title_without_date() {
        assert_eq!(
            generate_pll_title(Some("XX"), None),
            "제 XX회 프로듀서 레터 라이브 X월 XX일 방송 결정!"
        );
    }

    #[test]
    fn parses_regular_maintenance_schedule() {
        let body = "記 日　時：2026年3月24日(火) 15:00より19:00頃まで ※終了予定時刻に関しては、状況により変更する場合があります。";
        let parsed = parse_maintenance_schedule(body)
            .expect("schedule parsed")
            .expect("schedule present");
        assert_eq!(parsed.0, 1_774_332_000);
        assert_eq!(parsed.1, 1_774_346_400);
    }

    #[test]
    fn parses_follow_up_maintenance_schedule() {
        let body = "記 日　時：2026年2月5日(木) 14:00より17:10頃まで 対　象：ファイナルファンタジーXIVをご利用のお客様";
        let parsed = parse_maintenance_schedule(body)
            .expect("schedule parsed")
            .expect("schedule present");
        assert_eq!(parsed.0, 1_770_267_600);
        assert_eq!(parsed.1, 1_770_279_000);
    }

    #[test]
    fn parses_pll_start_from_detail_text() {
        let body = "第91回 FFXIVプロデューサーレターLIVE 日時 2026年3月13日（金）20:00頃～ ※開始時間は変更される場合があります。";
        let start = extract_pll_start(body)
            .expect("pll parse succeeded")
            .expect("pll start exists");
        assert_eq!(start, 1_773_399_600);
    }

    #[test]
    fn classifies_follow_up_emergency_maintenance() {
        assert_eq!(
            classify_maintenance_notice(
                "[続報]全ワールド 緊急メンテナンス作業 終了時間変更のお知らせ(12/25)"
            ),
            MaintenanceNoticeKind::EmergencyFollowUp
        );
    }
}
