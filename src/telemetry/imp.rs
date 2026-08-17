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
    digest: crate::telemetry::digest::Digest,
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
                digest: crate::telemetry::digest::Digest::load(data_dir, product),
            }),
        }
    }

    /// Fire the `install` event when appropriate: first launch ever on this
    /// machine, or a version change since the previous launch. Non-blocking;
    /// persists `last_version` synchronously so a crash before the POST
    /// returns doesn't cause double-fires next start.
    pub fn launch(&self) {
        if !self.inner.enabled || self.inner.installation_id.is_empty() {
            return;
        }
        let state_path = state_path(&self.inner.data_dir, self.inner.product);
        let mut state = load_state(&state_path).unwrap_or_default();

        let install_payload = if state.last_version.is_empty() {
            // installation_id was either just created OR existed but this
            // product never wrote a state file before. Either way, treat
            // as install. `via` comes from the install-source marker
            // file written by the installer; missing → "unknown".
            let mut p = read_install_source(&self.inner.data_dir, self.inner.product);
            p.insert("via".into(), p.get("via").cloned().unwrap_or_else(|| "unknown".into()));
            Some(p)
        } else if state.last_version != APP_VERSION {
            let mut p = std::collections::BTreeMap::new();
            p.insert("via".into(), "upgrade".into());
            p.insert("from_version".into(), state.last_version.clone());
            p.insert("to_version".into(), APP_VERSION.into());
            Some(p)
        } else {
            None
        };

        if let Some(payload) = install_payload {
            self.spawn_post("install", Some(serde_json::to_value(payload).unwrap_or(serde_json::Value::Null)));
        }

        // Read-modify-write: preserve last_command_day across launch.
        state.last_version = APP_VERSION.into();
        let _ = save_state(&state_path, &state);

        self.ship_digests();
    }

    /// Count one occurrence of `key` in today's local digest. Nothing is sent
    /// now; completed days ship as one `digest` event each. `key` is a dotted
    /// enum identifier (`memory.search`) — never free text.
    pub fn bump(&self, key: &str) {
        if !self.inner.enabled || self.inner.installation_id.is_empty() {
            return;
        }
        self.inner.digest.bump(key);
        self.ship_digests();
    }

    /// Count an error bucket: `error.<stage>.<code>`. `code` is normalized to
    /// the enum charset defensively so a dynamically built code can never
    /// smuggle content into a count key.
    pub fn error(&self, stage: &str, code: &str) {
        let code: String = code
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(32)
            .collect();
        let code = if code.is_empty() { "other".to_string() } else { code };
        self.bump(&format!("error.{stage}.{code}"));
    }

    /// Ship every completed day as one `digest` event. Days are removed from
    /// the local file before the POST (the double-send guard) and merged back
    /// on failure so the next trigger retries them.
    fn ship_digests(&self) {
        // A one-shot CLI invocation may have no async runtime; counts stay
        // in the local file and the daemon ships them later.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for (day, counts) in self.inner.digest.take_completed() {
            let payload = serde_json::json!({ "day": &day, "counts": &counts });
            let body = serde_json::json!({
                "installation_id": self.inner.installation_id,
                "event_type": "digest",
                "app_version": APP_VERSION,
                "platform": platform_name(),
                "product": self.inner.product,
                "payload": payload,
            });
            let client = self.inner.client.clone();
            let this = self.clone();
            handle.spawn(async move {
                let ok = matches!(
                    client.post(TRACK_URL).json(&body).send().await,
                    Ok(resp) if resp.status().is_success()
                );
                if !ok {
                    this.inner.digest.restore(&day, counts);
                }
            });
        }
    }

    /// Record a Memory.* call. Non-blocking. `verb` is the bare method
    /// name ("search", "add", "memory.search" — caller's choice; the
    /// server stores it verbatim in payload.verb).
    ///
    /// Daily-deduped: fires at most once per UTC day per installation. DAU
    /// is the only thing we care about right now, so flooding D1 with one
    /// row per Memory.* call is pure overhead. The first command of the
    /// day fires; subsequent commands return early without touching the
    /// network. Server-side DAU query is unchanged.
    pub fn command(&self, verb: &str) {
        if !self.inner.enabled || self.inner.installation_id.is_empty() {
            return;
        }
        let state_path = state_path(&self.inner.data_dir, self.inner.product);
        let mut state = load_state(&state_path).unwrap_or_default();
        let today = today_utc_day();
        if state.last_command_day == today {
            return;
        }
        state.last_command_day = today;
        let _ = save_state(&state_path, &state);

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
    /// UTC day-since-epoch of the most recent `command` event. Zero = never
    /// fired. Compared against `today_utc_day()` to gate per-day dedup.
    #[serde(default)]
    last_command_day: i64,
}

/// Days since 1970-01-01 UTC. Used as the daily-dedup key for the
/// `command` event — avoids pulling in chrono just for date comparison.
fn today_utc_day() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0)
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


