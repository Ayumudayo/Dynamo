use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Asia::{Seoul, Tokyo};
use dynamo_core::{
    Context, DiscordCommand, Error, GatewayIntents, Module, ModuleCategory, ModuleManifest,
    SettingsField, SettingsFieldKind, SettingsSchema, SettingsSection, module_access_for_context,
};
use poise::serenity_prelude::CreateEmbed;
use regex::Regex;
use reqwest::Client;
use rss::Channel;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use tokio::fs;

const MODULE_ID: &str = "gameinfo";
const DEFAULT_WT_LINK: &str = "http://warthunder.com/en/registration?r=userinvite_18945695";
const DEFAULT_WOT_LINK: &str =
    "https://worldoftanks.asia/referral/9ed8df012d204670b04c1cc1c88d98d5";
const DEFAULT_THUMBNAIL_URL: &str = "https://media.discordapp.net/attachments/1138398345065414657/1329005700730585118/png-clipart-war-thunder-playstation-4-aircraft-airplane-macchi-c-202-thunder-game-video-game-removebg-preview.png?ex=6788c482&is=67877302&hm=31b9ed755040306ea8d1c9db258ffaa590df7e3bfa6139d875c62915d46c1b73&=&format=webp&quality=lossless";
const MAINTENANCE_URL: &str = "https://lodestonenews.com/news/maintenance/current";
const TOPICS_RSS_URL: &str = "https://jp.finalfantasyxiv.com/lodestone/news/topics.xml";
const PLL_CACHE_DURATION_SECONDS: i64 = 12 * 60 * 60;

pub struct GameInfoModule;

impl Module for GameInfoModule {
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

    fn commands(&self) -> Vec<DiscordCommand> {
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

#[derive(Debug, Deserialize)]
struct MaintenanceResponse {
    game: Vec<MaintenanceItem>,
}

#[derive(Debug, Deserialize)]
struct MaintenanceItem {
    id: String,
    start: String,
    end: String,
    url: String,
}

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
    let mut embed = CreateEmbed::new()
        .title(settings.title)
        .field(
            "War Thunder",
            format!("[Open referral link]({})", settings.wt_link),
            false,
        )
        .field(
            "World of Tanks",
            format!("[Open referral link]({})", settings.wot_link),
            false,
        );

    if !settings.thumbnail_url.trim().is_empty() {
        embed = embed.thumbnail(settings.thumbnail_url);
    }

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

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
            .field("Start time", format!("<t:{}:F>", info.start_stamp), false)
            .field("End time", format!("<t:{}:F>", info.end_stamp), false)
            .field("Time remaining", format!("<t:{}:R>", info.end_stamp), false)
            .footer(poise::serenity_prelude::CreateEmbedFooter::new(
                "From Lodestone News",
            ))
            .timestamp(timestamp_from_unix(info.end_stamp))
    }))
}

async fn fetch_maintenance_info() -> Result<Option<MaintInfo>, Error> {
    let cache_store = GameInfoCacheStore::new();
    let now = Utc::now().timestamp();
    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;

    let response = client.get(MAINTENANCE_URL).send().await;
    let Ok(response) = response else {
        return Ok(cache_store
            .load_maintinfo()
            .await?
            .filter(|info| info.end_stamp > now));
    };

    let parsed = response.json::<MaintenanceResponse>().await;
    let Ok(parsed) = parsed else {
        return Ok(cache_store
            .load_maintinfo()
            .await?
            .filter(|info| info.end_stamp > now));
    };

    let Some(item) = parsed.game.into_iter().next() else {
        return Ok(cache_store
            .load_maintinfo()
            .await?
            .filter(|info| info.end_stamp > now));
    };

    let start = parse_iso_timestamp(&item.start)?;
    let end = parse_iso_timestamp(&item.end)?;
    let next = MaintInfo {
        id: item.id,
        start_stamp: start,
        end_stamp: end,
        title_kr: format_maintenance_title(start, end),
        url: item.url,
    };

    if next.end_stamp <= now {
        return Ok(cache_store
            .load_maintinfo()
            .await?
            .filter(|info| info.end_stamp > now));
    }

    cache_store.save_maintinfo(&next).await?;
    Ok(Some(next))
}

