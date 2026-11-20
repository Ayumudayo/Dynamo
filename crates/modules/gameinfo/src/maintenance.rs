use crate::{
    cache::GameInfoCacheStore,
    constants::{ERROR_EMBED_COLOR, SUCCESS_EMBED_COLOR},
    lodestone::{
        LodestoneLink, fetch_lodestone_html, lodestone_client, parse_article_title_and_body,
        parse_detail_links,
    },
    translate::{translate_api_key, translate_optional_text},
};
use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use chrono_tz::Asia::Tokyo;
use dynamo_runtime_api::Error;
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, Timestamp};
use regex::Regex;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::HashSet;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, RwLock};

const MAINTENANCE_LIST_URL: &str = "https://jp.finalfantasyxiv.com/lodestone/news/category/2";
const NOTICE_LINKS: &str = r#"a[href*="/lodestone/news/detail/"]"#;
const MAINTENANCE_MODE_NOTICE_LINKS: &str = r#"body.error .ldst__error #toptabchanger_newsarea a.ic__maintenance--list[href*="/lodestone/news/detail/"]"#;
const MAX_MAINTENANCE_CANDIDATES: usize = 6;
const MAX_COMPLETED_NOTICES: usize = 256;
const EXPIRED_MAINTENANCE_CACHE_GRACE_SECONDS: i64 = 24 * 60 * 60;
static MAINTENANCE_INFO: OnceLock<Arc<RwLock<Option<MaintInfo>>>> = OnceLock::new();
static POLLER_STARTED: AtomicBool = AtomicBool::new(false);
static INITIAL_POLL_COMPLETE: AtomicBool = AtomicBool::new(false);
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
    #[serde(default)]
    pub(crate) completed: bool,
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

/// Persist the cancelled event, rather than only the notice URL. Follow-ups
/// have different URLs and must not allow the original schedule to reappear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MaintenanceCancellation {
    pub(crate) notice_url: String,
    pub(crate) original_url: Option<String>,
    pub(crate) scope: String,
    pub(crate) start_stamp: Option<i64>,
    pub(crate) start_date: Option<String>,
}

