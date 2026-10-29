use dynamo_runtime_api::Error;
use reqwest::{
    Client,
    header::{ACCEPT_LANGUAGE, HeaderMap, HeaderValue, USER_AGENT},
};
use scraper::{ElementRef, Html, Selector};
use std::sync::OnceLock;
use std::time::Duration;

const LODESTONE_BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub(crate) struct LodestoneLink {
    pub(crate) title: String,
    pub(crate) url: String,
}

pub(crate) fn lodestone_client() -> Result<Client, Error> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(LODESTONE_BROWSER_USER_AGENT),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("ja,en;q=0.9"));

    let client = Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()?;
    let _ = CLIENT.set(client.clone());
    Ok(CLIENT.get().cloned().unwrap_or(client))
}

pub(crate) async fn fetch_lodestone_html(client: &Client, url: &str) -> Result<String, Error> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

pub(crate) fn parse_detail_links(html: &str, selector: &str) -> Vec<LodestoneLink> {
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
        let title = if title.is_empty() {
            let parent_title = node
                .parent()
                .and_then(ElementRef::wrap)
                .map(extract_text)
                .unwrap_or_default();
            if parent_title.is_empty() {
                node.value()
                    .attr("aria-label")
                    .or_else(|| node.value().attr("title"))
                    .map(normalize_text)
                    .unwrap_or_default()
            } else {
                parent_title
            }
        } else {
            title
        };
        if title.is_empty() {
            continue;
        }

        links.push(LodestoneLink { title, url });
    }

    links
}

pub(crate) fn parse_article_title_and_body(html: &str) -> Option<(String, String)> {
    let document = Html::parse_document(html);
    let title_selector = Selector::parse("article h1").expect("valid selector");
    let body_selector = Selector::parse("article .news__detail__wrapper").expect("valid selector");

    let title = document
        .select(&title_selector)
        .next()
        .map(extract_text)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            let selector = Selector::parse(r#"meta[property="og:title"]"#).ok()?;
            document
                .select(&selector)
                .next()?
                .value()
                .attr("content")
                .map(normalize_text)
                .filter(|text| !text.is_empty())
        })
        .or_else(|| {
            let selector = Selector::parse("h1, h2").ok()?;
            document
                .select(&selector)
                .map(extract_text)
                .find(|text| text.contains("メンテナンス作業"))
        })?;
    let body = document
        .select(&body_selector)
        .next()
        .map(extract_text)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            // Lodestone has changed detail-page wrappers before. If the named
            // wrapper disappears, find the smallest semantic content block
            // that still contains a maintenance time range.
            let selector = Selector::parse("article, main, section, div").ok()?;
            document
                .select(&selector)
                .map(extract_text)
                .filter(|text| contains_schedule_shape(text))
                .min_by_key(String::len)
        })
        .or_else(|| {
            let selector = Selector::parse("body").ok()?;
            document
                .select(&selector)
                .next()
                .map(extract_text)
                .filter(|text| contains_schedule_shape(text))
        })?;

    Some((title, body))
}

fn contains_schedule_shape(text: &str) -> bool {
    text.contains('年')
        && text.contains('月')
        && text.contains("より")
        && (text.contains("まで") || text.contains("終了時刻未定") || text.contains("終了時間未定"))
}

pub(crate) fn extract_first_heading(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    let h3_selector = Selector::parse("article h3").expect("valid selector");
    document
        .select(&h3_selector)
        .next()
        .map(extract_text)
        .filter(|text| !text.is_empty())
}

pub(crate) fn extract_text(node: ElementRef<'_>) -> String {
    normalize_text(&node.text().collect::<Vec<_>>().join(" "))
}

pub(crate) fn normalize_text(input: &str) -> String {
    input
        .replace('\u{3000}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn decode_html_entities(input: &str) -> String {
    input
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

pub(crate) fn absolutize_lodestone_url(href: &str) -> Option<String> {
    if href.starts_with("https://jp.finalfantasyxiv.com/") {
        return Some(href.to_string());
    }

    if href.starts_with("/lodestone/") {
        return Some(format!("https://jp.finalfantasyxiv.com{href}"));
    }

    None
}
