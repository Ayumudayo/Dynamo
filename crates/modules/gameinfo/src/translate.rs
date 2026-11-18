use crate::lodestone::decode_html_entities;
use dynamo_runtime_api::Error;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::warn;

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

    match translate_texts(client, &[text.to_string()]).await {
        Ok(mut values) => values.pop(),
        Err(error) => {
            if translate_api_key().is_some() {
                warn!(?error, "failed to translate text with Google Translate API");
            }
            None
        }
    }
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

    match translate_texts(client, &cleaned).await {
        Ok(values) => values,
        Err(error) => {
            if translate_api_key().is_some() {
                warn!(?error, "failed to translate list with Google Translate API");
            }
            Vec::new()
        }
    }
}

async fn translate_texts(client: &Client, texts: &[String]) -> Result<Vec<String>, Error> {
    let Some(key) = translate_api_key() else {
        return Err(anyhow::anyhow!(
            "google translate api key is not configured"
        ));
    };

    let request = TranslateRequest {
        q: texts.to_vec(),
        source: "ja",
        target: "ko",
        format: "text",
    };
    let response = client
        .post(GOOGLE_TRANSLATE_API_URL)
        .header("X-Goog-Api-Key", key)
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
