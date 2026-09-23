mod config;
mod control;
mod fixture;
mod ready;
mod repositories;
mod runtime;
mod security;
mod server;

#[cfg(test)]
use std::{env, io::ErrorKind, path::Path, process, sync::atomic::Ordering};

#[cfg(test)]
use axum::{Router, response::Response};

#[cfg(test)]
use crate::{FIRA_SANS_REGULAR_PATH, SESSION_COOKIE_NAME};
#[cfg(test)]
use config::{
    ENV_FIXTURE_MODE, ENV_FIXTURE_SHA256, ENV_FIXTURE_VERSION, ENV_NONCE, ENV_READY_FILE,
    ENV_REVISION, FORBIDDEN_ENVIRONMENT, FixtureIdentity, FixtureMode, PerfHarnessConfig,
    fixture_bytes_sha256, is_lower_hex, validate_compiled_revision,
};
#[cfg(test)]
use dynamo_settings::{
    DeploymentCommandSettings, DeploymentModuleSettings, GuildCommandSettings, GuildModuleSettings,
};
#[cfg(test)]
use fixture::FixtureData;
#[cfg(test)]
use ready::{ReadyFile, publish_ready_bytes, write_ready_file};
#[cfg(test)]
use server::{build_fixture_state, build_perf_router};

pub(crate) use runtime::PerfRuntime;
pub use server::run_perf_harness;

const PERF_CONTROL_HEADER: &str = "x-dynamo-perf-control";
const SCHEMA_VERSION: u32 = 1;

#[cfg(test)]
mod tests;
