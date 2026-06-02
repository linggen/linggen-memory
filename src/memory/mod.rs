//! # memory — memory types and storage
//!
//! v0.1 memory-store primitives for `ling-mem`. See `doc/tech-spec.md` for the
//! locked schema and CLI contract.
//!
//! This module is deliberately thin: a [`Memory`] struct, three validated enums
//! ([`MemoryType`], [`Outcome`], [`Origin`]), and the Arrow/LanceDB plumbing
//! that stores and retrieves them. CLI parsing, embedding, and extraction
//! pipelines live in sibling modules.

pub mod recall;
pub mod schema;
pub mod schema_version;
pub mod store;
pub mod types;

pub use schema::{
    build_schema, memories_to_record_batch, record_batch_to_memories, SEMANTIC_TABLE_NAME, VECTOR_DIM,
};
pub use store::{
    MemoryPatch, MemoryStore, Filters, InsertOutcome, SortOrder, DEDUP_SIMILARITY_THRESHOLD,
};
pub use recall::Recall;
pub use types::{Memory, MemoryType, Origin, Outcome, ParseEnumError, Tier};
