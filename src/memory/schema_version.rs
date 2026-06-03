//! Store schema versioning + open-time compatibility guard.
//!
//! See `doc/schema-versioning.md`. The store carries a monotonic
//! [`STORE_SCHEMA_VERSION`] — **decoupled from the binary semver** — recorded
//! in a sidecar file beside the LanceDB dir:
//!
//! ```text
//! <data_dir>/memory/
//! ├── memory.lancedb/      # the store
//! └── SCHEMA_VERSION       # sidecar: a single integer, e.g. "1\n"
//! ```
//!
//! Sidecar (not a LanceDB row) because it is readable *before* opening
//! LanceDB — the open is the step the guard protects — and it does not depend
//! on LanceDB internals.
//!
//! The guard makes binary upgrades data-safe in both directions so the
//! plugin/skill can range-pin (`^1`) the binary instead of an exact version:
//! a newer binary migrates an older store; an older binary refuses a newer
//! store rather than writing old-shape rows into it (the multi-channel skew
//! that bit us when a stale plugin's binary opened a store a newer one wrote).
//!
//! Fine-grained additive column migrations stay per-table and idempotent in
//! [`super::store`] (`ensure_late_schema_additions`); this module owns only the
//! coarse store-generation gate. `STORE_SCHEMA_VERSION` bumps rarely — only on
//! a real layout change — so patch/minor binaries share a value and coexist
//! without locking each other out.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// On-disk store layout version this binary writes. Bump ONLY on a store
/// layout change (new required column, rename, type/dim change), which is by
/// policy a MAJOR binary release — keeping `^1` auto-update from ever
/// crossing an incompatible store.
pub const STORE_SCHEMA_VERSION: u32 = 1;

/// Oldest on-disk version this binary can open / migrate up from. A store
/// older than this is refused with export→reset→import guidance.
pub const MIN_READABLE_SCHEMA: u32 = 1;

const SIDECAR: &str = "SCHEMA_VERSION";

fn sidecar_path(data_dir: &Path) -> PathBuf {
    data_dir.join("memory").join(SIDECAR)
}

/// Outcome of comparing the on-disk store version to this binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compat {
    /// On-disk == current. Open as-is.
    Current,
    /// On-disk in `[MIN_READABLE, current)`. Run migrations, then stamp current.
    Migrate { from: u32 },
    /// No sidecar — fresh store or a legacy (pre-versioning) store. Stamp
    /// current after a successful open; additive migrations self-correct.
    Adopt,
    /// On-disk < min readable. Refuse: too old to migrate forward.
    TooOld { on_disk: u32 },
    /// On-disk > current. Refuse: written by a newer ling-mem.
    TooNew { on_disk: u32 },
}

/// Read the sidecar version. `None` if absent / unreadable / non-integer.
pub fn read_version(data_dir: &Path) -> Option<u32> {
    let raw = std::fs::read_to_string(sidecar_path(data_dir)).ok()?;
    raw.trim().parse::<u32>().ok()
}

/// Write the sidecar version (creating the parent dir if needed).
pub fn write_version(data_dir: &Path, v: u32) -> Result<()> {
    let p = sidecar_path(data_dir);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&p, format!("{v}\n"))
        .with_context(|| format!("writing schema-version sidecar at {}", p.display()))
}

/// Stamp the sidecar to [`STORE_SCHEMA_VERSION`] if it isn't already — called
/// after a successful open so the store records the version that wrote it.
/// Idempotent: writes only when the value differs.
pub fn stamp_current(data_dir: &Path) -> Result<()> {
    if read_version(data_dir) != Some(STORE_SCHEMA_VERSION) {
        write_version(data_dir, STORE_SCHEMA_VERSION)?;
    }
    Ok(())
}

/// Classify the on-disk store against this binary.
pub fn classify(data_dir: &Path) -> Compat {
    match read_version(data_dir) {
        None => Compat::Adopt,
        Some(v) if v == STORE_SCHEMA_VERSION => Compat::Current,
        Some(v) if v > STORE_SCHEMA_VERSION => Compat::TooNew { on_disk: v },
        Some(v) if v < MIN_READABLE_SCHEMA => Compat::TooOld { on_disk: v },
        Some(v) => Compat::Migrate { from: v },
    }
}

/// For a refuse case ([`Compat::TooNew`] / [`Compat::TooOld`]), the actionable
/// error message. `None` for openable cases.
pub fn refuse_message(c: Compat, lancedb_dir: &Path) -> Option<String> {
    let dir = lancedb_dir.display();
    match c {
        Compat::TooNew { on_disk } => Some(format!(
            "ling-mem store at {dir} was written by a newer release (store schema v{on_disk}); \
             this binary writes v{STORE_SCHEMA_VERSION}. Upgrade ling-mem (`ling-mem upgrade`) — \
             refusing to open to protect your memory."
        )),
        Compat::TooOld { on_disk } => Some(format!(
            "ling-mem store at {dir} is schema v{on_disk}, older than this build supports \
             (min v{MIN_READABLE_SCHEMA}). Export, reset, then re-import:\n\n  \
             ling-mem export memory.jsonl\n  rm -rf {dir}\n  ling-mem import memory.jsonl"
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn absent_sidecar_is_adopt() {
        let d = tempdir().unwrap();
        assert_eq!(classify(d.path()), Compat::Adopt);
        assert_eq!(read_version(d.path()), None);
    }

    #[test]
    fn roundtrips_and_classifies_current() {
        let d = tempdir().unwrap();
        write_version(d.path(), STORE_SCHEMA_VERSION).unwrap();
        assert_eq!(read_version(d.path()), Some(STORE_SCHEMA_VERSION));
        assert_eq!(classify(d.path()), Compat::Current);
    }

    #[test]
    fn newer_store_is_refused() {
        let d = tempdir().unwrap();
        write_version(d.path(), STORE_SCHEMA_VERSION + 1).unwrap();
        let c = classify(d.path());
        assert!(matches!(c, Compat::TooNew { .. }));
        assert!(refuse_message(c, d.path()).unwrap().contains("upgrade ling-mem")
            || refuse_message(c, d.path()).unwrap().contains("ling-mem upgrade"));
    }

    #[test]
    fn stamp_is_idempotent() {
        let d = tempdir().unwrap();
        stamp_current(d.path()).unwrap();
        assert_eq!(read_version(d.path()), Some(STORE_SCHEMA_VERSION));
        // second call is a no-op (no panic, value unchanged)
        stamp_current(d.path()).unwrap();
        assert_eq!(read_version(d.path()), Some(STORE_SCHEMA_VERSION));
    }

    #[test]
    fn garbage_sidecar_reads_as_none() {
        let d = tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("memory")).unwrap();
        std::fs::write(d.path().join("memory").join(SIDECAR), "not-a-number\n").unwrap();
        assert_eq!(read_version(d.path()), None);
        assert_eq!(classify(d.path()), Compat::Adopt);
    }
}
