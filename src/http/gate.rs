//! The LAN gate — who may reach this store from off-machine.
//!
//! The daemon binds loopback and nothing else unless someone asks for more, so
//! the ordinary user — one machine, memory for their own agents — never opens a
//! port and never needs any of this. It exists for the one case that is real:
//! a second machine on the LAN whose agents should share ONE memory, rather
//! than growing a second binary, a second daemon, and a second store that
//! forks the user's biography.
//!
//! Two rules, and the second is why the first is safe:
//!
//! 1. **A wide bind is refused unless the machine has paired devices.**
//!    Opening a port before there is anything to check a caller against is an
//!    open store on the network, so `--host` fails loudly instead.
//! 2. **A non-loopback request must carry a paired device's token.** Loopback
//!    stays open: the CLI, the engine, and the local app all talk to
//!    `127.0.0.1`, and a process already on this machine is inside the fence.
//!
//! The auth is **not new**. Both daemons live under the same `~/.linggen`, so
//! this reads the very file the engine's pairing flow writes — pair once, on
//! the Mac's screen, and both daemons accept the device. There is no second
//! enrolment, no second secret, and nothing here mints one: this file only ever
//! reads.

use super::envelope::ApiError;
use super::state::SharedState;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

/// The header a paired device presents. Same name the engine's gate reads —
/// one device, one token, either daemon.
const DEVICE_HEADER: &str = "x-linggen-device";

/// Written by the engine's pairing flow (`server/api/pair.rs`). Only the
/// secret matters here; the rest of each row is the Mac's business.
#[derive(Deserialize)]
struct PairedDevice {
    #[serde(default)]
    secret: String,
}

pub fn devices_path(data_dir: &Path) -> PathBuf {
    data_dir.join("paired-devices.json")
}

/// Has this machine ever paired a device? The precondition for binding wide.
///
/// The file's *existence* is the question, not its contents: an empty list is a
/// machine whose devices were all revoked, and re-binding it wide should still
/// be allowed — every request will simply be refused until something pairs.
pub fn has_paired_devices_file(data_dir: &Path) -> bool {
    devices_path(data_dir).is_file()
}

/// Does any paired device own this token?
///
/// Read per request rather than cached: the file is tiny, and a device revoked
/// on the Mac must stop working now, not at the next daemon restart.
fn is_paired_token(data_dir: &Path, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(devices_path(data_dir)) else {
        return false;
    };
    let devices: Vec<PairedDevice> = serde_json::from_str(&text).unwrap_or_default();
    devices.iter().any(|d| d.secret == token)
}

/// Refuse a wide bind on a machine that has never paired a device.
///
/// Called before the listener exists, so the failure is a daemon that does not
/// start rather than a port that is open and unguarded for the moments it takes
/// someone to notice.
pub fn check_bind(host: IpAddr, data_dir: &Path) -> anyhow::Result<()> {
    if host.is_loopback() || has_paired_devices_file(data_dir) {
        return Ok(());
    }
    anyhow::bail!(
        "refusing to bind {host}: no paired devices on this machine, so every \
         request from off-machine would be unauthenticated. Pair the other \
         machine with Linggen first (its pairing flow writes {}), then start \
         again. Loopback needs none of this and is the default.",
        devices_path(data_dir).display()
    )
}

/// Everything a caller off this machine must pass. Loopback goes straight
/// through — it is the CLI, the engine, and the local app, all of which are
/// already inside the fence.
///
/// `/api/health` is deliberately open: it is a liveness probe that returns a
/// version and nothing about the user, and a pairing UI needs to see the daemon
/// before it has a token to show it.
pub async fn lan_gate(
    State(state): State<SharedState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    if peer.ip().is_loopback() || req.uri().path() == "/api/health" {
        return next.run(req).await;
    }
    let token = req
        .headers()
        .get(DEVICE_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if is_paired_token(&state.data_dir, &token) {
        return next.run(req).await;
    }
    tracing::warn!("refused {} from {} — no paired device token", req.uri().path(), peer.ip());
    ApiError::unauthorized(format!(
        "this store is reachable from your machine but not open to it: pair with \
         Linggen and send the device token as `{DEVICE_HEADER}`"
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_never_needs_a_paired_device() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_bind("127.0.0.1".parse().unwrap(), dir.path()).is_ok());
        assert!(check_bind("::1".parse().unwrap(), dir.path()).is_ok());
    }

    /// Binding wide before anything can be checked is an open store, so the
    /// daemon refuses to start rather than opening the port and hoping.
    #[test]
    fn a_wide_bind_is_refused_until_something_is_paired() {
        let dir = tempfile::tempdir().unwrap();
        let err = check_bind("0.0.0.0".parse().unwrap(), dir.path()).unwrap_err();
        assert!(err.to_string().contains("no paired devices"));

        std::fs::write(devices_path(dir.path()), "[]").unwrap();
        assert!(
            check_bind("0.0.0.0".parse().unwrap(), dir.path()).is_ok(),
            "an all-revoked machine may still bind — every request is refused anyway"
        );
    }

    /// The file the engine's pairing flow writes, read as-is. Nothing here
    /// mints a token; a row this daemon cannot parse grants nothing.
    #[test]
    fn a_token_is_paired_only_if_the_engines_file_says_so() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            devices_path(dir.path()),
            r#"[{"id":"a","name":"DS242","secret":"cafe1234","created_at":0}]"#,
        )
        .unwrap();
        assert!(is_paired_token(dir.path(), "cafe1234"));
        assert!(!is_paired_token(dir.path(), "beef0000"));
        assert!(!is_paired_token(dir.path(), ""));

        std::fs::write(devices_path(dir.path()), "not json").unwrap();
        assert!(!is_paired_token(dir.path(), "cafe1234"));
    }

    #[test]
    fn a_missing_file_grants_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_paired_devices_file(dir.path()));
        assert!(!is_paired_token(dir.path(), "anything"));
    }
}