impl MaintenanceCancellation {
    pub(crate) fn matches(&self, info: &MaintInfo) -> bool {
        if self.notice_url == info.url || self.original_url.as_deref() == Some(&info.url) {
            return true;
        }
        if self.scope != info.scope {
            return false;
        }
        if let Some(start) = self.start_stamp {
            return start == info.start_stamp;
        }
        self.start_date.as_ref().is_some_and(|date| {
            Tokyo
                .timestamp_opt(info.start_stamp, 0)
                .single()
                .is_some_and(|start| start.format("%Y-%m-%d").to_string() == *date)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaintenanceNoticeKind {
    Regular,
    Emergency,
    FollowUp,
    EmergencyFollowUp,
    Completed,
    EmergencyCompleted,
}

pub(crate) async fn fetch_maintenance_info() -> Result<Option<MaintInfo>, Error> {
    if shared_info().read().await.is_none()
        && POLLER_STARTED.load(Ordering::SeqCst)
        && !INITIAL_POLL_COMPLETE.load(Ordering::Acquire)
    {
        for _ in 0..15 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if INITIAL_POLL_COMPLETE.load(Ordering::Acquire) || shared_info().read().await.is_some()
            {
                break;
            }
        }
    }
    let now = Utc::now().timestamp();
    let current = shared_info().read().await.clone();
    Ok(current.filter(|info| is_cache_usable(info, now)))
}

pub fn spawn_maintenance_poll_loop() {
    if POLLER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = shared_info();
    tokio::spawn(async move {
        if let Ok(cached) = GameInfoCacheStore::new().load_maintinfo().await {
            let now = Utc::now().timestamp();
            *state.write().await = cached
                .filter(|info| is_cache_usable(info, now))
                .map(mark_stale);
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
                    let now = Utc::now().timestamp();
                    let mut guard = state.write().await;
                    if let Some(info) = guard.as_mut() {
                        if is_cache_usable(info, now) {
                            info.stale = true;
                        } else {
                            *guard = None;
                        }
                    }
                }
            }
            INITIAL_POLL_COMPLETE.store(true, Ordering::Release);
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

struct RefreshContext<'a> {
    cache_store: &'a GameInfoCacheStore,
    state: &'a RwLock<Option<MaintInfo>>,
    completed_notices: &'a Mutex<HashSet<String>>,
    now: i64,
    wants_translation: bool,
}

async fn refresh_maintenance_info() -> Result<Option<MaintInfo>, Error> {
    let cache_store = GameInfoCacheStore::new();
    let now = Utc::now().timestamp();
    let client = lodestone_client()?;
    let state = shared_info();
    let completed_notices = COMPLETED_NOTICES.get_or_init(|| Mutex::new(HashSet::new()));
    let ctx = RefreshContext {
        cache_store: &cache_store,
        state: &state,
        completed_notices,
        now,
        wants_translation: translate_api_key().is_some(),
    };

    let candidates = fetch_maintenance_candidates(&client, MAINTENANCE_LIST_URL).await?;
    refresh_maintenance_candidates(
        &ctx,
        candidates,
        |url| {
            let client = client.clone();
            async move { fetch_lodestone_html(&client, &url).await }
        },
        |summary| {
            let client = client.clone();
            async move { translate_optional_text(&client, summary.as_deref()).await }
        },
    )
    .await
}

async fn refresh_maintenance_candidates<F, FFut, T, TFut>(
    ctx: &RefreshContext<'_>,
    candidates: Vec<LodestoneLink>,
    mut fetch_detail: F,
    mut translate: T,
) -> Result<Option<MaintInfo>, Error>
where
    F: FnMut(String) -> FFut,
    FFut: std::future::Future<Output = Result<String, Error>>,
    T: FnMut(Option<String>) -> TFut,
    TFut: std::future::Future<Output = Option<String>>,
{
    let (mut cached, mut cancellations) = ctx.cache_store.load_maintenance_state().await?;
    cached = cached.filter(|info| is_cache_usable(info, ctx.now));

    for candidate in candidates.into_iter().take(MAX_MAINTENANCE_CANDIDATES) {
        if cancellations.iter().any(|entry| {
            entry.notice_url == candidate.url
                || entry.original_url.as_deref() == Some(&candidate.url)
        }) {
            continue;
        }
        if !should_recheck_notice_detail(&candidate.url, cached.as_ref(), ctx.now) {
            // Candidates are newest first. Preserve this cached notice rather
            // than replacing it with an older original notice below it.
            break;
        }
        if ctx.completed_notices.lock().await.contains(&candidate.url) {
            continue;
        }
        let detail_html = fetch_detail(candidate.url.clone()).await?;

        // A cancellation notice supersedes any older scheduled notice. Do not
        // fall back to a still-cached window after observing the cancellation.
        if let Some((title, body)) = parse_article_title_and_body(&detail_html)
            && is_japan_maintenance_title(&title)
            && is_cancelled_maintenance_notice(&title, &body)
        {
            let cancellation =
                cancellation_from_notice(&candidate.url, &title, &body, cached.as_ref())?;
            // Persist both the tombstone and removal atomically before marking
            // anything processed. A failed write must retry the cancellation.
            ctx.cache_store
                .save_maintenance_cancellation(&cancellation)
                .await?;
            clear_cancelled_snapshot(ctx.state, &cancellation).await;
            if cached
                .as_ref()
                .is_some_and(|info| cancellation.matches(info))
            {
                cached = None;
            }
            cancellations.push(cancellation);
            continue;
        }

        let Some(mut next) = parse_maintenance_detail(&candidate.url, &detail_html)? else {
            anyhow::bail!("maintenance detail could not be parsed: {}", candidate.url);
        };
        if cancellations.iter().any(|entry| entry.matches(&next)) {
            continue;
        }
        if should_skip_historical_notice(&next, cached.as_ref(), ctx.now) {
            let mut completed = ctx.completed_notices.lock().await;
            if completed.len() >= MAX_COMPLETED_NOTICES {
                completed.clear();
            }
            completed.insert(candidate.url);
            continue;
        }
        if let Some((_, body)) = parse_article_title_and_body(&detail_html) {
            next.body_fingerprint = fingerprint(&body);
        }

        if ctx.wants_translation {
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
                next.translated_description = translate(summary_ja).await;
            }
        }

        let changed = cached.as_ref().is_none_or(|info| {
            info.id != next.id
                || info.start_stamp != next.start_stamp
                || info.end_stamp != next.end_stamp
                || info.end_time_unknown != next.end_time_unknown
                || info.completed != next.completed
                || info.title_kr != next.title_kr
                || info.body_fingerprint != next.body_fingerprint
                || info.scope != next.scope
                || info.translated_description != next.translated_description
        });
        if changed {
            ctx.cache_store.save_maintinfo(&next).await?;
        }
        return Ok(Some(next));
    }

    Ok(cached
        .filter(|info| is_cache_usable(info, ctx.now))
        .map(mark_stale))
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
        && cached.is_none_or(|cached| cached.url != info.url)
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

async fn fetch_maintenance_candidates(
    client: &Client,
    url: &str,
) -> Result<Vec<LodestoneLink>, Error> {
    let response = client.get(url).send().await?;
    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
        // The Lodestone maintenance page returns 503 with its news tabs intact.
        let error = response.error_for_status_ref().expect_err("503 response");
        let candidates =
            parse_maintenance_candidates(&response.text().await?, MAINTENANCE_MODE_NOTICE_LINKS);
        if candidates.is_empty() {
            return Err(error.into());
        }
        return Ok(candidates);
    }
    let html = response.error_for_status()?.text().await?;
    Ok(parse_maintenance_candidates(&html, NOTICE_LINKS))
}

fn parse_maintenance_candidates(html: &str, selector: &str) -> Vec<LodestoneLink> {
    let mut candidates: Vec<_> = parse_detail_links(html, selector)
        .into_iter()
        .filter(|link| is_japan_maintenance_title(&link.title))
        .collect();
    // Special notices can appear before newer items, including their follow-ups.
    // Stable sorting preserves the listing order when timestamps are absent.
    candidates.sort_by_key(|link| Reverse(link.published_at));
    candidates
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
        None if has_unknown_end_time(&body) => {
            let Some(start) = parse_maintenance_start_stamp(&body)? else {
                return Ok(None);
            };
            (start, 0, true)
        }
        None => return Ok(None),
    };
    let completed = is_completed_maintenance_notice(&title, &body);
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
        completed,
        title_kr: display_title,
        url: url.to_string(),
        scope: scope.to_string(),
        body_fingerprint: fingerprint(&body),
        translated_description: None,
        stale: false,
    }))
}

pub(crate) fn has_unknown_end_time(body: &str) -> bool {
    static UNKNOWN_END_REGEX: OnceLock<Regex> = OnceLock::new();
    UNKNOWN_END_REGEX
        .get_or_init(|| Regex::new(r"終了(?:予定)?時[刻間](?:は)?未定").expect("valid regex"))
        .is_match(body)
}

