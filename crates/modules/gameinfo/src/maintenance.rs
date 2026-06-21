use crate::{
    cache::GameInfoCacheStore,
    constants::{ERROR_EMBED_COLOR, GAMEINFO_THUMBNAIL_URL, SUCCESS_EMBED_COLOR},
    lodestone::{
        fetch_lodestone_html, lodestone_client, parse_article_title_and_body, parse_detail_links,
    },
    translate::{translate_api_key, translate_optional_text},
};
use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use chrono_tz::Asia::Tokyo;
use dynamo_runtime_api::Error;
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, Timestamp};
use regex::Regex;
use serde::{Deserialize, Serialize};

const MAINTENANCE_LIST_URL: &str = "https://jp.finalfantasyxiv.com/lodestone/news/category/2";
const MAX_MAINTENANCE_CANDIDATES: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MaintInfo {
    pub(crate) id: String,
    pub(crate) start_stamp: i64,
    pub(crate) end_stamp: i64,
    pub(crate) title_kr: String,
    pub(crate) url: String,
    pub(crate) translated_description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaintenanceNoticeKind {
    Regular,
    Emergency,
    FollowUp,
    EmergencyFollowUp,
}

pub(crate) async fn fetch_maintenance_info() -> Result<Option<MaintInfo>, Error> {
    let cache_store = GameInfoCacheStore::new();
    let now = Utc::now().timestamp();
    let client = lodestone_client()?;
    let cached = cache_store.load_maintinfo().await?;
    let wants_translation = translate_api_key().is_some();

    let list_html = fetch_lodestone_html(&client, MAINTENANCE_LIST_URL).await;
    let Ok(list_html) = list_html else {
        return Ok(cached.filter(|info| info.end_stamp > now));
    };

    let candidates = parse_maintenance_candidates(&list_html);
    for candidate in candidates.into_iter().take(MAX_MAINTENANCE_CANDIDATES) {
        if let Some(info) = cached.as_ref() {
            let translation_ready = !wants_translation || info.translated_description.is_some();
            if info.id == candidate.url && info.end_stamp > now && translation_ready {
                return Ok(Some(info.clone()));
            }
        }

        let detail_html = fetch_lodestone_html(&client, &candidate.url).await;
        let Ok(detail_html) = detail_html else {
            continue;
        };

        let Some(mut next) = parse_maintenance_detail(&candidate.url, &detail_html)? else {
            continue;
        };

        if wants_translation {
            let summary_ja = extract_maintenance_summary_ja(&detail_html);
            next.translated_description =
                translate_optional_text(&client, summary_ja.as_deref()).await;
        }

        if next.end_stamp <= now {
            continue;
        }

        cache_store.save_maintinfo(&next).await?;
        return Ok(Some(next));
    }

    Ok(cached.filter(|info| info.end_stamp > now))
}

fn parse_maintenance_candidates(html: &str) -> Vec<crate::lodestone::LodestoneLink> {
    parse_detail_links(html, r#"a[href*="/lodestone/news/detail/"]"#)
        .into_iter()
        .filter(|link| is_global_maintenance_title(&link.title))
        .collect()
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
        translated_description: None,
    }))
}

fn is_global_maintenance_title(input: &str) -> bool {
    input.contains("全ワールド") && input.contains("メンテナンス作業")
}

pub(crate) fn build_maintenance_embed(info: &MaintInfo) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title(&info.title_kr)
        .url(&info.url)
        .color(SUCCESS_EMBED_COLOR)
        .thumbnail(GAMEINFO_THUMBNAIL_URL)
        .timestamp(Timestamp::now())
        .field("시작 시각", format!("<t:{}:F>", info.start_stamp), false)
        .field("종료 시각", format!("<t:{}:F>", info.end_stamp), false)
        .field(
            "종료까지 남은 시간",
            format!("<t:{}:R>", info.end_stamp),
            false,
        );

    if let Some(description) = info.translated_description.as_deref()
        && !description.trim().is_empty()
    {
        embed = embed.description(description);
    }

    embed
}

pub(crate) fn create_maintenance_error_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("점검 정보를 불러올 수 없습니다")
        .description("현재 점검 공지가 없거나 API 업데이트가 되지 않았습니다.")
        .url("https://jp.finalfantasyxiv.com/lodestone")
        .color(ERROR_EMBED_COLOR)
        .thumbnail(GAMEINFO_THUMBNAIL_URL)
        .footer(CreateEmbedFooter::new("From Lodestone News"))
}

pub(crate) fn parse_maintenance_schedule(body_text: &str) -> Result<Option<(i64, i64)>, Error> {
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
        end += ChronoDuration::days(1);
    }

    Ok(Some((start.timestamp(), end.timestamp())))
}

pub(crate) fn extract_maintenance_summary_ja(html: &str) -> Option<String> {
    let (_, body) = parse_article_title_and_body(html)?;
    let marker = Regex::new(r"\s記\s+日\s*時[:：]").expect("valid regex");
    let summary = marker
        .find(&body)
        .map(|matched| body[..matched.start()].trim())
        .unwrap_or_else(|| body.trim());
    let summary = take_first_sentence(&trim_maintenance_intro(summary));
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

fn take_first_sentence(input: &str) -> String {
    let trimmed = input.trim();
    if let Some((head, _)) = trimmed.split_once('。') {
        format!("{head}。")
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn trim_maintenance_intro(summary: &str) -> String {
    let intro =
        Regex::new(r"^下記日時にお(?:きまして|いて)、?").expect("valid maintenance intro regex");
    intro.replace(summary.trim(), "").trim().to_string()
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

pub(crate) fn classify_maintenance_notice(title: &str) -> MaintenanceNoticeKind {
    let is_follow_up = title.contains("続報") || title.contains("終了時間変更");
    let is_emergency = title.contains("緊急");

    match (is_follow_up, is_emergency) {
        (true, true) => MaintenanceNoticeKind::EmergencyFollowUp,
        (true, false) => MaintenanceNoticeKind::FollowUp,
        (false, true) => MaintenanceNoticeKind::Emergency,
        (false, false) => MaintenanceNoticeKind::Regular,
    }
}

pub(crate) fn format_maintenance_title(
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
