use crate::constants::QUOTE_PAGE_URL;
use dynamo_service_stock::Error;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, LOCATION, ORIGIN, REFERER};
use url::Url;

pub(crate) fn header_location(
    headers: &HeaderMap,
    base_url: &str,
) -> Result<Option<String>, Error> {
    let Some(location) = headers.get(LOCATION) else {
        return Ok(None);
    };

    let value = location.to_str()?;
    let absolute = Url::parse(base_url)?.join(value)?;
    Ok(Some(absolute.to_string()))
}

pub(crate) fn html_headers(current_url: Option<&str>, referer: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml"),
    );
    if let Some(referer) = referer.and_then(|value| HeaderValue::from_str(value).ok()) {
        headers.insert(REFERER, referer);
    }
    if let Some(origin) = current_url.and_then(origin_header) {
        headers.insert(ORIGIN, origin);
    }
    headers
}

pub(crate) fn crumb_headers(referer: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    let referer =
        HeaderValue::from_str(referer).unwrap_or_else(|_| HeaderValue::from_static(QUOTE_PAGE_URL));
    headers.insert(REFERER, referer);
    headers.insert(
        ORIGIN,
        HeaderValue::from_static("https://finance.yahoo.com"),
    );
    headers
}

pub(crate) fn json_headers(symbol: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        ORIGIN,
        HeaderValue::from_static("https://finance.yahoo.com"),
    );
    let referer = format!("https://finance.yahoo.com/quote/{symbol}");
    if let Ok(referer) = HeaderValue::from_str(&referer) {
        headers.insert(REFERER, referer);
    }
    headers
}

pub(crate) fn form_headers(current_url: &str, referer: Option<&str>) -> HeaderMap {
    let mut headers = html_headers(Some(current_url), referer);
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    headers
}

fn origin_header(url: &str) -> Option<HeaderValue> {
    let parsed = Url::parse(url).ok()?;
    let origin = format!("{}://{}", parsed.scheme(), parsed.host_str()?);
    HeaderValue::from_str(&origin).ok()
}
