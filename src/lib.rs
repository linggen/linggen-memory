//! # ling-mem — library interface
//!
//! Semantic memory store backing the `ling-mem` binary. See `doc/tech-spec.md`
//! for the v0.1 schema and CLI contract.
//!
//! The public surface of the library is kept small — the binary and tests are
//! the intended consumers; external Rust users should prefer invoking the CLI.

pub mod cli;
pub mod daemon;
pub mod embed;
pub mod memory;
pub mod http;
pub mod telemetry;
pub mod update;

pub use memory::{Memory, MemoryType, Origin, Outcome};
