use std::collections::BTreeMap;

use dynamo_service_stock::Error;
use reqwest::header::{HeaderMap, SET_COOKIE};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default)]
pub(crate) struct YahooSession {
    pub(crate) crumb: Option<String>,
    pub(crate) cookies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PersistedYahooSession {
    pub(crate) crumb: Option<String>,
    #[serde(default)]
    pub(crate) cookies: BTreeMap<String, String>,
}

pub(crate) fn build_cookie_header(cookies: &BTreeMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub(crate) fn capture_set_cookies(
    headers: &HeaderMap,
    session: &mut YahooSession,
) -> Result<(), Error> {
    for value in headers.get_all(SET_COOKIE) {
        let raw = value.to_str()?;
        if let Some((name, cookie_value)) = parse_set_cookie(raw) {
            session.cookies.insert(name, cookie_value);
        }
    }

    Ok(())
}

fn parse_set_cookie(header: &str) -> Option<(String, String)> {
    let pair = header.split(';').next()?.trim();
    let (name, value) = pair.split_once('=')?;
    if name.is_empty() {
        return None;
    }

    Some((name.to_string(), value.to_string()))
}
