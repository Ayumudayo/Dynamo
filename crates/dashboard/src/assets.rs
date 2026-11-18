//! Content-addressed browser assets (stylesheet and script) and the security
//! headers that every dashboard response carries.
//!
//! `build.rs` bundles `assets/dashboard.css` (with the locked font faces) and
//! `assets/dashboard.js` (with the mutation transport) and names each file
//! after its SHA-256, so responses can be cached as immutable. The only inline
//! script is the one-line boot script, which the CSP allows by hash.

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
    routing::get,
};

use crate::font_assets::FONT_CACHE_CONTROL;

#[allow(dead_code)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/browser_assets.rs"));
}

pub(crate) use generated::*;

pub(crate) fn browser_asset_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(DASHBOARD_CSS_PATH, get(dashboard_css))
        .route(DASHBOARD_JS_PATH, get(dashboard_js))
}

/// Returns true for the exact content-addressed asset routes.
#[cfg_attr(not(feature = "perf-harness"), allow(dead_code))]
pub(crate) fn is_browser_asset_path(path: &str) -> bool {
    path == DASHBOARD_CSS_PATH || path == DASHBOARD_JS_PATH
}

async fn dashboard_css(headers: HeaderMap) -> Response {
    asset_response(
        &headers,
        DASHBOARD_CSS,
        DASHBOARD_CSS_ETAG,
        "text/css; charset=utf-8",
    )
}

async fn dashboard_js(headers: HeaderMap) -> Response {
    asset_response(
        &headers,
        DASHBOARD_JS,
        DASHBOARD_JS_ETAG,
        "text/javascript; charset=utf-8",
    )
}

fn asset_response(
    request_headers: &HeaderMap,
    body: &'static str,
    etag: &'static str,
    content_type: &'static str,
) -> Response {
    let not_modified = request_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
            })
        });
    let mut response = if not_modified {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        response
    } else {
        Response::new(Body::from(body))
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(FONT_CACHE_CONTROL),
    );
    headers.insert(header::ETAG, HeaderValue::from_static(etag));
    response
}

/// The Content-Security-Policy for dashboard pages. Scripts and styles are
/// same-origin only (plus the hashed boot script); inline event handlers and
/// inline styles are therefore impossible by construction.
pub(crate) fn content_security_policy() -> String {
    format!(
        "default-src 'none'; script-src 'self' {BOOT_SCRIPT_CSP_SOURCE}; style-src 'self'; \
         img-src 'self' data: https://cdn.discordapp.com; font-src 'self'; connect-src 'self'; \
         form-action 'self'; frame-ancestors 'none'; base-uri 'none'"
    )
}

pub(crate) async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let is_html = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if is_html {
        if let Ok(value) = HeaderValue::from_str(&content_security_policy()) {
            headers.insert(header::CONTENT_SECURITY_POLICY, value);
        }
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    response
}
