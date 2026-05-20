//! Shared daemon state passed to every handler via `axum::State`.
//!
//! Opened once at daemon startup — the stores hold LanceDB connections and
//! the Embedder caches the ONNX model in memory, so reusing them across
//! requests avoids per-call setup cost.

use crate::embed::Embedder;
use crate::memory::{MemoryStore, Recall};
use std::sync::Arc;

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
}

pub type SharedState = Arc<AppState>;
