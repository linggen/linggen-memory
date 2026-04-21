//! Ingestion — text extraction and lightweight file watching.
//!
//! The old code-indexing ingestion pipeline (git, local, web, full walker,
//! IngestionService) has been removed. What remains:
//!
//! - `extract_text`: pull plain text out of PDF, DOCX, and other formats
//!   uploaded by the user via the API `upload` endpoint.
//! - `FileWatcher`: thin wrapper around `notify` used by the API's internal
//!   indexer to react to on-disk edits of the user's memory markdown files.
//!
//! Everything here should be reusable by any semantic-memory consumer; none
//! of it should assume a project or a codebase.

use anyhow::Result;
use async_trait::async_trait;
use linggen_core::Document;

pub mod watcher;
pub use watcher::FileWatcher;

pub mod extract;
pub use extract::extract_text;

#[async_trait]
pub trait Ingestor: Send + Sync {
    /// Ingests documents from the source.
    async fn ingest(&self) -> Result<Vec<Document>>;
}
