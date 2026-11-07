//! `start | stop | restart | status` — the user-facing lifecycle commands.
//!
//! `start` spawns the foreground `serve` subcommand as a detached child and
//! waits until the pidfile appears (or the child dies, whichever is first).
//! `stop` reads the pidfile and sends SIGTERM, waiting for the process to
//! exit; one still alive after [`STOP_TIMEOUT`] gets SIGKILL. `status` is a pure inspection — pidfile + liveness + health
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
/// After SIGKILL: the kernel ends the process at once; this only covers the
/// moment until `kill(pid, 0)` stops seeing it.
const KILL_WAIT: Duration = Duration::from_secs(3);
const HEALTH_TIMEOUT: Duration = Duration::from_millis(500);

/// Result of a lifecycle command — rendered as JSON to stdout.
///
/// `update` is attached only by [`start`] / [`restart`] (post-bind) so the
/// agent can prompt the user when a newer release is available. It uses
/// the cached probe — no network call on every start beyond the first.
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
        self.to_json_with_update(None)
    }

    pub fn to_json_with_update(
        &self,
        update: Option<&crate::update::UpdateInfo>,
    ) -> serde_json::Value {
        let mut value = match self {
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
        };
        if let (Some(update), Some(obj)) = (update, value.as_object_mut()) {
            obj.insert(
                "update".to_string(),
                serde_json::to_value(update).unwrap_or(serde_json::Value::Null),
            );
        }
        value
    }
}

/// Spawn a background `ling-mem serve` and wait for it to bind.
///
/// `data_dir` is propagated to the child via `$LINGGEN_DATA_DIR` so the
/// child's skill-dir resolution matches the parent's. Without this, a
/// test or custom `--data-dir` invocation would spawn a child that wrote
/// its pidfile to `~/.linggen/memory/...` while the parent polled the
/// override path and timed out.
pub async fn start(
    data_dir: &Path,
    skill_dir: &Path,
    port: u16,
    host: Option<std::net::IpAddr>,
) -> Result<LifecycleOutcome> {
    if let Some(info) = live_pidfile(skill_dir)? {
        return Ok(LifecycleOutcome::AlreadyRunning(info));
    }

    // Capture the detached daemon's stdout/stderr to a log file instead of
    // /dev/null. Without it, a daemon-side fault (the 109 GB OOM) leaves no
    // trace and the triggering request is unrecoverable. Size-rotated:
    // one previous generation kept as serve.log.1.
    // A data dir this daemon has never used has no skill dir yet.
    std::fs::create_dir_all(skill_dir)
        .with_context(|| format!("creating {}", skill_dir.display()))?;
    let log_path = skill_dir.join("serve.log");
    if std::fs::metadata(&log_path)
        .map(|m| m.len() > 5 * 1024 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&log_path, skill_dir.join("serve.log.1"));
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening daemon log {}", log_path.display()))?;
    let log_err = log_file
        .try_clone()
        .context("cloning daemon log handle for stderr")?;

    let exe = std::env::current_exe().context("resolving current executable path")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve").arg("--port").arg(port.to_string());
    // Only a flag the user typed is passed on; without one the child binds
    // the saved network setting (`http::network`), so every starter agrees.
    if let Some(host) = host {
        cmd.arg("--host").arg(host.to_string());
    }
    cmd.env("LINGGEN_DATA_DIR", data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));

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
            send_signal(child.id(), Signal::Term).ok();
            return Err(anyhow!(
                "timed out waiting for daemon to bind ({}s)",
                START_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(START_POLL_INTERVAL).await;
    }
}

/// SIGTERM the running daemon and wait for it to exit; SIGKILL it if it is
/// still alive after [`STOP_TIMEOUT`].
///
/// The escalation is what lets a new binary replace a daemon that hangs on
/// its way out — ling-mem 1.9.1 and earlier could stay alive long after
/// "daemon stopped" (see `daemon::serve::exit_now`). By then it has finished
/// serving and dropped its pidfile, so nothing is lost by the kill.
pub async fn stop(skill_dir: &Path) -> Result<LifecycleOutcome> {
    stop_within(skill_dir, STOP_TIMEOUT, KILL_WAIT).await
}

