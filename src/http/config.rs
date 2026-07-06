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
    /// Short-term retention length: how long an episodic row lives
    /// before the forget sweep may evict it — and only on days a
    /// remember pass has already judged (`remembered_at` stamped).
    /// This is the human-facing knob the console / dream skill reads.
    #[serde(default = "default_episodic_ttl_days")]
    pub episodic_ttl_days: u32,

    /// Store-wide cosine floor for `search`. Applied server-side when a
    /// search request omits `min_score`, so every host (engine auto-recall,
    /// the CC/Codex recall hook, the CLI) shares one recall selectivity
    /// instead of each hardcoding its own. An explicit per-call `min_score`
    /// still overrides. Same role as `episodic_ttl_days`: a store property
    /// the daemon owns and clients defer to.
    #[serde(default = "default_recall_min_score")]
    pub recall_min_score: f32,
}

fn default_episodic_ttl_days() -> u32 {
    7
}

fn default_recall_min_score() -> f32 {
    0.6
}

impl Default for Config {
    fn default() -> Self {
        Self {
            episodic_ttl_days: default_episodic_ttl_days(),
            recall_min_score: default_recall_min_score(),
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
pub(crate) async fn load(data_dir: &std::path::Path) -> Config {
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
    if !(0.0..=1.0).contains(&cfg.recall_min_score) {
        return Err(ApiError::bad_request(
            "recall_min_score must be between 0.0 and 1.0",
        ));
    }
    save(&state.data_dir, &cfg)
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!(e)))?;
    Ok(ok(serde_json::to_value(cfg).expect("Config serializes")))
}
