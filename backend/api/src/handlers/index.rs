use crate::job_manager::JobManager;
use crate::memory::MemoryStore;
use dashmap::DashMap;
use embeddings::{EmbeddingModel, TextChunker};
use std::sync::Arc;
use storage::{InternalIndexStore, MetadataStore, VectorStore};

pub struct AppState {
    pub embedding_model: Arc<tokio::sync::RwLock<Option<EmbeddingModel>>>,
    pub chunker: Arc<TextChunker>,
    pub vector_store: Arc<VectorStore>,
    pub internal_index_store: Arc<InternalIndexStore>,
    pub metadata_store: Arc<MetadataStore>,
    pub memory_store: Arc<MemoryStore>,
    pub cancellation_flags: DashMap<String, bool>, // job_id -> is_cancelled
    pub job_manager: Arc<JobManager>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    pub library_path: std::path::PathBuf,
}
