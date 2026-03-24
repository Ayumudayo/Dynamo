use dynamo_runtime_api::Error;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT_LANGUAGE, USER_AGENT},
    Client,
};
use scraper::{ElementRef, Html, Selector};
use std::time::Duration;

const LODESTONE_BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub(crate) struct LodestoneLink {
    pub(crate) title: String,
    pub(crate) url: String,
}

pub(crate) fn lodestone_client() -> Result<Client, Error> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(LODESTONE_BROWSER_USER_AGENT));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("ja,en;q=0.9"));

    Ok(Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()?)
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
        .filter(|text| !text.is_empty())?;
    let body = document
        .select(&body_selector)
        .next()
        .map(extract_text)
        .filter(|text| !text.is_empty())?;

    Some((title, body))
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
