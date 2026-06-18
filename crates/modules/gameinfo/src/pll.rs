use crate::{
    cache::GameInfoCacheStore,
    constants::{ERROR_EMBED_COLOR, GAMEINFO_THUMBNAIL_URL, SUCCESS_EMBED_COLOR},
    lodestone::{
        absolutize_lodestone_url, extract_first_heading, extract_text, fetch_lodestone_html,
        lodestone_client, parse_article_title_and_body, parse_detail_links,
    },
    referral::to_valid_url,
    translate::{translate_api_key, translate_optional_text, translate_text_list},
};
use chrono::{Datelike, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Asia::{Seoul, Tokyo};
use dynamo_runtime_api::Error;
use poise::serenity_prelude::{CreateActionRow, CreateButton, CreateEmbed, Timestamp};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};

const TOPICS_LIST_URL: &str = "https://jp.finalfantasyxiv.com/lodestone/topics/";
const PLL_CACHE_DURATION_SECONDS: i64 = 12 * 60 * 60;
const MAX_PLL_PAGES: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PllInfo {
    #[serde(alias = "fixedTitle")]
    pub(crate) fixed_title: String,
    pub(crate) url: String,
    pub(crate) start_stamp: Option<i64>,
    #[serde(alias = "expireTime")]
    pub(crate) expire_time: i64,
    pub(crate) translated_description: Option<String>,
    pub(crate) translated_contents: Vec<String>,
    pub(crate) stream_links: Vec<StreamLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StreamLink {
    pub(crate) label: String,
    pub(crate) url: String,
}

#[derive(Debug, Default)]
struct PllSections {
    summary_ja: Option<String>,
    content_items_ja: Vec<String>,
    stream_links: Vec<StreamLink>,
}

pub(crate) async fn fetch_pll_info() -> Result<Option<PllInfo>, Error> {
    let cache_store = GameInfoCacheStore::new();
    let now = Utc::now().timestamp();
    let cached = cache_store.load_pllinfo().await?;
    let client = lodestone_client()?;
    let wants_translation = translate_api_key().is_some();
    let mut debug_past_candidate = None;

    for page in 1..=MAX_PLL_PAGES {
        let list_url = format!("{TOPICS_LIST_URL}?page={page}");
        let list_html = fetch_lodestone_html(&client, &list_url).await;
        let Ok(list_html) = list_html else {
            break;
        };

        let candidates = parse_pll_candidates(&list_html);
        for candidate in candidates {
            if let Some(info) = cached.as_ref() {
                let translation_ready = !wants_translation
                    || info.translated_description.is_some()
                    || !info.translated_contents.is_empty();
                let schedule_ready = is_valid_pll_schedule(info);
                if info.url == candidate.url
                    && pll_is_usable(info.start_stamp, now)
                    && (info.expire_time > now || allow_past_pll_debug())
                    && translation_ready
                    && schedule_ready
                {
                    return Ok(Some(info.clone()));
                }
            }

            let detail_html = fetch_lodestone_html(&client, &candidate.url).await;
            let Ok(detail_html) = detail_html else {
                continue;
            };

            let Some(mut next) = parse_pll_detail(&candidate.url, &detail_html)? else {
                continue;
            };

            let sections = parse_pll_sections(&detail_html);
            next.stream_links = sections.stream_links;

            if wants_translation {
                next.translated_description =
                    translate_optional_text(&client, sections.summary_ja.as_deref()).await;
                next.translated_contents =
                    translate_text_list(&client, &sections.content_items_ja).await;
            }

            if !is_valid_pll_schedule(&next) {
                continue;
            }

            if !pll_is_live_or_unknown(next.start_stamp, now) {
                if allow_past_pll_debug() && debug_past_candidate.is_none() {
                    debug_past_candidate = Some(next);
                }
                continue;
            }

            next.expire_time = now + PLL_CACHE_DURATION_SECONDS;
            cache_store.save_pllinfo(&next).await?;
            return Ok(Some(next));
        }
    }

    if let Some(mut next) = debug_past_candidate {
        next.expire_time = now + PLL_CACHE_DURATION_SECONDS;
        cache_store.save_pllinfo(&next).await?;
        return Ok(Some(next));
    }

    load_pll_fallback(&cache_store, now).await
}

async fn load_pll_fallback(
    cache_store: &GameInfoCacheStore,
    now: i64,
) -> Result<Option<PllInfo>, Error> {
    let Some(cached) = cache_store.load_pllinfo().await? else {
        return Ok(None);
    };

    if !is_valid_pll_schedule(&cached) {
        return Ok(None);
    }

    if cached.expire_time > now {
        return Ok(Some(cached));
    }

    if cached.start_stamp.is_some_and(|stamp| stamp > now) {
        return Ok(Some(cached));
    }

    if allow_past_pll_debug() {
        return Ok(Some(cached));
    }

    Ok(None)
}

fn parse_pll_candidates(html: &str) -> Vec<crate::lodestone::LodestoneLink> {
    parse_detail_links(html, r#"a[href*="/lodestone/topics/detail/"]"#)
        .into_iter()
        .filter(|link| is_pll_title(&link.title))
        .collect()
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
        translated_description: None,
        translated_contents: Vec::new(),
        stream_links: Vec::new(),
    }))
}

