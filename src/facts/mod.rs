//! # facts — memory-fact types and storage
//!
//! v0.1 memory-store primitives for `ling-mem`. See `doc/tech-spec.md` for the
//! locked schema and CLI contract.
//!
//! This module is deliberately thin: a [`Fact`] struct, three validated enums
//! ([`FactType`], [`Outcome`], [`Origin`]), and the Arrow/LanceDB plumbing
//! that stores and retrieves them. CLI parsing, embedding, and extraction
//! pipelines live in sibling modules.

pub mod schema;
pub mod store;
pub mod types;

pub use schema::{
    build_schema, facts_to_record_batch, record_batch_to_facts, TABLE_NAME, VECTOR_DIM,
};
pub use store::{FactPatch, FactsStore, Filters, SortOrder};
pub use types::{Fact, FactType, Origin, Outcome, ParseEnumError};
