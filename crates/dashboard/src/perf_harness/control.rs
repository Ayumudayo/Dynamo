use std::{process, sync::atomic::Ordering};

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use super::{SCHEMA_VERSION, runtime::PerfRuntime};
use crate::DashboardState;

#[derive(Debug, Serialize)]
pub(super) struct InstanceSnapshot<'a> {
    schema_version: u32,
    revision: &'a str,
    nonce: &'a str,
    pid: u32,
    fixture_mode: super::config::FixtureMode,
    fixture: &'a super::config::FixtureIdentity,
    outbound_calls: u64,
    browser_outbound_attempts: u64,
}

#[derive(Debug, Serialize)]
pub(super) struct CounterSnapshot {
    schema_version: u32,
    denied_requests: u64,
    server_write_attempts: u64,
    repository_reads: u64,
    repository_mutations: u64,
    provider_guild_lookups: u64,
    outbound_calls: u64,
    browser_outbound_attempts: u64,
}

pub(super) fn instance_snapshot(runtime: &PerfRuntime) -> InstanceSnapshot<'_> {
    InstanceSnapshot {
        schema_version: SCHEMA_VERSION,
        revision: &runtime.revision,
        nonce: &runtime.nonce,
        pid: process::id(),
        fixture_mode: runtime.fixture_mode,
        fixture: &runtime.fixture,
        outbound_calls: runtime.outbound_calls.load(Ordering::SeqCst),
        browser_outbound_attempts: runtime.browser_outbound_attempts.load(Ordering::SeqCst),
    }
}

pub(super) fn counter_snapshot(runtime: &PerfRuntime) -> CounterSnapshot {
    CounterSnapshot {
        schema_version: SCHEMA_VERSION,
        denied_requests: runtime.denied_requests.load(Ordering::SeqCst),
        server_write_attempts: runtime.server_write_attempts.load(Ordering::SeqCst),
        repository_reads: runtime.repository_reads.load(Ordering::SeqCst),
        repository_mutations: runtime.repository_mutations.load(Ordering::SeqCst),
        provider_guild_lookups: runtime.provider_guild_lookups.load(Ordering::SeqCst),
        outbound_calls: runtime.outbound_calls.load(Ordering::SeqCst),
        browser_outbound_attempts: runtime.browser_outbound_attempts.load(Ordering::SeqCst),
    }
}

pub(super) async fn perf_instance(State(state): State<std::sync::Arc<DashboardState>>) -> Response {
    match state.perf_runtime.as_deref() {
        Some(runtime) => Json(instance_snapshot(runtime)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn perf_counters(State(state): State<std::sync::Arc<DashboardState>>) -> Response {
    match state.perf_runtime.as_deref() {
        Some(runtime) => Json(counter_snapshot(runtime)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub(super) async fn record_browser_outbound_attempt(
    State(state): State<std::sync::Arc<DashboardState>>,
) -> Response {
    let Some(runtime) = state.perf_runtime.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    runtime.increment_browser_outbound_attempts();
    StatusCode::NO_CONTENT.into_response()
}

pub(super) async fn shutdown_harness(
    State(state): State<std::sync::Arc<DashboardState>>,
) -> Response {
    let Some(runtime) = state.perf_runtime.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if runtime.trigger_shutdown() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::CONFLICT.into_response()
    }
}