async fn get_pll_embed() -> Result<Option<CreateEmbed>, Error> {
    let info = fetch_pll_info().await?;
    Ok(info.map(|info| {
        let mut embed = CreateEmbed::new()
            .title(info.fixed_title)
            .url(info.url)
            .footer(poise::serenity_prelude::CreateEmbedFooter::new(
                "From Lodestone News",
            ));

        let time_value = info
            .start_stamp
            .map(|stamp| format!("<t:{stamp}:F>"))
            .unwrap_or_else(|| "Unavailable".to_string());
        let relative = info
            .start_stamp
            .map(|stamp| format!("<t:{stamp}:R>"))
            .unwrap_or_else(|| "Unavailable".to_string());

        embed = embed.field("Broadcast start", time_value, false).field(
            "Time remaining",
            relative,
            false,
        );

        if let Some(stamp) = info.start_stamp {
            embed = embed.timestamp(timestamp_from_unix(stamp));
        }

        embed
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

    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let response = client.get(TOPICS_RSS_URL).send().await;
    let Ok(response) = response else {
        return Ok(load_pll_fallback(&cache_store, now).await?);
    };

    let xml = response.text().await;
    let Ok(xml) = xml else {
        return Ok(load_pll_fallback(&cache_store, now).await?);
    };

    let channel = Channel::read_from(xml.as_bytes());
    let Ok(channel) = channel else {
        return Ok(load_pll_fallback(&cache_store, now).await?);
    };

    let Some(item) = find_pll_item(&channel) else {
        return Ok(load_pll_fallback(&cache_store, now).await?);
    };

    let summary = item.description().unwrap_or_default();
    let heading = extract_heading_text(summary, item.title().unwrap_or_default());
    let round_number = extract_round_number(&heading);
    let start_stamp = extract_pll_start(summary)?;
    let next = PllInfo {
        fixed_title: generate_pll_title(round_number.as_deref(), start_stamp),
        url: item
            .link()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "https://jp.finalfantasyxiv.com/lodestone".to_string()),
        start_stamp,
        expire_time: now + PLL_CACHE_DURATION_SECONDS,
    };

    cache_store.save_pllinfo(&next).await?;
    Ok(Some(next))
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

fn find_pll_item(channel: &Channel) -> Option<rss::Item> {
    channel
        .items()
        .iter()
        .find(|item| {
            let title = item.title().unwrap_or_default();
            is_pll_title(title)
                || is_pll_title(&extract_heading_text(
                    item.description().unwrap_or_default(),
                    title,
                ))
        })
        .cloned()
}

fn is_pll_title(input: &str) -> bool {
    Regex::new(r"第\d+回\s?FFXIV\s?PLL")
        .expect("valid regex")
        .is_match(input)
}

fn extract_heading_text(summary_html: &str, fallback: &str) -> String {
    let fragment = Html::parse_fragment(summary_html);
    let specific = Selector::parse("h3.mdl-title__heading--lg").expect("valid selector");
    let generic = Selector::parse("h3").expect("valid selector");

    if let Some(node) = fragment.select(&specific).next() {
        let text = node.text().collect::<Vec<_>>().join(" ").trim().to_string();
        if !text.is_empty() {
            return text;
        }
    }

    if let Some(node) = fragment.select(&generic).next() {
        let text = node.text().collect::<Vec<_>>().join(" ").trim().to_string();
        if !text.is_empty() {
            return text;
        }
    }

    fallback.to_string()
}

fn extract_summary_text(summary_html: &str) -> String {
    Html::parse_fragment(summary_html)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_round_number(heading: &str) -> Option<String> {
    Regex::new(r"第(\d+)回")
        .expect("valid regex")
        .captures(heading)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_string())
}

fn extract_pll_start(summary_html: &str) -> Result<Option<i64>, Error> {
    let summary_text = extract_summary_text(summary_html);
    let regex = Regex::new(r"(\d{4}年\d{1,2}月\d{1,2}日（[^）]+）)\s?(\d{1,2}:\d{2})頃?～")
        .expect("valid regex");

    let Some(captures) = regex.captures(&summary_text) else {
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

fn generate_pll_title(round_number: Option<&str>, start_stamp: Option<i64>) -> String {
    let round = round_number.unwrap_or("XX");
    let Some(start_stamp) = start_stamp else {
        return format!("Round {round} Producer Letter Live date to be announced");
    };

    let local = Seoul
        .timestamp_opt(start_stamp, 0)
        .single()
        .unwrap_or_else(|| Utc::now().with_timezone(&Seoul));
    format!(
        "Round {round} Producer Letter Live scheduled on {}/{}",
        local.month(),
        local.day()
    )
}

fn format_maintenance_title(start_stamp: i64, end_stamp: i64) -> String {
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

    format!("Global maintenance window ({range})")
}

fn parse_iso_timestamp(input: &str) -> Result<i64, Error> {
    Ok(DateTime::parse_from_rfc3339(input)?.timestamp())
}

fn timestamp_from_unix(unix: i64) -> poise::serenity_prelude::Timestamp {
    poise::serenity_prelude::Timestamp::from_unix_timestamp(unix)
        .unwrap_or_else(|_| poise::serenity_prelude::Timestamp::now())
}

fn create_maintenance_error_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("Cannot load maintenance data")
        .description("No maintenance information is currently available.")
        .url("https://jp.finalfantasyxiv.com/lodestone")
        .footer(poise::serenity_prelude::CreateEmbedFooter::new(
            "From Lodestone News",
        ))
}

fn create_pll_error_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("No PLL Info")
        .description("No producer letter schedule found.")
        .url("https://jp.finalfantasyxiv.com/lodestone")
        .footer(poise::serenity_prelude::CreateEmbedFooter::new(
            "From Lodestone News",
        ))
}

struct GameInfoCacheStore {
    data_path: PathBuf,
    sample_path: PathBuf,
}

impl GameInfoCacheStore {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../");
        Self {
            data_path: root.join("src/data/gameinfo-cache.json"),
            sample_path: root.join("src/data/gameinfo-cache.sample.json"),
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
    use super::{format_maintenance_title, generate_pll_title};

    #[test]
    fn formats_same_day_maintenance_window() {
        assert_eq!(
            format_maintenance_title(1_750_104_000, 1_750_154_400),
            "Global maintenance window (6/17)"
        );
    }

    #[test]
    fn generates_pll_title_with_round_and_date() {
        assert_eq!(
            generate_pll_title(Some("87"), Some(1_750_417_200)),
            "Round 87 Producer Letter Live scheduled on 6/20"
        );
    }
}
