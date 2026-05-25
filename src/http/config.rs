//! User-tunable daemon config — small JSON file at
//! `<data_dir>/memory/.config.json`. The console + the dream pass both
//! read through `GET /api/config`; the console writes via `PUT`.
//!
//! Kept minimal on purpose. Schema lives here, not in a separate spec —
//! add a field, version-bump the default, ship.

use axum::{
    extract::State,
    response::Response,
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::http::envelope::{ok, ApiError};
use crate::http::state::SharedState;

/// User-tunable knobs persisted to disk. Add a field with a sensible
/// default and the existing config files round-trip cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// How long an episodic row survives before the dream consolidator
    /// is allowed to terminally promote-or-delete it. The CLI's
    /// `evict --before <ts>` takes the resolved instant; this is the
    /// human-facing knob the console / dream skill reads.
    #[serde(default = "default_episodic_ttl_days")]
    pub episodic_ttl_days: u32,
}

fn default_episodic_ttl_days() -> u32 {
    7
}

impl Default for Config {
    fn default() -> Self {
        Self {
            episodic_ttl_days: default_episodic_ttl_days(),
        }
    }
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/config", get(get_config))
        .route("/api/config", put(put_config))
}

fn config_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("memory").join(".config.json")
}

/// Read-or-default. Never errors on missing/corrupt — the daemon
/// always has a working config even on first run.
async fn load(data_dir: &std::path::Path) -> Config {
    let path = config_path(data_dir);
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return Config::default();
    };
    serde_json::from_slice::<Config>(&bytes).unwrap_or_default()
}

async fn save(data_dir: &std::path::Path, cfg: &Config) -> Result<(), std::io::Error> {
    let path = config_path(data_dir);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Atomic-ish: write to .tmp + rename. Console writes are rare; no
    // need for a heavier lock.
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(cfg).expect("Config serializes");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(tmp, path).await?;
    Ok(())
}

async fn get_config(State(state): State<SharedState>) -> Result<Response, ApiError> {
    let cfg = load(&state.data_dir).await;
    Ok(ok(serde_json::to_value(cfg).expect("Config serializes")))
}

async fn put_config(
    State(state): State<SharedState>,
    Json(cfg): Json<Config>,
) -> Result<Response, ApiError> {
    // Light range check — the dream pass treats a 0 TTL as "evict
    // everything," which is rarely what the user wants. Allow it but
    // pin the lower bound; cap is generous to keep this future-proof.
    if cfg.episodic_ttl_days > 3650 {
        return Err(ApiError::bad_request(
            "episodic_ttl_days must be <= 3650 (10 years)",
        ));
    }
    save(&state.data_dir, &cfg)
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!(e)))?;
    Ok(ok(serde_json::to_value(cfg).expect("Config serializes")))
}
