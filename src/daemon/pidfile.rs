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

/// True if a process with `pid` exists on this host.
///
/// `kill(pid, 0)` answers three ways. `Ok` and `EPERM` both mean the pid is
/// taken — EPERM is "exists, but you may not signal it", which is exactly
/// what a sandboxed shell sees when it looks at a daemon it didn't spawn
/// (Codex's seatbelt denies signals to non-children). Only `ESRCH` means
/// the process is gone. Reading EPERM as dead is how a sandboxed `ling-mem`
/// once deleted a live daemon's pidfile and orphaned it from every host.
#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal;
    use nix::unistd::Pid;
    let pid = match i32::try_from(pid) {
        Ok(p) => Pid::from_raw(p),
        Err(_) => return false,
    };
    match signal::kill(pid, None) {
        Ok(()) | Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
pub fn pid_is_alive(_pid: u32) -> bool {
    // Windows implementation deferred — daemon mode is Unix-first.
    false
}

/// True when nothing on this host holds TCP `port`.
///
/// Probed by binding the wildcard address with `SO_REUSEADDR` off, so the
/// answer is the same on macOS and Linux: any holder of the port, on any
/// interface, makes the bind fail. Binding rather than connecting because a
/// sandbox that closes loopback makes a live daemon look unreachable, but
/// cannot make a held port bindable. Every failure — in use, or refused by
/// a sandbox — counts as "not free": this process cannot prove the port is
/// its to take.
#[cfg(unix)]
pub fn port_is_free(port: u16) -> bool {
    use nix::sys::socket::{bind, socket, AddressFamily, SockFlag, SockType, SockaddrIn};
    use std::os::fd::AsRawFd;
    let Ok(fd) = socket(
        AddressFamily::Inet,
        SockType::Stream,
        SockFlag::empty(),
        None,
    ) else {
        return false;
    };
    bind(fd.as_raw_fd(), &SockaddrIn::new(0, 0, 0, 0, port)).is_ok()
}

#[cfg(not(unix))]
pub fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).is_ok()
}

/// Test helper: poll until `port` reads free, or give up after ~1s.
///
/// A dropped listener is closed at once, but a test binary spawning
/// children in parallel lends each forked child a copy of every open fd
/// until it execs — so the port can read held for a few microseconds after
/// the drop. Production takes the conservative reading (leave the file,
/// tell the user); tests wait it out.
#[cfg(test)]
pub(crate) fn wait_until_free(port: u16) -> bool {
    for _ in 0..100 {
        if port_is_free(port) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn pid_one_is_alive_even_when_unsignalable() {
        // launchd / init: exists, and EPERM for a non-root caller — the
        // sandbox shape, reproduced without a sandbox.
        assert!(pid_is_alive(1));
    }

    #[test]
    fn reaped_child_is_dead() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id();
        child.wait().expect("reap");
        assert!(!pid_is_alive(pid));
    }

    #[test]
    fn held_port_is_not_free_until_released() {
        // The holder binds loopback with SO_REUSEADDR (std's default); the
        // probe must still see the conflict, and must see it clear.
        let holder = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = holder.local_addr().unwrap().port();
        assert!(!port_is_free(port));
        drop(holder);
        assert!(wait_until_free(port));
    }
}
