//! Shared daemon state passed to every handler via `axum::State`.
//!
//! Opened once at daemon startup — the stores hold LanceDB connections and
//! the Embedder caches the ONNX model in memory, so reusing them across
//! requests avoids per-call setup cost.

use crate::embed::Embedder;
use crate::memory::{MemoryStore, Recall};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct AppState {
    /// Curated `semantic` store — the default-table write/CRUD/browse path.
    pub store: Arc<MemoryStore>,
    /// Staging `episodic` store — the encoder + dream worklist target.
    /// Selected over `store` when an `add`/`delete` request sets
    /// `episodic: true`.
    pub episodic: Arc<MemoryStore>,
    /// Dual-table read path (`semantic` + `episodic`) for recall/search.
    /// Shares both handles above.
    pub recall: Arc<Recall>,
    pub embedder: Arc<Embedder>,
    /// Data directory root (typically `~/.linggen/`). User-tunable knobs
    /// (`.config.json` — episodic TTL etc.) live under
    /// `<data_dir>/memory/.config.json`.
    pub data_dir: PathBuf,
    /// Bound TCP port of this daemon's HTTP server. The MCP handler uses
    /// it to loopback POST against `/api/memory/<verb>` so MCP `tools/call`
    /// reuses the same handlers (and their dispatch fixes) instead of
    /// duplicating logic.
    pub port: u16,

    /// When the daemon was started, and when it last served a request —
    /// milliseconds since `started`. Background storage maintenance waits
    /// for a quiet window before rewriting tables, because compaction
    /// holds a table's write lock and would otherwise stall a live recall.
    ///
    /// An `Instant` baseline plus a relative counter, rather than wall
    /// clock: a clock step (NTP, sleep/wake, timezone) must not make the
    /// daemon believe it has been idle for hours.
    started: Instant,
    last_request_ms: AtomicU64,
}

impl AppState {
    /// Build the shared state. `started` is stamped here so "idle since"
    /// is measured from daemon launch, not from the first request.
    pub fn new(
        store: Arc<MemoryStore>,
        episodic: Arc<MemoryStore>,
        recall: Arc<Recall>,
        embedder: Arc<Embedder>,
        data_dir: PathBuf,
        port: u16,
    ) -> Self {
        Self {
            store,
            episodic,
            recall,
            embedder,
            data_dir,
            port,
            started: Instant::now(),
            last_request_ms: AtomicU64::new(0),
        }
    }

    /// Record that a request just arrived. Called from middleware on every
    /// route; `Relaxed` is right because the only reader is a background
    /// timer that tolerates being a moment stale.
    pub fn mark_request(&self) {
        self.last_request_ms
            .store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// How long since the last request (or since startup, if none).
    pub fn idle_for(&self) -> Duration {
        let last = self.last_request_ms.load(Ordering::Relaxed);
        self.started
            .elapsed()
            .saturating_sub(Duration::from_millis(last))
    }
}

pub type SharedState = Arc<AppState>;