fn is_completed_maintenance_notice(title: &str, body: &str) -> bool {
    static BODY_COMPLETED_REGEX: OnceLock<Regex> = OnceLock::new();

    if title.contains("終了のお知らせ") {
        return true;
    }

    BODY_COMPLETED_REGEX
        .get_or_init(|| {
            Regex::new(
                r"メンテナンス作業(?:を|は[^。]*|について[^。]*|につきまして[^。]*)終了(?:いた)?しました",
            )
            .expect("valid maintenance completion regex")
        })
        .is_match(body)
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
    static FOLLOW_UP_CANCEL_REGEX: OnceLock<Regex> = OnceLock::new();
    static FOLLOW_UP_POSTPONE_REGEX: OnceLock<Regex> = OnceLock::new();

    if title.contains("中止")
        || body.contains("メンテナンス作業を中止")
        || body.contains("メンテナンス作業は中止")
        || body.contains("実施を中止")
    {
        return true;
    }

    let follow_up_cancel = FOLLOW_UP_CANCEL_REGEX.get_or_init(|| {
        Regex::new(
            r"メンテナンス作業(?:について|につきまして)[、,\s]*(?:実施を)?中止(?:とさせていただきます|いたします|します|となりました|になりました)",
        )
        .expect("valid follow-up cancellation regex")
    });
    if follow_up_cancel.is_match(body) {
        return true;
    }

    if !has_labeled_schedule(body) {
        let follow_up_postpone = FOLLOW_UP_POSTPONE_REGEX.get_or_init(|| {
            Regex::new(
                r"メンテナンス作業(?:について|につきまして)[、,\s]*(?:実施を)?延期(?:の連絡を受けました|とさせていただきます|いたします|します|となりました|になりました)",
            )
            .expect("valid follow-up postponement regex")
        });
        return title.contains("延期") || follow_up_postpone.is_match(body);
    }

    false
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

fn cancellation_from_notice(
    url: &str,
    title: &str,
    body: &str,
    cached: Option<&MaintInfo>,
) -> Result<MaintenanceCancellation, Error> {
    let scope = maintenance_scope(title, body).to_string();
    let parsed_start = parse_maintenance_start_stamp(body)?;
    let matched = cached.filter(|info| {
        info.url == url
            || (info.scope == scope
                && match parsed_start {
                    Some(start) => info.start_stamp == start,
                    None => cancellation_mentions_start_date(info, body),
                })
    });
    let start_stamp = parsed_start.or_else(|| matched.map(|info| info.start_stamp));
    let date_pattern = Regex::new(r"(?P<y>\d{4})年(?P<m>\d{1,2})月(?P<d>\d{1,2})日")
        .expect("valid cancellation date regex");
    let start_date = date_pattern.captures(body).and_then(|capture| {
        NaiveDate::from_ymd_opt(
            capture["y"].parse().ok()?,
            capture["m"].parse().ok()?,
            capture["d"].parse().ok()?,
        )
        .map(|date| date.format("%Y-%m-%d").to_string())
    });
    if matched.is_none() && start_stamp.is_none() && start_date.is_none() {
        anyhow::bail!("cannot associate maintenance cancellation with an event: {url}");
    }
    Ok(MaintenanceCancellation {
        notice_url: url.to_string(),
        original_url: matched.map(|info| info.url.clone()),
        scope,
        start_stamp,
        start_date,
    })
}

async fn clear_cancelled_snapshot(
    state: &RwLock<Option<MaintInfo>>,
    cancellation: &MaintenanceCancellation,
) {
    let mut snapshot = state.write().await;
    if snapshot
        .as_ref()
        .is_some_and(|info| cancellation.matches(info))
    {
        *snapshot = None;
    }
}

pub(crate) fn build_maintenance_embed(info: &MaintInfo) -> CreateEmbed {
    let now = Utc::now().timestamp();
    let mut embed = CreateEmbed::new()
        .title(&info.title_kr)
        .url(&info.url)
        .color(SUCCESS_EMBED_COLOR)
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
    if !info.completed && !info.end_time_unknown && info.end_stamp > now {
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

    if info.completed {
        let footer = if info.stale {
            "캐시된 점검 정보 · 점검 종료 공지 확인됨"
        } else {
            "점검 종료 공지 확인됨"
        };
        embed = embed.footer(CreateEmbedFooter::new(footer));
    } else if info.stale {
        let footer = if info.end_time_unknown {
            "캐시된 점검 정보 · 종료 시각 미정 · Lodestone에서 갱신 여부를 확인하지 못함"
        } else if info.end_stamp > now {
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
        .footer(CreateEmbedFooter::new("From Lodestone News"))
}

fn schedule_regexes() -> &'static [Regex; 3] {
    static REGEXES: OnceLock<[Regex; 3]> = OnceLock::new();
    REGEXES.get_or_init(|| {
        const CORE: &str = r"(?P<sy>\d{4})年(?P<sm>\d{1,2})月(?P<sd>\d{1,2})日(?:\([^)]*\)|（[^）]*）)\s*(?P<sh>\d{1,2}):(?P<smin>\d{2})頃?より\s*(?:(?P<ey>\d{4})年)?(?:(?P<em>\d{1,2})月)?(?:(?P<ed>\d{1,2})日(?:\([^)]*\)|（[^）]*）)?)?\s*(?P<eh>\d{1,2}):(?P<emin>\d{2})頃?まで";
        [
            Regex::new(&format!(r"日\s*時[:：]\s*{CORE}")).expect("valid schedule regex"),
            Regex::new(&format!(r"変更後[:：]\s*{CORE}")).expect("valid schedule regex"),
            Regex::new(CORE).expect("valid schedule regex"),
        ]
    })
}

fn start_stamp_regexes() -> &'static [Regex; 3] {
    static REGEXES: OnceLock<[Regex; 3]> = OnceLock::new();
    REGEXES.get_or_init(|| {
        const CORE: &str = r"(?P<y>\d{4})年(?P<m>\d{1,2})月(?P<d>\d{1,2})日(?:\([^)]*\)|（[^）]*）)\s*(?P<h>\d{1,2}):(?P<min>\d{2})頃?より";
        [
            Regex::new(&format!(r"日\s*時[:：]\s*{CORE}")).expect("valid start-time regex"),
            Regex::new(&format!(r"変更後[:：]\s*{CORE}")).expect("valid start-time regex"),
            Regex::new(CORE).expect("valid start-time regex"),
        ]
    })
}

fn capture_prioritized<'a>(
    regexes: &'static [Regex; 3],
    text: &'a str,
) -> Option<regex::Captures<'a>> {
    regexes.iter().find_map(|regex| regex.captures(text))
}

