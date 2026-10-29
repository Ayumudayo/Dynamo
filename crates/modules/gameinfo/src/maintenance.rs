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
use std::collections::HashSet;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, RwLock};

const MAINTENANCE_LIST_URL: &str = "https://jp.finalfantasyxiv.com/lodestone/news/category/2";
const MAX_MAINTENANCE_CANDIDATES: usize = 6;
const EXPIRED_MAINTENANCE_CACHE_GRACE_SECONDS: i64 = 24 * 60 * 60;
static MAINTENANCE_INFO: OnceLock<Arc<RwLock<Option<MaintInfo>>>> = OnceLock::new();
static POLLER_STARTED: AtomicBool = AtomicBool::new(false);
static COMPLETED_NOTICES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn shared_info() -> Arc<RwLock<Option<MaintInfo>>> {
    MAINTENANCE_INFO
        .get_or_init(|| Arc::new(RwLock::new(None)))
        .clone()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MaintInfo {
    pub(crate) id: String,
    pub(crate) start_stamp: i64,
    pub(crate) end_stamp: i64,
    #[serde(default)]
    pub(crate) end_time_unknown: bool,
    pub(crate) title_kr: String,
    pub(crate) url: String,
    #[serde(default = "default_maintenance_scope")]
    pub(crate) scope: String,
    #[serde(default)]
    pub(crate) body_fingerprint: u64,
    pub(crate) translated_description: Option<String>,
    #[serde(skip)]
    pub(crate) stale: bool,
}

fn default_maintenance_scope() -> String {
    "전 월드".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaintenanceNoticeKind {
    Regular,
    Emergency,
    FollowUp,
    EmergencyFollowUp,
}

pub(crate) async fn fetch_maintenance_info() -> Result<Option<MaintInfo>, Error> {
    Ok(shared_info().read().await.clone())
}

pub fn spawn_maintenance_poll_loop() {
    if POLLER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = shared_info();
    tokio::spawn(async move {
        if let Ok(cached) = GameInfoCacheStore::new().load_maintinfo().await {
            *state.write().await = cached.map(mark_stale);
        }
        let mut consecutive_failures = 0u32;
        loop {
            match refresh_maintenance_info().await {
                Ok(next) => {
                    consecutive_failures = 0;
                    *state.write().await = next;
                }
                Err(error) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    if consecutive_failures == 1 {
                        tracing::warn!(%error, "maintenance polling failed; retrying with bounded backoff");
                    }
                    if let Some(info) = state.write().await.as_mut() {
                        info.stale = true;
                    }
                }
            }
            let now = Utc::now().timestamp();
            let info = state.read().await.clone();
            let interval = if consecutive_failures > 0 {
                match consecutive_failures {
                    1 => 60,
                    2 => 120,
                    _ => 300,
                }
            } else {
                next_maintenance_poll_seconds(info.as_ref(), now)
            };
            tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        }
    });
}

async fn refresh_maintenance_info() -> Result<Option<MaintInfo>, Error> {
    let cache_store = GameInfoCacheStore::new();
    let now = Utc::now().timestamp();
    let client = lodestone_client()?;
    let cached = cache_store.load_maintinfo().await?;
    let wants_translation = translate_api_key().is_some();

    let list_html = fetch_lodestone_html(&client, MAINTENANCE_LIST_URL).await?;

    let candidates = parse_maintenance_candidates(&list_html);
    for candidate in candidates.into_iter().take(MAX_MAINTENANCE_CANDIDATES) {
        if !should_recheck_notice_detail(&candidate.url, cached.as_ref(), now) {
            // The listing is newest first. Preserve this cached notice rather
            // than replacing it with an older original notice below it.
            break;
        }
        if COMPLETED_NOTICES
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .await
            .contains(&candidate.url)
        {
            continue;
        }
        let detail_html = fetch_lodestone_html(&client, &candidate.url).await?;

        // A cancellation notice supersedes any older scheduled notice. Do not
        // fall back to a still-cached window after observing the cancellation.
        if let Some((title, body)) = parse_article_title_and_body(&detail_html)
            && is_japan_maintenance_title(&title)
            && is_cancelled_maintenance_notice(&title, &body)
        {
            COMPLETED_NOTICES
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
                .await
                .insert(candidate.url.clone());
            let related = cached.as_ref().is_none_or(|info| {
                info.id == candidate.url
                    || (info.scope == maintenance_scope(&title, &body)
                        && cancellation_mentions_start_date(info, &body))
            });
            if related {
                cache_store.clear_maintinfo().await?;
                return Ok(None);
            }
            continue;
        }

        let Some(mut next) = parse_maintenance_detail(&candidate.url, &detail_html)? else {
            anyhow::bail!("maintenance detail could not be parsed: {}", candidate.url);
        };
        if should_skip_historical_notice(&next, cached.as_ref(), now) {
            COMPLETED_NOTICES
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
                .await
                .insert(candidate.url);
            continue;
        }
        if let Some((_, body)) = parse_article_title_and_body(&detail_html) {
            next.body_fingerprint = fingerprint(&body);
        }

        if wants_translation {
            let translation_unchanged = cached.as_ref().is_some_and(|info| {
                info.id == next.id
                    && info.start_stamp == next.start_stamp
                    && info.end_stamp == next.end_stamp
                    && info.body_fingerprint == next.body_fingerprint
                    && info.translated_description.is_some()
            });
            if translation_unchanged {
                next.translated_description = cached
                    .as_ref()
                    .and_then(|info| info.translated_description.clone());
            } else {
                let summary_ja = extract_maintenance_summary_ja(&detail_html);
                next.translated_description =
                    translate_optional_text(&client, summary_ja.as_deref()).await;
            }
        }

        let changed = cached.as_ref().is_none_or(|info| {
            info.id != next.id
                || info.start_stamp != next.start_stamp
                || info.end_stamp != next.end_stamp
                || info.end_time_unknown != next.end_time_unknown
                || info.body_fingerprint != next.body_fingerprint
                || info.scope != next.scope
                || info.translated_description != next.translated_description
        });
        if changed {
            cache_store.save_maintinfo(&next).await?;
        }
        return Ok(Some(next));
    }

    Ok(cached.map(mark_stale))
}

pub(crate) fn should_skip_historical_notice(
    info: &MaintInfo,
    cached: Option<&MaintInfo>,
    now: i64,
) -> bool {
    // Preserve the selected schedule for display and listing-poll cadence.
    // This does not require repeatedly fetching its expired detail page.
    !info.end_time_unknown
        && !is_cache_usable(info, now)
        && !cached.is_some_and(|cached| cached.url == info.url)
}

pub(crate) fn should_recheck_notice_detail(
    url: &str,
    cached: Option<&MaintInfo>,
    now: i64,
) -> bool {
    cached.is_none_or(|info| info.url != url || info.end_time_unknown || info.end_stamp > now)
}

fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

pub(crate) fn next_maintenance_poll_seconds(info: Option<&MaintInfo>, now: i64) -> u64 {
    let Some(info) = info else {
        return 300;
    };
    if info.end_time_unknown {
        if info
            .start_stamp
            .saturating_add(EXPIRED_MAINTENANCE_CACHE_GRACE_SECONDS)
            <= now
        {
            return 300;
        }
        let boundary = info.start_stamp.saturating_sub(3600);
        return if boundary > now {
            300_u64.min((boundary - now) as u64)
        } else {
            60
        };
    }
    if info
        .end_stamp
        .saturating_add(EXPIRED_MAINTENANCE_CACHE_GRACE_SECONDS)
        <= now
    {
        return 300;
    }
    let boundary = info.start_stamp.saturating_sub(3600);
    if boundary > now {
        return 300_u64.min((boundary - now) as u64);
    }
    60
}

fn is_cache_usable(info: &MaintInfo, now: i64) -> bool {
    if info.end_time_unknown {
        return info
            .start_stamp
            .saturating_add(EXPIRED_MAINTENANCE_CACHE_GRACE_SECONDS)
            > now;
    }
    info.end_stamp
        .saturating_add(EXPIRED_MAINTENANCE_CACHE_GRACE_SECONDS)
        > now
}

fn mark_stale(mut info: MaintInfo) -> MaintInfo {
    info.stale = true;
    info
}

fn parse_maintenance_candidates(html: &str) -> Vec<crate::lodestone::LodestoneLink> {
    parse_detail_links(html, r#"a[href*="/lodestone/news/detail/"]"#)
        .into_iter()
        .filter(|link| is_japan_maintenance_title(&link.title))
        .collect()
}

fn parse_maintenance_detail(url: &str, html: &str) -> Result<Option<MaintInfo>, Error> {
    let Some((title, body)) = parse_article_title_and_body(html) else {
        return Ok(None);
    };
    if !is_japan_maintenance_title(&title) {
        return Ok(None);
    }

    let (start_stamp, end_stamp, end_time_unknown) = match parse_maintenance_schedule(&body)? {
        Some((start, end)) => (start, end, false),
        None if body.contains("終了時刻未定") || body.contains("終了時間未定") => {
            let Some(start) = parse_maintenance_start_stamp(&body)? else {
                return Ok(None);
            };
            (start, 0, true)
        }
        None => return Ok(None),
    };
    let scope = maintenance_scope(&title, &body);
    let display_title = format_maintenance_title(
        start_stamp,
        if end_time_unknown {
            start_stamp
        } else {
            end_stamp
        },
        classify_maintenance_notice(&title),
    );
    let display_title = if scope == "전 월드" {
        display_title
    } else {
        display_title.replacen("전 월드", scope, 1)
    };

    Ok(Some(MaintInfo {
        id: url.to_string(),
        start_stamp,
        end_stamp,
        end_time_unknown,
        title_kr: display_title,
        url: url.to_string(),
        scope: scope.to_string(),
        body_fingerprint: fingerprint(&body),
        translated_description: None,
        stale: false,
    }))
}

fn is_global_maintenance_title(input: &str) -> bool {
    input.contains("全ワールド") && input.contains("メンテナンス作業")
}

fn is_japan_maintenance_title(input: &str) -> bool {
    input.contains("メンテナンス作業")
        && (is_global_maintenance_title(input)
            || input.contains("日本データセンター")
            || input.contains("日本DC"))
}

pub(crate) fn maintenance_scope(title: &str, body: &str) -> &'static str {
    if (title.contains("PS4") || body.contains("PlayStation®4")) && body.contains("日本リージョン")
    {
        "일본 리전 · PS4 한정"
    } else if title.contains("日本データセンター") || title.contains("日本DC") {
        "일본 DC 한정"
    } else {
        "전 월드"
    }
}

fn is_cancelled_maintenance_notice(title: &str, body: &str) -> bool {
    title.contains("中止")
        || body.contains("メンテナンス作業を中止")
        || body.contains("メンテナンス作業は中止")
        || body.contains("実施を中止")
}

fn cancellation_mentions_start_date(info: &MaintInfo, body: &str) -> bool {
    let date = Tokyo.timestamp_opt(info.start_stamp, 0).single();
    date.is_some_and(|date| {
        body.contains(&format!(
            "{}年{}月{}日",
            date.year(),
            date.month(),
            date.day()
        ))
    })
}

pub(crate) fn build_maintenance_embed(info: &MaintInfo) -> CreateEmbed {
    let now = Utc::now().timestamp();
    let mut embed = CreateEmbed::new()
        .title(&info.title_kr)
        .url(&info.url)
        .color(SUCCESS_EMBED_COLOR)
        .thumbnail(GAMEINFO_THUMBNAIL_URL)
        .timestamp(Timestamp::now())
        .field("시작 시각", format!("<t:{}:F>", info.start_stamp), false)
        .field(
            "종료 시각",
            if info.end_time_unknown {
                "미정".to_string()
            } else {
                format!("<t:{}:F>", info.end_stamp)
            },
            false,
        )
        .field("대상 범위", &info.scope, true);
    if !info.end_time_unknown && info.end_stamp > now {
        embed = embed.field(
            "종료까지 남은 시간",
            format!("<t:{}:R>", info.end_stamp),
            false,
        );
    }

    if let Some(description) = info.translated_description.as_deref()
        && !description.trim().is_empty()
    {
        embed = embed.description(description);
    }

    if info.stale {
        let footer = if info.end_time_unknown {
            "캐시된 점검 정보 · 종료 시각 미정 · Lodestone에서 갱신 여부를 확인하지 못함"
        } else if info.end_stamp > Utc::now().timestamp() {
            "캐시된 점검 정보 · Lodestone에서 갱신 여부를 확인하지 못함"
        } else {
            "캐시된 정보 · 예정 종료 시각 경과 · 점검 종료 여부 미확인"
        };
        embed = embed.footer(CreateEmbedFooter::new(footer));
    } else if !info.end_time_unknown && info.end_stamp <= now {
        embed = embed.footer(CreateEmbedFooter::new(
            "예정 종료 시각 경과 · 점검 종료 여부 미확인",
        ));
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
        r"(?:日\s*時[:：]\s*)?(?P<sy>\d{4})年(?P<sm>\d{1,2})月(?P<sd>\d{1,2})日(?:\([^)]*\)|（[^）]*）)\s*(?P<sh>\d{1,2}):(?P<smin>\d{2})より\s*(?:(?P<ey>\d{4})年)?(?:(?P<em>\d{1,2})月)?(?:(?P<ed>\d{1,2})日(?:\([^)]*\)|（[^）]*）)?)?\s*(?P<eh>\d{1,2}):(?P<emin>\d{2})頃?まで",
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

pub(crate) fn parse_maintenance_start_stamp(body_text: &str) -> Result<Option<i64>, Error> {
    let regex = Regex::new(r"(?P<y>\d{4})年(?P<m>\d{1,2})月(?P<d>\d{1,2})日(?:\([^)]*\)|（[^）]*）)\s*(?P<h>\d{1,2}):(?P<min>\d{2})より")
        .expect("valid start-time regex");
    let Some(c) = regex.captures(body_text) else {
        return Ok(None);
    };
    let get = |name: &str| -> Result<u32, Error> {
        Ok(c.name(name)
            .ok_or_else(|| anyhow::anyhow!("missing {name}"))?
            .as_str()
            .parse()?)
    };
    let date = NaiveDate::from_ymd_opt(get("y")? as i32, get("m")?, get("d")?)
        .ok_or_else(|| anyhow::anyhow!("invalid maintenance start date"))?;
    let naive = date
        .and_hms_opt(get("h")?, get("min")?, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid maintenance start time"))?;
    let stamp = Tokyo
        .from_local_datetime(&naive)
        .single()
        .ok_or_else(|| anyhow::anyhow!("maintenance start date was ambiguous in Asia/Tokyo"))?;
    Ok(Some(stamp.timestamp()))
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
    let is_follow_up =
        title.contains("続報") || title.contains("終了時間変更") || title.contains("延長");
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
