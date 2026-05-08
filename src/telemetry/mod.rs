//! Anonymous usage telemetry for `ling-mem`.
//!
//! ## What we send
//!
//! POST https://linggen.dev/api/track  (Pages Function — see linggensite repo)
//!
//! On every daemon start:
//! - `install` event the first time the binary runs on this machine
//!   (installation_id newly created; payload.via from the install marker).
//! - `install` event whenever the version changes from the last recorded one
//!   (payload.via = "upgrade", payload.from_version, payload.to_version).
//! - `heartbeat` event at most once per UTC day (rate-limited locally).
//!
//! On every Memory.* HTTP call:
//! - `command` event with payload.verb = "memory.search" / "memory.add" / etc.
//!
//! ## What we never send
//!
//! Path content, fact content, query text, embeddings, IPs (CF strips and we
//! don't store), any user-identifying string. The installation_id is a
//! random v4 UUID stored at `~/.linggen/installation_id`.
//!
//! ## Disabling telemetry
//!
//! Runtime:
//!   - env: `LING_MEM_NO_TELEMETRY=1`
//!   - file: `touch ~/.linggen/no-telemetry`
//! Compile time:
//!   - `cargo build --no-default-features`
//!
//! ## OSS audit
//!
//! Every field sent is listed above. The receiver is also open source —
//! see `linggensite/functions/api/_lib/analytics.ts`. No third-party
//! analytics are involved.

#[cfg(feature = "telemetry")]
mod imp;

#[cfg(feature = "telemetry")]
pub use imp::Telemetry;

/// No-op stub used when the `telemetry` feature is disabled at compile time.
/// Keeps call sites unchanged. `Clone` is required because the daemon clones
/// the handle into axum middleware state.
#[cfg(not(feature = "telemetry"))]
#[derive(Clone)]
pub struct Telemetry;

#[cfg(not(feature = "telemetry"))]
impl Telemetry {
    pub fn new(_product: &'static str, _data_dir: &std::path::Path) -> Self {
        Self
    }
    pub fn launch(&self) {}
    pub fn command(&self, _verb: &str) {}
}
