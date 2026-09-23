use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::{PERF_CONTROL_HEADER, config::FixtureMode, runtime::PerfRuntime};
use crate::{
    FIRA_CODE_VARIABLE_PATH, FIRA_SANS_BOLD_PATH, FIRA_SANS_LIGHT_PATH, FIRA_SANS_MEDIUM_PATH,
    FIRA_SANS_REGULAR_PATH, FIRA_SANS_SEMIBOLD_PATH,
};

pub(super) async fn enforce_read_only_harness(
    State(runtime): State<Arc<PerfRuntime>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method();
    let allowed_read = (method == Method::GET || method == Method::HEAD)
        && is_allowed_read_request(&runtime, request.uri());
    let path = request.uri().path();
    let authenticated_control = method == Method::POST
        && request.uri().query().is_none()
        && matches!(
            path,
            "/__perf/browser-outbound-attempt" | "/__perf/shutdown"
        )
        && request
            .headers()
            .get(PERF_CONTROL_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == runtime.cookie_value);
    if allowed_read || authenticated_control {
        return next.run(request).await;
    }
    runtime.increment_denied_requests();
    if method != Method::GET && method != Method::HEAD {
        runtime.increment_server_write_attempts();
    }
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": "performance harness route denied" })),
    )
        .into_response()
}

pub(super) fn is_allowed_read_request(runtime: &PerfRuntime, uri: &axum::http::Uri) -> bool {
    if runtime.fixture_mode != FixtureMode::Public && uri.path() == runtime.guild_path() {
        return uri.query().is_none_or(is_allowed_guild_query);
    }
    uri.query().is_none() && is_allowed_read_path(runtime, uri.path())
}

pub(super) fn is_allowed_guild_query(query: &str) -> bool {
    let mut fields = query.split('&');
    let Some(tab_field) = fields.next() else {
        return false;
    };
    let Some(tab) = tab_field.strip_prefix("tab=") else {
        return false;
    };
    if matches!(tab, "overview" | "modules" | "commands") {
        return fields.next().is_none();
    }
    if tab != "logs" {
        return false;
    }
    let mut last_rank = 0;
    for field in fields {
        let Some((key, value)) = field.split_once('=') else {
            return false;
        };
        let rank = match key {
            "log_entity" if matches!(value, "module" | "command") => 1,
            "log_action" if matches!(value, "toggle" | "save_settings") => 2,
            "log_page" if is_canonical_log_page(value) => 3,
            _ => return false,
        };
        if rank <= last_rank {
            return false;
        }
        last_rank = rank;
    }
    true
}

fn is_canonical_log_page(value: &str) -> bool {
    value
        .parse::<u64>()
        .ok()
        .filter(|page| (1..=10_000).contains(page))
        .is_some_and(|page| page.to_string() == value)
}

pub(super) fn is_allowed_read_path(runtime: &PerfRuntime, path: &str) -> bool {
    matches!(path, "/healthz" | "/__perf/instance" | "/__perf/counters")
        || matches!(
            path,
            FIRA_SANS_LIGHT_PATH
                | FIRA_SANS_REGULAR_PATH
                | FIRA_SANS_MEDIUM_PATH
                | FIRA_SANS_SEMIBOLD_PATH
                | FIRA_SANS_BOLD_PATH
                | FIRA_CODE_VARIABLE_PATH
        )
        || match runtime.fixture_mode {
            FixtureMode::Public => path == "/",
            FixtureMode::GuildDetail => path == runtime.guild_path(),
            FixtureMode::ReadOnly => {
                matches!(path, "/" | "/selector") || path == runtime.guild_path()
            }
        }
}
