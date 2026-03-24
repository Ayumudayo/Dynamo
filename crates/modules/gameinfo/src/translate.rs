use crate::lodestone::decode_html_entities;
use dynamo_runtime_api::Error;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const GOOGLE_TRANSLATE_API_URL: &str = "https://translation.googleapis.com/language/translate/v2";

#[derive(Debug, Serialize)]
struct TranslateRequest {
    q: Vec<String>,
    source: &'static str,
    target: &'static str,
    format: &'static str,
}

#[derive(Debug, Deserialize)]
struct TranslateResponse {
    data: TranslateResponseData,
}

#[derive(Debug, Deserialize)]
struct TranslateResponseData {
    translations: Vec<TranslatedItem>,
}

#[derive(Debug, Deserialize)]
struct TranslatedItem {
    #[serde(rename = "translatedText")]
    translated_text: String,
}

pub(crate) fn translate_api_key() -> Option<String> {
    std::env::var("GOOGLE_TRANSLATE_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) async fn translate_optional_text(client: &Client, text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    if text.is_empty() {
        return None;
    }

    translate_texts(client, &[text.to_string()])
        .await
        .ok()
        .and_then(|mut values| values.pop())
}

pub(crate) async fn translate_text_list(client: &Client, texts: &[String]) -> Vec<String> {
    let cleaned = texts
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if cleaned.is_empty() {
        return Vec::new();
    }

    translate_texts(client, &cleaned).await.unwrap_or_default()
}

async fn translate_texts(client: &Client, texts: &[String]) -> Result<Vec<String>, Error> {
    let Some(key) = translate_api_key() else {
        return Err(anyhow::anyhow!("google translate api key is not configured").into());
    };

    let request = TranslateRequest {
        q: texts.to_vec(),
        source: "ja",
        target: "ko",
        format: "text",
    };
    let response = client
        .post(GOOGLE_TRANSLATE_API_URL)
        .query(&[("key", key.as_str())])
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json::<TranslateResponse>()
        .await?;

    Ok(response
        .data
        .translations
        .into_iter()
        .map(|item| decode_html_entities(&item.translated_text))
        .collect())
}
