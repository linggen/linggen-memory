//! `start | stop | restart | status` — the user-facing lifecycle commands.
//!
//! `start` spawns the foreground `serve` subcommand as a detached child and
//! waits until the pidfile appears (or the child dies, whichever is first).
//! `stop` reads the pidfile and sends SIGTERM, waiting for the pidfile to
//! disappear. `status` is a pure inspection — pidfile + liveness + health
//! probe against the daemon's HTTP port.

use crate::daemon::pidfile::{self, PidInfo};
use crate::daemon::DEFAULT_PORT;
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

const START_POLL_INTERVAL: Duration = Duration::from_millis(100);
const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_TIMEOUT: Duration = Duration::from_millis(500);

/// Result of a lifecycle command — rendered as JSON to stdout.
#[derive(Debug)]
pub enum LifecycleOutcome {
    Running(PidInfo),
    Started(PidInfo),
    AlreadyRunning(PidInfo),
    Stopped { previous_pid: u32 },
    NotRunning,
}

impl LifecycleOutcome {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            LifecycleOutcome::Running(info) => json!({
                "state": "running",
                "pid": info.pid,
                "port": info.port,
                "started_at": info.started_at,
                "version": info.version,
            }),
            LifecycleOutcome::Started(info) => json!({
                "state": "started",
                "pid": info.pid,
                "port": info.port,
                "started_at": info.started_at,
                "version": info.version,
            }),
            LifecycleOutcome::AlreadyRunning(info) => json!({
                "state": "already_running",
                "pid": info.pid,
                "port": info.port,
                "started_at": info.started_at,
                "version": info.version,
            }),
            LifecycleOutcome::Stopped { previous_pid } => json!({
                "state": "stopped",
                "previous_pid": previous_pid,
            }),
            LifecycleOutcome::NotRunning => json!({"state": "not_running"}),
        }
    }
}

/// Spawn a background `ling-mem serve` and wait for it to bind.
///
/// `data_dir` is propagated to the child via `$LINGGEN_DATA_DIR` so the
/// child's skill-dir resolution matches the parent's. Without this, a
/// test or custom `--data-dir` invocation would spawn a child that wrote
/// its pidfile to `~/.linggen/memory/...` while the parent polled the
/// override path and timed out.
pub async fn start(data_dir: &Path, skill_dir: &Path, port: u16) -> Result<LifecycleOutcome> {
    if let Some(info) = live_pidfile(skill_dir)? {
        return Ok(LifecycleOutcome::AlreadyRunning(info));
    }

    let exe = std::env::current_exe().context("resolving current executable path")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .env("LINGGEN_DATA_DIR", data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().context("spawning `ling-mem serve`")?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(info) = live_pidfile(skill_dir)? {
            // Intentionally don't .wait() — the child is detached.
            return Ok(LifecycleOutcome::Started(info));
        }
        if let Some(status) = child.try_wait().context("polling child")? {
            return Err(anyhow!(
                "`ling-mem serve` exited before binding (status {status})"
            ));
        }
        if Instant::now() >= deadline {
            // SIGTERM so the child runs its cleanup path (pidfile removal,
            // connection drain) — SIGKILL would leave a stale pidfile.
            send_sigterm(child.id()).ok();
            return Err(anyhow!(
                "timed out waiting for daemon to bind ({}s)",
                START_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(START_POLL_INTERVAL).await;
    }
}

/// SIGTERM the running daemon and wait for the pidfile to disappear.
pub async fn stop(skill_dir: &Path) -> Result<LifecycleOutcome> {
    let Some(info) = pidfile::read(skill_dir)? else {
        return Ok(LifecycleOutcome::NotRunning);
    };
    if !pidfile::pid_is_alive(info.pid) {
        pidfile::remove(skill_dir);
        return Ok(LifecycleOutcome::NotRunning);
    }

    send_sigterm(info.pid)?;

    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !pidfile::pid_is_alive(info.pid) {
            pidfile::remove(skill_dir);
            return Ok(LifecycleOutcome::Stopped {
                previous_pid: info.pid,
            });
        }
        tokio::time::sleep(STOP_POLL_INTERVAL).await;
    }

    Err(anyhow!(
        "daemon (pid {}) did not exit within {}s of SIGTERM",
        info.pid,
        STOP_TIMEOUT.as_secs()
    ))
}

/// `stop` then `start`. No-op stop when already down.
pub async fn restart(data_dir: &Path, skill_dir: &Path, port: u16) -> Result<LifecycleOutcome> {
    let _ = stop(skill_dir).await?;
    start(data_dir, skill_dir, port).await
}

/// Pure inspection — does not spawn or signal anything.
pub async fn status(skill_dir: &Path) -> Result<serde_json::Value> {
    let Some(info) = pidfile::read(skill_dir)? else {
        return Ok(json!({"state": "not_running"}));
    };
    if !pidfile::pid_is_alive(info.pid) {
        return Ok(json!({
            "state": "stale_pidfile",
            "pid": info.pid,
            "port": info.port,
        }));
    }
    let health = probe_health(info.port).await;
    Ok(json!({
        "state": "running",
        "pid": info.pid,
        "port": info.port,
        "started_at": info.started_at,
        "version": info.version,
        "health": health,
    }))
}

/// Cheap HTTP probe against `/api/health`. Returns `"ok"`, `"error"`, or
/// `"unreachable"` so `status` output is always useful even when the
/// daemon is wedged.
async fn probe_health(port: u16) -> String {
    let url = format!("http://127.0.0.1:{port}/api/health");
    let client = match reqwest::Client::builder().timeout(HEALTH_TIMEOUT).build() {
        Ok(c) => c,
        Err(_) => return "unreachable".to_string(),
    };
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => "ok".to_string(),
        Ok(_) => "error".to_string(),
        Err(_) => "unreachable".to_string(),
    }
}

/// Return `Some(info)` only when the pidfile exists *and* its pid is alive.
/// Stale files are cleaned up as a side effect so callers see consistent
/// state on retry.
fn live_pidfile(skill_dir: &Path) -> Result<Option<PidInfo>> {
    match pidfile::read(skill_dir)? {
        Some(info) if pidfile::pid_is_alive(info.pid) => Ok(Some(info)),
        Some(_) => {
            pidfile::remove(skill_dir);
            Ok(None)
        }
        None => Ok(None),
    }
}

#[cfg(unix)]
fn send_sigterm(pid: u32) -> Result<()> {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;
    let pid = i32::try_from(pid).context("pid out of range")?;
    signal::kill(Pid::from_raw(pid), Signal::SIGTERM)
        .with_context(|| format!("SIGTERM to pid {pid}"))
}

#[cfg(not(unix))]
fn send_sigterm(_pid: u32) -> Result<()> {
    Err(anyhow!(
        "daemon stop is only implemented on Unix in this release"
    ))
}

#[allow(dead_code)]
const _: () = {
    // Reference DEFAULT_PORT so callers importing this module can see it
    // documented in one place without a separate `use` dance.
    let _ = DEFAULT_PORT;
};
