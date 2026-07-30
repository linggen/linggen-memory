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

mod chains;
mod config;
pub(crate) mod days;
pub mod envelope;
pub mod gate;
mod health;
pub(crate) mod issues;
mod mcp;
mod memory;
pub mod state;
mod stats;
mod ui;

use crate::telemetry::Telemetry;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use state::SharedState;

/// Compose the full router for the daemon.
///
/// `state` carries the shared MemoryStore and Embedder — opened once at
/// daemon startup and reused across all requests. `telemetry` is the
/// anonymous-usage telemetry handle (no-op when opted out / feature
/// disabled); it's wrapped around the Memory subrouter so every Memory.*
/// call records a `command` event.
pub fn build_router(state: SharedState, telemetry: Telemetry) -> Router {
    Router::new()
        .route("/api/health", get(health::handler))
        .merge(
            memory::router()
                .merge(days::router())
                .merge(chains::router())
                .merge(issues::router())
                .merge(stats::router())
                .layer(middleware::from_fn_with_state(
                    telemetry,
                    command_telemetry_layer,
                )),
        )
        .merge(mcp::router())
        .merge(config::router())
        .merge(ui::router())
        // Outermost, so it runs before any handler: a caller off this machine
        // must be a paired device. Loopback — the CLI, the engine, the local
        // app — goes straight through, and `/api/health` stays open for probes.
        // See `gate.rs`; the daemon also refuses to bind wide in the first
        // place unless this machine has paired devices.
        .layer(middleware::from_fn_with_state(state.clone(), gate::lan_gate))
        .with_state(state)
}

/// Middleware: emit a `command` telemetry event for every `/api/memory/*`
/// request. The verb is parsed from the URI path (the segment after
/// `/api/memory/`); requests with unexpected paths are skipped.
async fn command_telemetry_layer(
    axum::extract::State(telemetry): axum::extract::State<Telemetry>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(verb) = request.uri().path().strip_prefix("/api/memory/") {
        if !verb.is_empty() {
            telemetry.command(&format!("memory.{verb}"));
        }
    }
    next.run(request).await
}