pub(crate) fn build_pll_embed(info: &PllInfo) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title(&info.fixed_title)
        .url(&info.url)
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
        );

    if let Some(description) = info.translated_description.as_deref()
        && !description.trim().is_empty()
    {
        embed = embed.description(description);
    }

    if !info.translated_contents.is_empty() {
        let value = info
            .translated_contents
            .iter()
            .map(|item| format!("• {item}"))
            .collect::<Vec<_>>()
            .join("\n");
        embed = embed.field("방송 내용", value, false);
    }

    embed
}

pub(crate) fn build_pll_stream_buttons(info: &PllInfo) -> Vec<CreateActionRow> {
    let buttons = info
        .stream_links
        .iter()
        .filter_map(|link| {
            to_valid_url(&link.url).map(|url| CreateButton::new_link(url).label(&link.label))
        })
        .collect::<Vec<_>>();

    if buttons.is_empty() {
        Vec::new()
    } else {
        vec![CreateActionRow::Buttons(buttons)]
    }
}

pub(crate) fn create_pll_error_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("No PLL Info")
        .description("PLL 관련 정보를 찾을 수 없습니다.")
        .url("https://jp.finalfantasyxiv.com/lodestone")
        .color(ERROR_EMBED_COLOR)
        .thumbnail(GAMEINFO_THUMBNAIL_URL)
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

fn parse_pll_sections(html: &str) -> PllSections {
    let document = Html::parse_document(html);
    let wrapper_selector =
        Selector::parse("article .news__detail__wrapper").expect("valid selector");
    let link_selector = Selector::parse("a").expect("valid selector");
    let list_item_selector = Selector::parse("li").expect("valid selector");

    let Some(wrapper) = document.select(&wrapper_selector).next() else {
        return PllSections::default();
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        None,
        Viewing,
        Contents,
    }

    let mut before_sections = true;
    let mut current_section = Section::None;
    let mut summary_parts = Vec::new();
    let mut content_items_ja = Vec::new();
    let mut stream_links = Vec::new();

    for child in wrapper.children() {
        let Some(element) = ElementRef::wrap(child) else {
            continue;
        };

        match element.value().name() {
            "h3" => {
                before_sections = false;
                current_section = Section::None;
            }
            "h4" => {
                before_sections = false;
                current_section = match extract_text(element).as_str() {
                    "視聴方法" => Section::Viewing,
                    "放送内容" => Section::Contents,
                    _ => Section::None,
                };
            }
            "p" => {
                if before_sections {
                    let text = extract_text(element);
                    if !text.is_empty() {
                        summary_parts.push(text);
                    }
                }
            }
            "ul" | "ol" => match current_section {
                Section::Viewing => {
                    for anchor in element.select(&link_selector) {
                        let label = extract_text(anchor);
                        let Some(href) = anchor.value().attr("href") else {
                            continue;
                        };
                        let Some(url) = absolutize_lodestone_url(href).or_else(|| {
                            if href.starts_with("http") {
                                Some(href.to_string())
                            } else {
                                None
                            }
                        }) else {
                            continue;
                        };

                        if !label.is_empty() {
                            stream_links.push(StreamLink { label, url });
                        }
                    }
                }
                Section::Contents => {
                    for item in element.select(&list_item_selector) {
                        let text = extract_text(item);
                        if !text.is_empty() {
                            content_items_ja.push(text);
                        }
                    }
                }
                Section::None => {}
            },
            _ => {}
        }
    }

    PllSections {
        summary_ja: if summary_parts.is_empty() {
            None
        } else {
            Some(summary_parts.join("\n"))
        },
        content_items_ja,
        stream_links,
    }
}

pub(crate) fn allow_past_pll_debug() -> bool {
    cfg!(debug_assertions)
}

pub(crate) fn is_valid_pll_schedule(info: &PllInfo) -> bool {
    info.start_stamp.is_some() && !info.stream_links.is_empty()
}

fn pll_is_live_or_unknown(start_stamp: Option<i64>, now: i64) -> bool {
    start_stamp.is_none_or(|stamp| stamp >= now)
}

fn pll_is_usable(start_stamp: Option<i64>, now: i64) -> bool {
    pll_is_live_or_unknown(start_stamp, now) || allow_past_pll_debug()
}

pub(crate) fn extract_pll_start(body_text: &str) -> Result<Option<i64>, Error> {
    let regex = Regex::new(r"(\d{4}年\d{1,2}月\d{1,2}日（[^）]+）)\s?(\d{1,2}:\d{2})頃?～")
        .expect("valid regex");

    let Some(captures) = regex.captures(body_text) else {
        return Ok(None);
    };

    let date = captures
        .get(1)
        .map(|value| value.as_str().replace(['（', '）'], " "))
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

pub(crate) fn generate_pll_title(round_number: Option<&str>, start_stamp: Option<i64>) -> String {
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
