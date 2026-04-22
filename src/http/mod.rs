//! HTTP surface served by the daemon.
//!
//! Phase 3 ships the seven `Memory.*` methods at `/api/memory/<method>`
//! plus the lifecycle probe at `/api/health`. All endpoints return the
//! standard `{ok, data}` envelope (see `envelope.rs`).
//!
//! Per the spec (see `../../linggen/doc/memory-spec.md`), tool name maps
//! directly to endpoint path — `Memory.search` → `POST /api/memory/search`.
//! Linggen's dispatcher translates nothing; JSON args pass through.
//!
//! The same daemon also serves the Data Browser UI: `GET /` returns the
//! bundled `index.html` and `GET /assets/*` fans out to the rest of
//! `static/`. See `doc/ui-spec.md`.

pub mod envelope;
mod health;
mod memory;
pub mod state;
mod ui;

use axum::routing::get;
use axum::Router;
use state::SharedState;

/// Compose the full router for the daemon.
///
/// `state` carries the shared FactsStore and Embedder — opened once at
/// daemon startup and reused across all requests.
pub fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/api/health", get(health::handler))
        .merge(memory::router())
        .merge(ui::router())
        .with_state(state)
}
