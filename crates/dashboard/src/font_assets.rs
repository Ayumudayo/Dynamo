use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
    routing::get,
};

#[allow(dead_code)]
mod locked {
    include!(concat!(env!("OUT_DIR"), "/font_assets.rs"));
}

pub(crate) use locked::*;

pub(crate) const FONT_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

pub(crate) fn font_asset_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(FIRA_SANS_LIGHT_PATH, get(fira_sans_light_font))
        .route(FIRA_SANS_REGULAR_PATH, get(fira_sans_regular_font))
        .route(FIRA_SANS_MEDIUM_PATH, get(fira_sans_medium_font))
        .route(FIRA_SANS_SEMIBOLD_PATH, get(fira_sans_semibold_font))
        .route(FIRA_SANS_BOLD_PATH, get(fira_sans_bold_font))
        .route(FIRA_CODE_VARIABLE_PATH, get(fira_code_variable_font))
}

fn if_none_match_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
            })
        })
}

fn font_asset_response(
    request_headers: &HeaderMap,
    bytes: &'static [u8],
    etag: &'static str,
) -> Response {
    let not_modified = if_none_match_matches(request_headers, etag);
    let mut response = if not_modified {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response
    } else {
        Response::new(Body::from(bytes))
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("font/woff2"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(FONT_CACHE_CONTROL),
    );
    headers.insert(header::ETAG, HeaderValue::from_static(etag));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn fira_sans_light_font(headers: HeaderMap) -> Response {
    font_asset_response(&headers, FIRA_SANS_LIGHT_BYTES, FIRA_SANS_LIGHT_ETAG)
}

async fn fira_sans_regular_font(headers: HeaderMap) -> Response {
    font_asset_response(&headers, FIRA_SANS_REGULAR_BYTES, FIRA_SANS_REGULAR_ETAG)
}

async fn fira_sans_medium_font(headers: HeaderMap) -> Response {
    font_asset_response(&headers, FIRA_SANS_MEDIUM_BYTES, FIRA_SANS_MEDIUM_ETAG)
}

async fn fira_sans_semibold_font(headers: HeaderMap) -> Response {
    font_asset_response(&headers, FIRA_SANS_SEMIBOLD_BYTES, FIRA_SANS_SEMIBOLD_ETAG)
}

async fn fira_sans_bold_font(headers: HeaderMap) -> Response {
    font_asset_response(&headers, FIRA_SANS_BOLD_BYTES, FIRA_SANS_BOLD_ETAG)
}

async fn fira_code_variable_font(headers: HeaderMap) -> Response {
    font_asset_response(&headers, FIRA_CODE_VARIABLE_BYTES, FIRA_CODE_VARIABLE_ETAG)
}