async fn stop_within(
    skill_dir: &Path,
    term_timeout: Duration,
    kill_wait: Duration,
) -> Result<LifecycleOutcome> {
    let Some(info) = pidfile::read(skill_dir)? else {
        return Ok(LifecycleOutcome::NotRunning);
    };
    if !pidfile::pid_is_alive(info.pid) {
        retire_if_stale(skill_dir, &info)?;
        return Ok(LifecycleOutcome::NotRunning);
    }

    send_signal(info.pid, Signal::Term)?;
    if wait_for_exit(info.pid, term_timeout).await {
        return Ok(stopped(skill_dir, &info));
    }

    tracing::warn!(
        "daemon (pid {}) did not exit within {}s of SIGTERM; sending SIGKILL",
        info.pid,
        term_timeout.as_secs()
    );
    // Gone in between is the outcome we wanted, not an error.
    if send_signal(info.pid, Signal::Kill).is_err() && pidfile::pid_is_alive(info.pid) {
        return Err(anyhow!(
            "daemon (pid {}) did not exit within {}s of SIGTERM, and SIGKILL was refused",
            info.pid,
            term_timeout.as_secs()
        ));
    }
    if wait_for_exit(info.pid, kill_wait).await {
        return Ok(stopped(skill_dir, &info));
    }
    Err(anyhow!(
        "daemon (pid {}) is still alive {}s after SIGKILL",
        info.pid,
        kill_wait.as_secs()
    ))
}

/// Poll until `pid` is gone or `within` has passed; true when it is gone.
async fn wait_for_exit(pid: u32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if !pidfile::pid_is_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(STOP_POLL_INTERVAL).await;
    }
}

/// The daemon `info` names has exited. Its pidfile goes too — unless another
/// daemon has written its own there meanwhile.
fn stopped(skill_dir: &Path, info: &PidInfo) -> LifecycleOutcome {
    let still_ours = pidfile::read(skill_dir)
        .ok()
        .flatten()
        .map_or(false, |now| now.pid == info.pid);
    if still_ours {
        pidfile::remove(skill_dir);
    }
    LifecycleOutcome::Stopped {
        previous_pid: info.pid,
    }
}

