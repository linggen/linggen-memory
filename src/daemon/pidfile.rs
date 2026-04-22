//! `daemon.json` — discovery file for the running daemon.
//!
//! Written atomically on bind, read on every lifecycle / status call, and
//! removed on graceful shutdown. Missing file or stale pid both mean "not
//! running" — callers must handle both.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "daemon.json";

/// Runtime info written by a live daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PidInfo {
    pub pid: u32,
    pub port: u16,
    pub started_at: DateTime<Utc>,
    pub version: String,
}

/// Path to the daemon.json under the skill dir.
pub fn path(skill_dir: &Path) -> PathBuf {
    skill_dir.join(FILE_NAME)
}

/// Write the pidfile atomically. Creates the skill dir if needed.
pub fn write(skill_dir: &Path, info: &PidInfo) -> Result<()> {
    fs::create_dir_all(skill_dir)
        .with_context(|| format!("creating skill dir {}", skill_dir.display()))?;

    let tmp = skill_dir.join(format!("{FILE_NAME}.tmp"));
    let body = serde_json::to_vec_pretty(info).context("serializing pidfile")?;
    fs::write(&tmp, &body).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path(skill_dir)).context("renaming pidfile into place")?;
    Ok(())
}

/// Read and parse the pidfile. `Ok(None)` when it doesn't exist.
pub fn read(skill_dir: &Path) -> Result<Option<PidInfo>> {
    let p = path(skill_dir);
    match fs::read(&p) {
        Ok(bytes) => {
            let info: PidInfo = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", p.display()))?;
            Ok(Some(info))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::from(e).context(format!("reading {}", p.display()))),
    }
}

/// Best-effort removal. Absent file is not an error.
pub fn remove(skill_dir: &Path) {
    let _ = fs::remove_file(path(skill_dir));
}

/// True if a process with `pid` is alive on this host.
#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
    // `kill -0` returns 0 if the process exists and we can signal it.
    use nix::sys::signal;
    use nix::unistd::Pid;
    let pid = match i32::try_from(pid) {
        Ok(p) => Pid::from_raw(p),
        Err(_) => return false,
    };
    signal::kill(pid, None).is_ok()
}

#[cfg(not(unix))]
pub fn pid_is_alive(_pid: u32) -> bool {
    // Windows implementation deferred — daemon mode is Unix-first.
    false
}