fn has_labeled_schedule(body: &str) -> bool {
    let [by_datetime, by_after, _] = schedule_regexes();
    if by_datetime.is_match(body) || by_after.is_match(body) {
        return true;
    }
    if has_unknown_end_time(body) {
        let [start_by_datetime, start_by_after, _] = start_stamp_regexes();
        return start_by_datetime.is_match(body) || start_by_after.is_match(body);
    }
    false
}

pub(crate) fn parse_maintenance_schedule(body_text: &str) -> Result<Option<(i64, i64)>, Error> {
    let Some(captures) = capture_prioritized(schedule_regexes(), body_text) else {
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
    let Some(c) = capture_prioritized(start_stamp_regexes(), body_text) else {
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
    let is_completed = title.contains("終了のお知らせ");
    let is_follow_up = title.contains("続報")
        || title.contains("時間変更")
        || title.contains("日時変更")
        || title.contains("日程変更")
        || title.contains("延長");
    let is_emergency = title.contains("緊急");

    if is_completed {
        return if is_emergency {
            MaintenanceNoticeKind::EmergencyCompleted
        } else {
            MaintenanceNoticeKind::Completed
        };
    }

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
        MaintenanceNoticeKind::Completed => "전 월드 유지보수 작업 종료",
        MaintenanceNoticeKind::EmergencyCompleted => "전 월드 긴급 유지보수 작업 종료",
    };

    format!("{prefix} ({range})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    async fn fetch_test_listing(status: &str, html: &str) -> Result<Vec<LodestoneLink>, Error> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!(
            "http://{}/lodestone/news/category/2",
            listener.local_addr()?
        );
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
            html.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            for line in BufReader::new(&mut stream).lines() {
                if line.unwrap().is_empty() {
                    break;
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
        });
        let client = Client::builder().no_proxy().build()?;
        let candidates = fetch_maintenance_candidates(&client, &url).await;
        server.join().unwrap();
        candidates
    }

    #[tokio::test]
    async fn selects_newer_follow_up_before_pinned_original_in_maintenance_mode()
    -> Result<(), Error> {
        let html = r#"
            <body class="error">
              <div class="ldst__error"><div id="toptabchanger_newsarea">
                <h2>特別なお知らせ</h2>
                <a class="ic__maintenance--list" href="/lodestone/news/detail/original">
                  全ワールド メンテナンス作業のお知らせ
                  <time><script>ldst_strftime(1793948400, 'YMD');</script></time>
                </a>
                <h2>最新ニュース</h2>
                <a class="ic__maintenance--list" href="/lodestone/news/detail/companion">
                  コンパニオンアプリ メンテナンス作業 終了時間変更のお知らせ
                  <time><script>ldst_strftime(1794301200, 'YMD');</script></time>
                </a>
                <a class="ic__maintenance--list" href="/lodestone/news/detail/follow-up">
                  [続報] 全ワールド メンテナンス作業 終了時間変更のお知らせ
                  <time><script>ldst_strftime(1794300900, 'YMD');</script></time>
                </a>
                <a class="ic__maintenance--list" href="/lodestone/news/detail/original">
                  全ワールド メンテナンス作業のお知らせ
                  <time><script>ldst_strftime(1793948400, 'YMD');</script></time>
                </a>
              </div></div>
            </body>"#;
        let candidates = fetch_test_listing("503 Service Unavailable", html).await?;
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].url.ends_with("/follow-up"));
        assert!(candidates[1].url.ends_with("/original"));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_503_without_the_lodestone_maintenance_listing() {
        let html = r#"<body class="error">
            <a class="ic__maintenance--list" href="/lodestone/news/detail/original">
                全ワールド メンテナンス作業のお知らせ
            </a>
        </body>"#;
        let error = fetch_test_listing("503 Service Unavailable", html)
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<reqwest::Error>().unwrap().status(),
            Some(StatusCode::SERVICE_UNAVAILABLE)
        );
    }

    #[tokio::test]
    async fn cancellation_invalidates_snapshot_before_later_poll_errors() {
        let html = "<article><h1>全ワールド メンテナンス作業のお知らせ</h1><div class='news__detail__wrapper'>2026年10月20日(火) 13:00より19:00頃まで</div></article>";
        let original = parse_maintenance_detail("original", html).unwrap().unwrap();
        let later_event = cancellation_from_notice(
            "cancel-later",
            "全ワールド メンテナンス作業 中止",
            "2026年10月20日(火) 18:00より19:00頃まで 中止",
            Some(&original),
        )
        .unwrap();
        assert!(later_event.original_url.is_none());
        assert!(!later_event.matches(&original));
        let state = RwLock::new(Some(original.clone()));
        clear_cancelled_snapshot(&state, &later_event).await;
        assert!(state.read().await.is_some());
        let entry = cancellation_from_notice(
            "cancel",
            "全ワールド メンテナンス作業 中止",
            "2026年10月20日(火) 13:00より19:00頃まで 中止",
            Some(&original),
        )
        .unwrap();
        clear_cancelled_snapshot(&state, &entry).await;
        // The outer loop's error path only marks an existing value stale.
        // It cannot resurrect the event after a later candidate fetch fails.
        if let Some(info) = state.write().await.as_mut() {
            info.stale = true;
        }
        assert!(state.read().await.is_none());
    }

    #[test]
    fn cancellation_without_cached_original_matches_event_not_other_windows() {
        let entry = cancellation_from_notice(
            "cancel",
            "全ワールド メンテナンス作業 中止のお知らせ",
            "下記の作業を中止します。日時：2026年10月20日(火) 13:00より19:00頃まで",
            None,
        )
        .unwrap();
        let original_html = "<article><h1>全ワールド メンテナンス作業のお知らせ</h1><div class='news__detail__wrapper'>2026年10月20日(火) 13:00より19:00頃まで</div></article>";
        let original = parse_maintenance_detail("original", original_html)
            .unwrap()
            .unwrap();
        assert!(entry.matches(&original));
        let mut different = original.clone();
        different.start_stamp += 3600;
        assert!(!entry.matches(&different));
        different = original.clone();
        different.scope = "일본 DC 한정".into();
        assert!(!entry.matches(&different));
        let mut followup = original;
        followup.url = "followup".into();
        followup.end_stamp -= 300;
        assert!(entry.matches(&followup));
    }

    #[test]
    fn cancellation_with_no_event_evidence_remains_retryable() {
        assert!(
            cancellation_from_notice(
                "cancel",
                "全ワールド メンテナンス作業 中止のお知らせ",
                "メンテナンス作業を中止します。",
                None
            )
            .is_err()
        );
    }

    fn temp_cache_store(label: &str) -> (std::path::PathBuf, GameInfoCacheStore) {
        let dir =
            std::env::temp_dir().join(format!("dynamo-maint-e2e-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = GameInfoCacheStore::with_data_path(dir.join("gameinfo-cache.json"));
        (dir, store)
    }

    fn tokyo_ts(year: i32, month: u32, day: u32, hour: u32, min: u32) -> i64 {
        Tokyo
            .with_ymd_and_hms(year, month, day, hour, min, 0)
            .unwrap()
            .timestamp()
    }

    async fn poll_fixture_listing(
        store: &GameInfoCacheStore,
        state: &RwLock<Option<MaintInfo>>,
        completed_notices: &Mutex<HashSet<String>>,
        now: i64,
        listing_html: &str,
        pages: &std::collections::HashMap<String, String>,
    ) -> Result<Option<MaintInfo>, Error> {
        let candidates = parse_maintenance_candidates(listing_html, NOTICE_LINKS);
        let ctx = RefreshContext {
            cache_store: store,
            state,
            completed_notices,
            now,
            wants_translation: true,
        };
        refresh_maintenance_candidates(
            &ctx,
            candidates,
            |url| {
                let html = pages.get(&url).cloned();
                async move { html.ok_or_else(|| anyhow::anyhow!("missing fixture: {url}")) }
            },
            |summary| async move { summary.map(|text| format!("KO:{text}")) },
        )
        .await
    }

    #[tokio::test]
    async fn original_notice_is_replaced_by_follow_up_end_time_change_and_retranslated()
    -> Result<(), Error> {
        let (dir, store) = temp_cache_store("followup-replace");
        let state = RwLock::new(None);
        let completed_notices = Mutex::new(HashSet::new());

        let orig_path = "/lodestone/news/detail/c37f3c23df3ff03c7852805f20e227090d35b1b9";
        let orig_url = format!("https://jp.finalfantasyxiv.com{orig_path}");
        let followup_path = "/lodestone/news/detail/9720cb22d18c17264e41da5cfe3aa68b1de96640";
        let followup_url = format!("https://jp.finalfantasyxiv.com{followup_path}");

        let mut pages = std::collections::HashMap::new();
        pages.insert(
            orig_url.clone(),
            r#"<article class="news__detail">
                <header class="news__header"><h1><span class="news__header__tag">[メンテナンス]</span>全ワールド メンテナンス作業のお知らせ(11/9-10)</h1></header>
                <div class="news__detail__wrapper">下記日時におきまして、パッチ7.58公開に伴う全ワールドのメンテナンス作業を実施いたします。<br>
                　　　　　　　　　　　　　記<br>
                日　時：2026年11月9日(月) 19:00より<br>
                　　　　2026年11月10日(火) 19:00頃まで<br>
                対　象：ファイナルファンタジーXIVをご利用のお客様</div>
            </article>"#
                .to_string(),
        );
        pages.insert(
            followup_url.clone(),
            r#"<article class="news__detail">
                <header class="news__header"><h1><span class="news__header__tag">[続報]</span>全ワールド メンテナンス作業 終了時間変更のお知らせ(11/9-10)</h1></header>
                <div class="news__detail__wrapper">現在実施しております全ワールドのメンテナンス作業について、終了時刻を2026年11月10日(火) 18:15頃までに変更させていただきます。<br>
                　　　　　　　　　　　　　記<br>
                日　時：2026年11月9日(月) 19:00より<br>
                　　　　2026年11月10日(火) 18:15頃まで<br>
                対　象：ファイナルファンタジーXIVをご利用のお客様</div>
            </article>"#
                .to_string(),
        );

        let now = tokyo_ts(2026, 11, 10, 17, 30);
        let listing_v1 = format!(
            r#"<ul><li class="news__list"><a href="{orig_path}">[メンテナンス]全ワールド メンテナンス作業のお知らせ(11/9-10)<time><script>ldst_strftime(1793948400, 'YMD');</script></time></a></li></ul>"#
        );
        let first =
            poll_fixture_listing(&store, &state, &completed_notices, now, &listing_v1, &pages)
                .await?
                .expect("original notice loaded");
        assert_eq!(first.url, orig_url);
        assert_eq!(first.start_stamp, tokyo_ts(2026, 11, 9, 19, 0));
        assert_eq!(first.end_stamp, tokyo_ts(2026, 11, 10, 19, 0));
        assert_eq!(first.title_kr, "전 월드 유지보수 작업 (11/9-10)");
        assert_eq!(
            first.translated_description.as_deref(),
            Some("KO:パッチ7.58公開に伴う全ワールドのメンテナンス作業を実施いたします。")
        );
        assert_eq!(store.load_maintinfo().await?.unwrap().url, orig_url);

        let listing_v2 = format!(
            r#"<ul>
                <li class="news__list"><a href="{followup_path}">[続報]全ワールド メンテナンス作業 終了時間変更のお知らせ(11/9-10)<time><script>ldst_strftime(1794300900, 'YMD');</script></time></a></li>
                <li class="news__list"><a href="{orig_path}">[メンテナンス]全ワールド メンテナンス作業のお知らせ(11/9-10)<time><script>ldst_strftime(1793948400, 'YMD');</script></time></a></li>
            </ul>"#
        );
        let second =
            poll_fixture_listing(&store, &state, &completed_notices, now, &listing_v2, &pages)
                .await?
                .expect("follow-up notice loaded");
        assert_eq!(second.url, followup_url);
        assert_eq!(second.start_stamp, tokyo_ts(2026, 11, 9, 19, 0));
        assert_eq!(second.end_stamp, tokyo_ts(2026, 11, 10, 18, 15));
        assert_eq!(
            second.title_kr,
            "전 월드 유지보수 작업 종료 시간 변경 (11/9-10)"
        );
        assert_eq!(
            second.translated_description.as_deref(),
            Some(
                "KO:現在実施しております全ワールドのメンテナンス作業について、終了時刻を2026年11月10日(火) 18:15頃までに変更させていただきます。"
            )
        );
        let cached = store.load_maintinfo().await?.unwrap();
        assert_eq!(cached.url, followup_url);
        assert_eq!(cached.end_stamp, tokyo_ts(2026, 11, 10, 18, 15));

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[tokio::test]
    async fn follow_up_extension_replaces_expired_original_and_transitions_through_unknown_end_to_completion()
    -> Result<(), Error> {
        let (dir, store) = temp_cache_store("ext-to-complete");
        let state = RwLock::new(None);
        let completed_notices = Mutex::new(HashSet::new());

        let orig_path = "/lodestone/news/detail/ext-orig";
        let ext_path = "/lodestone/news/detail/ext-unknown";
        let done_path = "/lodestone/news/detail/ext-done";
        let orig_url = format!("https://jp.finalfantasyxiv.com{orig_path}");
        let ext_url = format!("https://jp.finalfantasyxiv.com{ext_path}");
        let done_url = format!("https://jp.finalfantasyxiv.com{done_path}");

        let mut pages = std::collections::HashMap::new();
        pages.insert(
            orig_url.clone(),
            "<article><h1>[メンテナンス]全ワールド メンテナンス作業のお知らせ(10/21)</h1><div class='news__detail__wrapper'>記 日　時：2026年10月21日(水) 15:00より19:00頃まで</div></article>".to_string(),
        );
        pages.insert(
            ext_url.clone(),
            "<article><h1>[続報]全ワールド メンテナンス作業 延長のお知らせ(10/21)</h1><div class='news__detail__wrapper'>現在実施しておりますメンテナンス作業を延長いたします。<br>記<br>日　時：2026年10月21日(水) 15:00より<br>※終了予定時刻は未定です。</div></article>".to_string(),
        );
        pages.insert(
            done_url.clone(),
            "<article><h1>[続報]全ワールド メンテナンス作業 終了のお知らせ(10/21)</h1><div class='news__detail__wrapper'>下記日時にメンテナンス作業を終了いたしました。<br>記<br>日　時：2026年10月21日(水) 15:00頃より20:05頃まで</div></article>".to_string(),
        );

        let listing_1 = format!(
            r#"<ul><li><a href="{orig_path}">全ワールド メンテナンス作業のお知らせ(10/21)<time><script>ldst_strftime(100, 'YMD');</script></time></a></li></ul>"#
        );
        let first = poll_fixture_listing(
            &store,
            &state,
            &completed_notices,
            tokyo_ts(2026, 10, 21, 14, 0),
            &listing_1,
            &pages,
        )
        .await?
        .unwrap();
        assert_eq!(first.url, orig_url);
        assert!(!first.end_time_unknown);
        assert!(!first.completed);

        let listing_2 = format!(
            r#"<ul>
                <li><a href="{ext_path}">[続報]全ワールド メンテナンス作業 延長のお知らせ(10/21)<time><script>ldst_strftime(200, 'YMD');</script></time></a></li>
                <li><a href="{orig_path}">全ワールド メンテナンス作業のお知らせ(10/21)<time><script>ldst_strftime(100, 'YMD');</script></time></a></li>
            </ul>"#
        );
        let second = poll_fixture_listing(
            &store,
            &state,
            &completed_notices,
            tokyo_ts(2026, 10, 21, 19, 5),
            &listing_2,
            &pages,
        )
        .await?
        .unwrap();
        assert_eq!(second.url, ext_url);
        assert!(second.end_time_unknown);
        assert!(!second.completed);

        let listing_3 = format!(
            r#"<ul>
                <li><a href="{done_path}">[続報]全ワールド メンテナンス作業 終了のお知らせ(10/21)<time><script>ldst_strftime(300, 'YMD');</script></time></a></li>
                <li><a href="{ext_path}">[続報]全ワールド メンテナンス作業 延長のお知らせ(10/21)<time><script>ldst_strftime(200, 'YMD');</script></time></a></li>
                <li><a href="{orig_path}">全ワールド メンテナンス作業のお知らせ(10/21)<time><script>ldst_strftime(100, 'YMD');</script></time></a></li>
            </ul>"#
        );
        let third = poll_fixture_listing(
            &store,
            &state,
            &completed_notices,
            tokyo_ts(2026, 10, 21, 20, 10),
            &listing_3,
            &pages,
        )
        .await?
        .unwrap();
        assert_eq!(third.url, done_url);
        assert!(!third.end_time_unknown);
        assert_eq!(third.end_stamp, tokyo_ts(2026, 10, 21, 20, 5));
        assert!(third.completed);
        assert_eq!(third.title_kr, "전 월드 유지보수 작업 종료 (10/21)");

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[tokio::test]
    async fn in_place_update_before_planned_end_refreshes_schedule_and_translation()
    -> Result<(), Error> {
        let (dir, store) = temp_cache_store("in-place-update");
        let state = RwLock::new(None);
        let completed_notices = Mutex::new(HashSet::new());

        let orig_path = "/lodestone/news/detail/in-place-orig";
        let orig_url = format!("https://jp.finalfantasyxiv.com{orig_path}");
        let listing = format!(
            r#"<ul><li><a href="{orig_path}">[メンテナンス]全ワールド メンテナンス作業のお知らせ(10/20)<time><script>ldst_strftime(100, 'YMD');</script></time></a></li></ul>"#
        );

        let mut pages = std::collections::HashMap::new();
        pages.insert(
            orig_url.clone(),
            "<article><h1>[メンテナンス]全ワールド メンテナンス作業のお知らせ(10/20)</h1><div class='news__detail__wrapper'>下記日時におきまして、パッチ7.56作業を実施いたします。 記 日　時：2026年10月20日(火) 15:00より19:00頃まで</div></article>".to_string(),
        );
        let first = poll_fixture_listing(
            &store,
            &state,
            &completed_notices,
            tokyo_ts(2026, 10, 20, 18, 0),
            &listing,
            &pages,
        )
        .await?
        .unwrap();
        assert_eq!(first.end_stamp, tokyo_ts(2026, 10, 20, 19, 0));

        pages.insert(
            orig_url.clone(),
            "<article><h1>[メンテナンス]全ワールド メンテナンス作業のお知らせ(10/20)</h1><div class='news__detail__wrapper'>下記日時におきまして、パッチ7.56および臨時調整を実施いたします。 記 日　時：2026年10月20日(火) 15:00より20:00頃まで</div></article>".to_string(),
        );
        let second = poll_fixture_listing(
            &store,
            &state,
            &completed_notices,
            tokyo_ts(2026, 10, 20, 18, 30),
            &listing,
            &pages,
        )
        .await?
        .unwrap();
        assert_eq!(second.end_stamp, tokyo_ts(2026, 10, 20, 20, 0));
        assert_ne!(second.body_fingerprint, first.body_fingerprint);
        assert_eq!(
            second.translated_description.as_deref(),
            Some("KO:パッチ7.56および臨時調整を実施いたします。")
        );
        let cached = store.load_maintinfo().await?.unwrap();
        assert_eq!(cached.end_stamp, tokyo_ts(2026, 10, 20, 20, 0));
        assert_eq!(cached.body_fingerprint, second.body_fingerprint);

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[tokio::test]
    async fn follow_up_cancellation_and_postponement_clear_original_without_resurrection()
    -> Result<(), Error> {
        // Part A: Follow-up cancellation without "中止" in title.
        let (dir_cancel, store_cancel) = temp_cache_store("followup-cancel");
        let state_cancel = RwLock::new(None);
        let completed_cancel = Mutex::new(HashSet::new());
        let orig_path = "/lodestone/news/detail/cancel-orig";
        let cancel_path = "/lodestone/news/detail/cancel-followup";
        let orig_url = format!("https://jp.finalfantasyxiv.com{orig_path}");
        let cancel_url = format!("https://jp.finalfantasyxiv.com{cancel_path}");
        let mut pages = std::collections::HashMap::new();
        pages.insert(
            orig_url.clone(),
            "<article><h1>[メンテナンス]全ワールド メンテナンス作業のお知らせ(3/13)</h1><div class='news__detail__wrapper'>記 日　時：2026年3月13日(金) 16:00より17:00頃まで</div></article>".to_string(),
        );
        pages.insert(
            cancel_url.clone(),
            "<article><h1>[続報]全ワールド メンテナンス作業のお知らせ(3/13)：続報</h1><div class='news__detail__wrapper'>2026年3月13日(金) 16:00より17:00頃まで実施とお知らせしておりました、全ワールドのメンテナンス作業につきまして、中止とさせていただきます。</div></article>".to_string(),
        );
        let orig_listing = format!(
            r#"<ul><li><a href="{orig_path}">[メンテナンス]全ワールド メンテナンス作業のお知らせ(3/13)<time><script>ldst_strftime(1000, 'YMD');</script></time></a></li></ul>"#
        );
        let orig = poll_fixture_listing(
            &store_cancel,
            &state_cancel,
            &completed_cancel,
            tokyo_ts(2026, 3, 13, 15, 0),
            &orig_listing,
            &pages,
        )
        .await?
        .unwrap();
        *state_cancel.write().await = Some(orig);

        let cancel_listing = format!(
            r#"<ul>
                <li><a href="{cancel_path}">[続報]全ワールド メンテナンス作業のお知らせ(3/13)：続報<time><script>ldst_strftime(2000, 'YMD');</script></time></a></li>
                <li><a href="{orig_path}">[メンテナンス]全ワールド メンテナンス作業のお知らせ(3/13)<time><script>ldst_strftime(1000, 'YMD');</script></time></a></li>
            </ul>"#
        );
        let after_cancel = poll_fixture_listing(
            &store_cancel,
            &state_cancel,
            &completed_cancel,
            tokyo_ts(2026, 3, 13, 15, 30),
            &cancel_listing,
            &pages,
        )
        .await?;
        assert!(after_cancel.is_none());
        assert!(state_cancel.read().await.is_none());
        assert!(store_cancel.load_maintinfo().await?.is_none());
        let _ = std::fs::remove_dir_all(&dir_cancel);

        // Part B: Follow-up postponement without a rescheduled date.
        let (dir_postpone, store_postpone) = temp_cache_store("followup-postpone");
        let state_postpone = RwLock::new(None);
        let completed_postpone = Mutex::new(HashSet::new());
        let p_orig_path = "/lodestone/news/detail/postpone-orig";
        let p_follow_path = "/lodestone/news/detail/postpone-followup";
        let p_orig_url = format!("https://jp.finalfantasyxiv.com{p_orig_path}");
        let p_follow_url = format!("https://jp.finalfantasyxiv.com{p_follow_path}");
        pages.insert(
            p_orig_url.clone(),
            "<article><h1>[メンテナンス]全ワールド メンテナンス作業のお知らせ(10/26-27)</h1><div class='news__detail__wrapper'>記 日　時：2026年10月26日(月) 22:30より2026年10月27日(火) 5:00まで</div></article>".to_string(),
        );
        pages.insert(
            p_follow_url.clone(),
            "<article><h1>[続報]全ワールド メンテナンス作業のお知らせ(10/26-27)：続報</h1><div class='news__detail__wrapper'>2026年10月26日(月) 22:30より2026年10月27日(火) 5:00まで実施とお知らせしておりました、全ワールドのメンテナンス作業につきまして、延期の連絡を受けましたのでお知らせいたします。 新たな作業日程につきましては、あらためてお知らせいたします。</div></article>".to_string(),
        );
        let p_orig_listing = format!(
            r#"<ul><li><a href="{p_orig_path}">[メンテナンス]全ワールド メンテナンス作業のお知らせ(10/26-27)<time><script>ldst_strftime(1000, 'YMD');</script></time></a></li></ul>"#
        );
        let p_orig = poll_fixture_listing(
            &store_postpone,
            &state_postpone,
            &completed_postpone,
            tokyo_ts(2026, 10, 26, 20, 0),
            &p_orig_listing,
            &pages,
        )
        .await?
        .unwrap();
        *state_postpone.write().await = Some(p_orig);

        let p_follow_listing = format!(
            r#"<ul>
                <li><a href="{p_follow_path}">[続報]全ワールド メンテナンス作業のお知らせ(10/26-27)：続報<time><script>ldst_strftime(2000, 'YMD');</script></time></a></li>
                <li><a href="{p_orig_path}">[メンテナンス]全ワールド メンテナンス作業のお知らせ(10/26-27)<time><script>ldst_strftime(1000, 'YMD');</script></time></a></li>
            </ul>"#
        );
        let after_postpone = poll_fixture_listing(
            &store_postpone,
            &state_postpone,
            &completed_postpone,
            tokyo_ts(2026, 10, 26, 21, 0),
            &p_follow_listing,
            &pages,
        )
        .await?;
        assert!(after_postpone.is_none());
        assert!(state_postpone.read().await.is_none());
        assert!(store_postpone.load_maintinfo().await?.is_none());
        let _ = std::fs::remove_dir_all(&dir_postpone);

        Ok(())
    }

    #[test]
    fn in_place_completion_footnote_sets_completed_without_false_positive_on_campaign_end() {
        let completed_html = r#"<article>
            <h1>[メンテナンス]日本データセンター クロスワールド機能 緊急メンテナンス作業のお知らせ(10/15)</h1>
            <div class="news__detail__wrapper">
                記 日　時：2026年10月15日(木) 11:00より11:15頃まで<br>
                ※2026年10月15日(木) 11:11追記 本メンテナンス作業は2026年10月15日(木) 11:02頃に終了いたしました。
            </div>
        </article>"#;
        let completed_info = parse_maintenance_detail("in-place-done", completed_html)
            .unwrap()
            .unwrap();
        assert!(completed_info.completed);

        let unrelated_html = r#"<article>
            <h1>[メンテナンス]全ワールド メンテナンス作業のお知らせ(11/9-10)</h1>
            <div class="news__detail__wrapper">
                ※期間限定キャンペーンの応募受付は終了いたしました。<br>
                記 日　時：2026年11月9日(月) 19:00より 2026年11月10日(火) 19:00頃まで
            </div>
        </article>"#;
        let upcoming_info = parse_maintenance_detail("unrelated-footnote", unrelated_html)
            .unwrap()
            .unwrap();
        assert!(!upcoming_info.completed);
    }
}
