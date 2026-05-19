//! Shared daemon state passed to every handler via `axum::State`.
//!
//! Opened once at daemon startup — the MemoryStore holds a LanceDB
//! connection and the Embedder caches the ONNX model in memory, so
//! reusing them across requests avoids per-call setup cost.

use crate::embed::Embedder;
use crate::memory::MemoryStore;
use std::sync::Arc;

pub struct AppState {
    pub store: Arc<MemoryStore>,
    pub embedder: Arc<Embedder>,
}

pub type SharedState = Arc<AppState>;
