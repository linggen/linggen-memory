//! Telemetry implementation — feature-gated. See `mod.rs` for the field list
//! and opt-out paths.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const TRACK_URL: &str = "https://linggen.dev/api/track";

/// Public telemetry handle. Cheap to clone — the inner state is shared
/// behind an Arc so the daemon can stash one in app state and clone it into
/// every middleware invocation.
#[derive(Clone)]
pub struct Telemetry {
    inner: Arc<Inner>,
}

struct Inner {
    enabled: bool,
    installation_id: String,
    product: &'static str,
    data_dir: PathBuf,
    client: reqwest::Client,
}

impl Telemetry {
    /// Construct a telemetry handle for `product`. Idempotent — safe to call
    /// every daemon start. Reads (or creates) the installation_id and the
    /// opt-out flag once. Returns a disabled handle when opt-out is in
    /// effect; all subsequent calls become no-ops.
    pub fn new(product: &'static str, data_dir: &Path) -> Self {
        let enabled = !is_opted_out(data_dir);
        let installation_id = if enabled {
            load_or_create_installation_id(data_dir).unwrap_or_default()
        } else {
            String::new()
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            inner: Arc::new(Inner {
                enabled,
                installation_id,
                product,
                data_dir: data_dir.to_path_buf(),
                client,
            }),
        }
    }

    /// Fire the `install` event when appropriate: first launch ever on this
    /// machine, or a version change since the previous launch. No
    /// heartbeat — DAU is derived server-side from any event row, since
    /// every active user produces at least one `command` event per day.
    /// Non-blocking; persists `last_version` synchronously so a crash
    /// before the POST returns doesn't cause double-fires next start.
    pub fn launch(&self) {
        if !self.inner.enabled || self.inner.installation_id.is_empty() {
            return;
        }
        let state_path = state_path(&self.inner.data_dir, self.inner.product);
        let prev = load_state(&state_path).unwrap_or_default();

        let install_payload = if prev.last_version.is_empty() {
            // installation_id was either just created OR existed but this
            // product never wrote a state file before. Either way, treat
            // as install. `via` comes from the install-source marker
            // file written by the installer; missing → "unknown".
            let mut p = read_install_source(&self.inner.data_dir, self.inner.product);
            p.insert("via".into(), p.get("via").cloned().unwrap_or_else(|| "unknown".into()));
            Some(p)
        } else if prev.last_version != APP_VERSION {
            let mut p = std::collections::BTreeMap::new();
            p.insert("via".into(), "upgrade".into());
            p.insert("from_version".into(), prev.last_version.clone());
            p.insert("to_version".into(), APP_VERSION.into());
            Some(p)
        } else {
            None
        };

        if let Some(payload) = install_payload {
            self.spawn_post("install", Some(serde_json::to_value(payload).unwrap_or(serde_json::Value::Null)));
        }

        let new_state = State { last_version: APP_VERSION.into() };
        let _ = save_state(&state_path, &new_state);
    }

    /// Record a Memory.* call. Non-blocking. `verb` is the bare method
    /// name ("search", "add", "memory.search" — caller's choice; the
    /// server stores it verbatim in payload.verb).
    pub fn command(&self, verb: &str) {
        if !self.inner.enabled || self.inner.installation_id.is_empty() {
            return;
        }
        let payload = serde_json::json!({ "verb": verb });
        self.spawn_post("command", Some(payload));
    }

    fn spawn_post(&self, event_type: &'static str, payload: Option<serde_json::Value>) {
        let body = serde_json::json!({
            "installation_id": self.inner.installation_id,
            "event_type": event_type,
            "app_version": APP_VERSION,
            "platform": platform_name(),
            "product": self.inner.product,
            "payload": payload,
        });
        let client = self.inner.client.clone();
        tokio::spawn(async move {
            let _ = client
                .post(TRACK_URL)
                .json(&body)
                .send()
                .await;
            // Errors are intentionally swallowed — we never want telemetry
            // to surface to the user. Rely on server-side observability.
        });
    }
}

// ── installation_id ─────────────────────────────────────────────────────────

fn installation_id_path(data_dir: &Path) -> PathBuf {
    data_dir.join("installation_id")
}

fn load_or_create_installation_id(data_dir: &Path) -> std::io::Result<String> {
    let path = installation_id_path(data_dir);
    if let Ok(s) = std::fs::read_to_string(&path) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    std::fs::create_dir_all(data_dir).ok();
    std::fs::write(&path, &id)?;
    Ok(id)
}

// ── opt-out ─────────────────────────────────────────────────────────────────

fn is_opted_out(data_dir: &Path) -> bool {
    if matches!(std::env::var("LING_MEM_NO_TELEMETRY").as_deref(), Ok("1") | Ok("true") | Ok("yes")) {
        return true;
    }
    if data_dir.join("no-telemetry").exists() {
        return true;
    }
    false
}

// ── per-product state ──────────────────────────────────────────────────────

#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    last_version: String,
}

fn state_path(data_dir: &Path, product: &str) -> PathBuf {
    data_dir.join(format!(".{product}-telemetry"))
}

fn load_state(path: &Path) -> std::io::Result<State> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes).unwrap_or_default())
}

fn save_state(path: &Path, state: &State) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let bytes = serde_json::to_vec(state).unwrap_or_default();
    std::fs::write(path, bytes)
}

// ── install-source marker ──────────────────────────────────────────────────

/// Read the install-source marker file written by the installer (wrapper,
/// linggen-engine bootstrap, clawhub, brew, etc.) into a payload map.
/// Missing file → empty map; caller fills in `via=unknown`.
fn read_install_source(data_dir: &Path, product: &str) -> std::collections::BTreeMap<String, String> {
    let path = data_dir.join(format!(".{product}-install-source"));
    let mut map = std::collections::BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    map
}

// ── platform detection ─────────────────────────────────────────────────────

fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}