/// `stop` then `start`. No-op stop when already down.
pub async fn restart(
    data_dir: &Path,
    skill_dir: &Path,
    port: u16,
    host: Option<std::net::IpAddr>,
) -> Result<LifecycleOutcome> {
    let _ = stop(skill_dir).await?;
    start(data_dir, skill_dir, port, host).await
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
/// A file whose pid is gone is retired via [`retire_if_stale`] so callers
/// see consistent state on retry — unless its port is still bound, which
/// is an error, never a deletion.
fn live_pidfile(skill_dir: &Path) -> Result<Option<PidInfo>> {
    match pidfile::read(skill_dir)? {
        Some(info) if pidfile::pid_is_alive(info.pid) => Ok(Some(info)),
        Some(info) => {
            retire_if_stale(skill_dir, &info)?;
            Ok(None)
        }
        None => Ok(None),
    }
}

/// The pidfile's pid is gone. Remove the file only when its port is free
/// too: a held port means something still serves there, and a file this
/// process cannot prove stale is not its to delete. The one time this
/// mattered, the "gone" pid was a live daemon that a sandboxed shell was
/// not allowed to signal — and the deletion orphaned it from every host.
fn retire_if_stale(skill_dir: &Path, info: &PidInfo) -> Result<()> {
    if pidfile::port_is_free(info.port) {
        pidfile::remove(skill_dir);
        return Ok(());
    }
    Err(anyhow!(
        "daemon.json names pid {} (gone), but port {} is still bound — leaving the file alone; \
         free the port, then `ling-mem start`",
        info.pid,
        info.port
    ))
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

#[cfg(unix)]
fn send_signal(pid: u32, sig: Signal) -> Result<()> {
    use nix::sys::signal;
    use nix::unistd::Pid;
    let pid = i32::try_from(pid).context("pid out of range")?;
    let (sig, name) = match sig {
        Signal::Term => (signal::Signal::SIGTERM, "SIGTERM"),
        Signal::Kill => (signal::Signal::SIGKILL, "SIGKILL"),
    };
    signal::kill(Pid::from_raw(pid), sig).with_context(|| format!("{name} to pid {pid}"))
}

#[cfg(not(unix))]
fn send_signal(_pid: u32, _sig: Signal) -> Result<()> {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::net::{Ipv4Addr, TcpListener};

    fn reaped_pid() -> u32 {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id();
        child.wait().expect("reap");
        pid
    }

    fn info(pid: u32, port: u16) -> PidInfo {
        PidInfo {
            pid,
            port,
            started_at: Utc::now(),
            version: "test".into(),
        }
    }

    #[test]
    fn live_pidfile_returns_a_live_owner_untouched() {
        let dir = tempfile::tempdir().unwrap();
        pidfile::write(dir.path(), &info(std::process::id(), 1)).unwrap();
        let got = live_pidfile(dir.path()).unwrap().expect("own pid is alive");
        assert_eq!(got.pid, std::process::id());
        assert!(pidfile::path(dir.path()).exists());
    }

    #[test]
    fn live_pidfile_refuses_to_retire_a_file_whose_port_is_bound() {
        let dir = tempfile::tempdir().unwrap();
        let holder = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = holder.local_addr().unwrap().port();
        pidfile::write(dir.path(), &info(reaped_pid(), port)).unwrap();

        let err = live_pidfile(dir.path()).expect_err("held port must not be retired");
        assert!(err.to_string().contains("still bound"), "{err}");
        assert!(pidfile::path(dir.path()).exists(), "pidfile must survive");

        drop(holder);
        assert!(pidfile::wait_until_free(port));
        assert!(live_pidfile(dir.path()).unwrap().is_none());
        assert!(
            !pidfile::path(dir.path()).exists(),
            "stale file retired once the port is free"
        );
    }

    /// A child that ignores SIGTERM, reaped by a thread once it dies — an
    /// unreaped child is a zombie, and `kill(pid, 0)` still finds a zombie.
    fn term_ignoring_child() -> (u32, std::thread::JoinHandle<()>) {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 60"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn sh");
        let mut ready = String::new();
        use std::io::BufRead;
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(
            ready.trim(),
            "ready",
            "trap must be set before the test signals"
        );
        let pid = child.id();
        let reaper = std::thread::spawn(move || {
            let _ = child.wait();
        });
        (pid, reaper)
    }

    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn stop_kills_a_daemon_that_ignores_sigterm() {
        let dir = tempfile::tempdir().unwrap();
        let (pid, reaper) = term_ignoring_child();
        pidfile::write(dir.path(), &info(pid, free_port())).unwrap();

        let started = Instant::now();
        let outcome = stop_within(dir.path(), Duration::from_millis(500), KILL_WAIT)
            .await
            .expect("stop escalates instead of failing");

        assert!(
            matches!(outcome, LifecycleOutcome::Stopped { previous_pid } if previous_pid == pid),
            "{outcome:?}"
        );
        assert!(!pidfile::pid_is_alive(pid), "SIGKILL ended it");
        assert!(!pidfile::path(dir.path()).exists(), "pidfile removed");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        reaper.join().unwrap();
    }

    #[tokio::test]
    async fn stop_keeps_a_pidfile_another_daemon_wrote_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let (pid, reaper) = term_ignoring_child();
        pidfile::write(dir.path(), &info(pid, free_port())).unwrap();
        let old = pidfile::read(dir.path()).unwrap().unwrap();
        // A newer daemon took the file over before the old one was reaped.
        pidfile::write(dir.path(), &info(std::process::id(), 1)).unwrap();

        send_signal(pid, Signal::Kill).unwrap();
        assert!(wait_for_exit(pid, KILL_WAIT).await);
        stopped(dir.path(), &old);
        assert_eq!(
            pidfile::read(dir.path()).unwrap().unwrap().pid,
            std::process::id(),
            "the other daemon's pidfile survives"
        );
        reaper.join().unwrap();
    }
}
