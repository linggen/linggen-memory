//! Daemon lifecycle — the `ling-mem` process that hosts the HTTP API.
//!
//! The daemon is the only dispatch path for `Memory.*` tools in Linggen v2
//! (see `../../linggen/doc/memory-spec.md`). Lifecycle is managed via
//! CLI subcommands:
//!
//! | Command | Role |
//! |:--------|:-----|
//! | `ling-mem serve`   | foreground; what `start` re-exec's, what launchd/systemd runs |
//! | `ling-mem start`   | background; spawns `serve`, waits for pidfile, returns |
//! | `ling-mem stop`    | SIGTERM to the pid in the pidfile, removes file on exit |
//! | `ling-mem restart` | stop + start |
//! | `ling-mem status`  | pidfile + liveness + health probe |
//!
//! Discovery: the daemon writes `daemon.json` under
//! `<data_dir>/memory/linggen-memory/` on bind. Callers (other Linggen
//! processes, `status`, `stop`) read it to find the running port and pid.

pub mod lifecycle;
pub mod pidfile;
pub mod serve;

use std::path::{Path, PathBuf};

/// Per-skill data root: `<data_dir>/memory/linggen-memory/`.
///
/// Holds `daemon.json` plus (Phase 3+) `data/`, `logs/`, `config.toml`.
pub fn skill_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("memory").join("linggen-memory")
}

/// Default bind port. Overridable via `--port` on any lifecycle command.
pub const DEFAULT_PORT: u16 = 9528;
