//! Anonymous usage telemetry for `ling-mem`.
//!
//! ## What we send
//!
//! POST https://linggen.dev/api/track  (Pages Function — see linggensite repo)
//!
//! On daemon start:
//! - `install` event the first time the binary runs on this machine
//!   (installation_id newly created; payload.via from the install marker).
//! - `install` event whenever the version changes from the last recorded one
//!   (payload.via = "upgrade", payload.from_version, payload.to_version).
//!
//! `payload.via` names the distribution channel and nothing more, read from
//! `~/.linggen/.ling-mem-install-source` — written by install-bin.sh (the
//! canonical installer every channel funnels through). A label is only set
//! by a caller that genuinely IS that channel (a hook or app declaring
//! itself) or that measures it (the skill's scripts/bootstrap.sh derives
//! the channel from its own on-disk path at run time): `plugin` (Claude
//! Code / Codex session-start hook), `website` (linggen.dev's install.sh),
//! `clawhub` / `skills-sh` (bootstrap path fingerprint), `linggen` (the
//! engine's first-use auto-install), `vscode-extension`, `install-bin`
//! (no label — direct invocations), `upgrade` (version change, not a
//! channel), `unknown` (no marker). `agent` (optional) names the host that
//! ran the install: `cc`, `codex`, `openclaw`, `linggen`, `vscode` —
//! observed or omitted, never guessed. Other marker keys are forwarded
//! verbatim (`installer_version`, `installed_at`). Full design:
//! linggensite/doc/analytics-spec.md.
//!
//! On every Memory.* HTTP call:
//! - `command` event with payload.verb = "memory.search" / "memory.add" / etc.
//!
//! No dedicated heartbeat — DAU is derived server-side from any event row
//! (`COUNT(DISTINCT installation_id) WHERE date(created_at) = today`). An
//! active user always produces at least one `command` event per day; idle
//! users wouldn't be retained anyway, so not counting them is the honest
//! definition of "active".
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
mod digest;
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
    pub fn bump(&self, _key: &str) {}
    pub fn error(&self, _stage: &str, _code: &str) {}
}
