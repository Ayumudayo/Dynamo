use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, ensure};
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(super) const ENV_FIXTURE_MODE: &str = "DYNAMO_PERF_FIXTURE_MODE";
pub(super) const ENV_FIXTURE_SHA256: &str = "DYNAMO_PERF_FIXTURE_SHA256";
pub(super) const ENV_FIXTURE_VERSION: &str = "DYNAMO_PERF_FIXTURE_VERSION";
pub(super) const ENV_NONCE: &str = "DYNAMO_PERF_NONCE";
pub(super) const ENV_READY_FILE: &str = "DYNAMO_PERF_READY_FILE";
pub(super) const ENV_REVISION: &str = "DYNAMO_PERF_REVISION";
pub(super) const FIXTURE_BYTES: &[u8] =
    include_bytes!("../../../../tests/perf/fixtures/guild-detail-v1.json");

pub(super) const FORBIDDEN_ENVIRONMENT: &[&str] = &[
    "DASHBOARD_HOST",
    "DASHBOARD_PORT",
    "DASHBOARD_BASE_URL",
    "PERF_BASE_URL",
    "DYNAMO_PERF_HOST",
    "DYNAMO_PERF_PORT",
    "DYNAMO_PERF_BASE_URL",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub(super) enum FixtureMode {
    Public,
    GuildDetail,
    ReadOnly,
}

impl FixtureMode {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "Public" => Ok(Self::Public),
            "GuildDetail" => Ok(Self::GuildDetail),
            "ReadOnly" => Ok(Self::ReadOnly),
            _ => anyhow::bail!("{ENV_FIXTURE_MODE} must be Public, GuildDetail, or ReadOnly"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct FixtureIdentity {
    pub(super) version: String,
    pub(super) sha256: String,
}

#[derive(Debug, Clone)]
pub(super) struct PerfHarnessConfig {
    pub(super) revision: String,
    pub(super) nonce: String,
    pub(super) fixture_mode: FixtureMode,
    pub(super) fixture: FixtureIdentity,
    pub(super) ready_file: PathBuf,
}

impl PerfHarnessConfig {
    pub(super) fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    pub(super) fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        for key in FORBIDDEN_ENVIRONMENT {
            ensure!(
                lookup(key).is_none(),
                "{key} is forbidden for the dashboard performance harness"
            );
        }
        let revision = required_value(&mut lookup, ENV_REVISION)?;
        ensure!(
            is_lower_hex(&revision, 40),
            "{ENV_REVISION} must be exactly 40 lowercase hexadecimal characters"
        );
        let nonce = required_value(&mut lookup, ENV_NONCE)?;
        ensure!(
            is_lower_hex(&nonce, 64),
            "{ENV_NONCE} must be exactly 64 lowercase hexadecimal characters"
        );
        let fixture_mode = FixtureMode::parse(&required_value(&mut lookup, ENV_FIXTURE_MODE)?)?;
        let fixture_version = required_value(&mut lookup, ENV_FIXTURE_VERSION)?;
        ensure!(
            is_safe_fixture_version(&fixture_version),
            "{ENV_FIXTURE_VERSION} is invalid"
        );
        let fixture_sha256 = required_value(&mut lookup, ENV_FIXTURE_SHA256)?;
        ensure!(
            is_lower_hex(&fixture_sha256, 64),
            "{ENV_FIXTURE_SHA256} must be exactly 64 lowercase hexadecimal characters"
        );
        ensure!(
            fixture_sha256 == fixture_bytes_sha256(),
            "{ENV_FIXTURE_SHA256} does not match the compiled fixture bytes"
        );
        let ready_file = PathBuf::from(required_value(&mut lookup, ENV_READY_FILE)?);
        validate_ready_path(&ready_file)?;
        Ok(Self {
            revision,
            nonce,
            fixture_mode,
            fixture: FixtureIdentity {
                version: fixture_version,
                sha256: fixture_sha256,
            },
            ready_file,
        })
    }
}

pub(super) fn validate_compiled_revision(
    runtime_revision: &str,
    compiled_revision: Option<&str>,
) -> anyhow::Result<()> {
    let compiled_revision = compiled_revision
        .context("performance harness binary lacks DYNAMO_PERF_COMPILED_REVISION provenance")?;
    ensure!(
        is_lower_hex(compiled_revision, 40),
        "compiled performance harness revision is invalid"
    );
    ensure!(
        compiled_revision == runtime_revision,
        "compiled performance harness revision does not match DYNAMO_PERF_REVISION"
    );
    Ok(())
}

fn required_value(
    lookup: &mut impl FnMut(&str) -> Option<String>,
    key: &str,
) -> anyhow::Result<String> {
    let value = lookup(key).with_context(|| format!("{key} is required"))?;
    ensure!(!value.is_empty(), "{key} must not be empty");
    Ok(value)
}

pub(super) fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_safe_fixture_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

pub(super) fn validate_ready_path(path: &Path) -> anyhow::Result<()> {
    ensure!(
        path.is_absolute(),
        "{ENV_READY_FILE} must be an absolute path"
    );
    ensure!(
        path.file_name().is_some(),
        "{ENV_READY_FILE} must name a file"
    );
    let parent = path
        .parent()
        .context("DYNAMO_PERF_READY_FILE must have a parent directory")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .context("DYNAMO_PERF_READY_FILE parent directory could not be inspected")?;
    ensure!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "{ENV_READY_FILE} parent must be an existing non-symlink directory"
    );
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("{ENV_READY_FILE} already exists"),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("DYNAMO_PERF_READY_FILE could not be inspected"),
    }
}

pub(super) fn fixture_bytes_sha256() -> String {
    format!("{:x}", Sha256::digest(FIXTURE_BYTES))
}

pub(super) fn random_control_secret() -> anyhow::Result<String> {
    let mut random = [0_u8; 32];
    OsRng
        .try_fill_bytes(&mut random)
        .context("failed to obtain operating-system randomness for harness control capability")?;
    let mut secret = String::with_capacity(69);
    secret.push_str("perf_");
    const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in random {
        secret.push(LOWER_HEX[usize::from(byte >> 4)] as char);
        secret.push(LOWER_HEX[usize::from(byte & 0x0f)] as char);
    }
    Ok(secret)
}
