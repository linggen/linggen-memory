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
        // Outside even the gate: a refused request is still traffic, and
        // background maintenance must not rewrite tables while anything at
        // all is knocking.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            mark_activity_layer,
        ))
        .with_state(state)
}

/// Middleware: stamp "the daemon is being used" on every request, so the
/// background maintenance loop can wait for a genuinely quiet window.
async fn mark_activity_layer(
    axum::extract::State(state): axum::extract::State<SharedState>,
    request: Request,
    next: Next,
) -> Response {
    state.mark_request();
    next.run(request).await
}

/// Middleware: emit a `command` telemetry event for every `/api/memory/*`
/// request. The verb is parsed from the URI path (the segment after
/// `/api/memory/`); requests with unexpected paths are skipped.
async fn command_telemetry_layer(
    axum::extract::State(telemetry): axum::extract::State<Telemetry>,
    request: Request,
    next: Next,
) -> Response {
    // Sanitize to the digest key charset: the segment comes off the URL, and
    // an arbitrary path must never become a count key.
    let verb: Option<String> = request
        .uri()
        .path()
        .strip_prefix("/api/memory/")
        .map(|v| v.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(24).collect())
        .filter(|v: &String| !v.is_empty());

    let response = next.run(request).await;

    if let Some(verb) = verb {
        telemetry.command(&format!("memory.{verb}"));
        // Digest counts split by outcome; the daily `command` row above keeps
        // the DAU contract unchanged.
        let status = response.status();
        if status.is_success() {
            telemetry.bump(&format!("memory.{verb}"));
        } else {
            let code = match status.as_u16() {
                401 | 403 => "auth_required",
                429 => "quota",
                s if s >= 500 => "server",
                _ => "request",
            };
            telemetry.error("memory", code);
        }
    }
    response
}
