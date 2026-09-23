use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process,
};

use anyhow::{Context, ensure};
use serde::Serialize;

use super::config::{FixtureIdentity, FixtureMode, random_control_secret, validate_ready_path};

#[derive(Serialize)]
pub(super) struct ReadyFile<'a> {
    pub(super) schema_version: u32,
    pub(super) host: &'static str,
    pub(super) dynamic_port: bool,
    pub(super) port: u16,
    pub(super) pid: u32,
    pub(super) revision: &'a str,
    pub(super) nonce: &'a str,
    pub(super) fixture_mode: FixtureMode,
    pub(super) fixture: &'a FixtureIdentity,
    pub(super) guild_id: u64,
    pub(super) cookie_name: &'static str,
    pub(super) cookie_value: &'a str,
}

pub(super) fn write_ready_file(path: &Path, ready: &ReadyFile<'_>) -> anyhow::Result<()> {
    validate_ready_path(path)?;
    let mut body =
        serde_json::to_vec(ready).context("failed to serialize performance ready file")?;
    body.push(b'\n');
    publish_ready_bytes(path, &body)
}

struct TemporaryReadyFile {
    path: PathBuf,
}

impl Drop for TemporaryReadyFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn publish_ready_bytes(path: &Path, body: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().context("ready file parent is required")?;
    let final_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("ready file name must be valid Unicode")?;
    let mut created = None;
    for _ in 0..16 {
        let unique = random_control_secret()?;
        let candidate = parent.join(format!(".{final_name}.{}.{}.tmp", process::id(), unique));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                created = Some((file, candidate));
                break;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).context("failed to create same-directory ready temp file");
            }
        }
    }
    let (mut file, temporary_path) =
        created.context("could not reserve a unique ready temp file")?;
    let _cleanup = TemporaryReadyFile {
        path: temporary_path.clone(),
    };
    file.write_all(body)
        .context("failed to write performance ready temp file")?;
    file.flush()
        .context("failed to flush performance ready temp file")?;
    file.sync_all()
        .context("failed to sync performance ready temp file")?;
    let temp_readback =
        fs::read(&temporary_path).context("failed to read back performance ready temp file")?;
    ensure!(
        temp_readback == body,
        "performance ready temp file readback mismatch"
    );
    validate_ready_path(path)?;
    fs::hard_link(&temporary_path, path)
        .context("failed to publish performance ready file without replacement")?;
    let published_metadata =
        fs::symlink_metadata(path).context("failed to inspect published performance ready file")?;
    ensure!(
        published_metadata.is_file() && !published_metadata.file_type().is_symlink(),
        "published performance ready file is not a regular non-symlink file"
    );
    let published_readback =
        fs::read(path).context("failed to read back published performance ready file")?;
    ensure!(
        published_readback == body,
        "published performance ready file readback mismatch"
    );
    Ok(())
}
