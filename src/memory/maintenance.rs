//! Physical store maintenance — compaction and version pruning.
//!
//! This is the **physical** lane, and it is deliberately walled off from the
//! semantic one (dream / condense / solve). It never reads content, never
//! judges a row, and never changes what memory *says*: row count and row
//! bytes are invariant across a run. All it touches is how those rows are
//! laid out on disk.
//!
//! ## Why it has to exist
//!
//! Lance is append-only by design — "every change is additive", and the
//! read path is a readonly filesystem so concurrent readers never lock.
//! There is no journal to switch off: the per-version *manifest* is the
//! atomic-commit and snapshot-isolation mechanism itself. The cost is that
//! every write commits a fresh manifest, and a manifest lists **every**
//! fragment in the table. So the disk cost of write N is proportional to
//! the fragment count at N — quadratic in total writes.
//!
//! Measured on a real store before this existed: 1052 semantic rows in
//! 17.4 MB of fragments, carrying 120 MB of manifests across 1146 versions
//! (~105 KB each). 82% of the table was bookkeeping about bookkeeping.
//!
//! LanceDB Enterprise runs this automatically; in OSS "compaction and
//! cleanup are manual — run `table.optimize()` regularly". Inline
//! maintenance is only a proposal upstream (lance#6005), and a Lance
//! maintainer pushed back there on doing this work inside the commit path.
//! So we schedule it ourselves, out of band.
//!
//! ## Where the policy lives
//!
//! Exactly here. [`Footprint::maintenance_due`] is the single decision
//! point; the daemon loop and any future caller both ask it rather than
//! re-deriving thresholds, so there is nothing to drift.

use std::path::{Path, PathBuf};

/// What one table costs, and how badly it wants maintaining.
///
/// Sizes come from walking the table directory — "how much disk is this
/// costing" is a filesystem fact, and cheap to answer: a few thousand
/// `stat` calls. Row and fragment counts come from Lance, because those
/// are questions about the *current version*, and the directory keeps
/// answering them wrong for days after a compaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Footprint {
    /// Live rows in the table.
    pub rows: u64,
    /// Fragments the current version references — from Lance, **not** a
    /// count of files in `data/`. Compaction unlinks merged fragments long
    /// before their files can be deleted (Lance keeps anything younger
    /// than 7 days unless deletion is forced), so the file count keeps
    /// reporting the pre-compaction number for days. Deciding on that
    /// would re-run compaction hourly against an already-clean table.
    pub fragments: u64,
    /// Of those, how many Lance itself considers uncompacted.
    pub small_fragments: u64,
    /// Committed version manifests under `_versions/`.
    pub versions: u64,
    /// Bytes of actual row data, including fragments already unlinked but
    /// not yet reclaimable.
    pub data_bytes: u64,
    /// Bytes of version manifests + transaction records — the part that
    /// pruning reclaims.
    pub version_bytes: u64,
}

/// How many uncompacted fragments justify a rewrite. Below this the pass
/// would cost more I/O than the scan it saves — a handful of small files
/// is a handful of manifest entries.
const MIN_SMALL_FRAGMENTS: u64 = 64;

/// Prune once version metadata outweighs live data by this factor. A
/// compacted store sits well under 1.0.
const MAX_VERSION_RATIO: f64 = 1.0;

/// ...but only when there is something meaningful to reclaim, so a small
/// fresh store is never rewritten to win back a few hundred KB.
const MIN_VERSION_BYTES: u64 = 32 * 1024 * 1024;

/// How long a version must survive before pruning may remove it.
///
/// Well above the "never shorter than typical write latency" floor
/// (~10 min) and well under Lance's own 7-day default. We can afford the
/// short end because version history is **not** our recovery story —
/// `ling-mem export` is, and nothing in this codebase ever checks out a
/// historical version.
pub const PRUNE_OLDER_THAN_DAYS: i64 = 2;

impl Footprint {
    /// Whether this table is worth maintaining right now.
    ///
    /// Two independent triggers, because they catch different diseases:
    /// fragmentation slows *reads* (every scan opens every file) while
    /// manifest bloat costs *disk*. Either alone justifies a pass.
    pub fn maintenance_due(&self) -> bool {
        self.fragmented() || self.version_heavy()
    }

    /// Enough uncompacted fragments that scans are paying for it.
    fn fragmented(&self) -> bool {
        self.small_fragments >= MIN_SMALL_FRAGMENTS
    }

    /// Manifests outweigh the data they describe.
    fn version_heavy(&self) -> bool {
        self.version_bytes > MIN_VERSION_BYTES
            && self.version_bytes as f64 > self.data_bytes as f64 * MAX_VERSION_RATIO
    }

    /// Total bytes this table occupies.
    pub fn total_bytes(&self) -> u64 {
        self.data_bytes + self.version_bytes
    }
}

/// The LanceDB directory holding both tables.
pub fn db_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("memory").join("memory.lancedb")
}

/// Measure `<db>/<table>.lance`.
///
/// Two sources, on purpose. Sizes come from walking the directory, because
/// "how much disk is this costing" is a filesystem fact. Row and fragment
/// counts come from Lance via the caller, because those are questions
/// about the *current version* — the directory still holds fragments that
/// compaction unlinked and pruning cannot yet reclaim.
///
/// A missing table reads as an empty footprint rather than an error: a
/// fresh install has nothing to maintain, which is the correct answer.
pub fn measure(
    data_dir: &Path,
    table_name: &str,
    rows: u64,
    fragments: u64,
    small_fragments: u64,
) -> Footprint {
    let root = db_dir(data_dir).join(format!("{table_name}.lance"));
    let (_, data_bytes) = count_and_size(&root.join("data"));
    let (versions, manifest_bytes) = count_and_size(&root.join("_versions"));
    let (_, txn_bytes) = count_and_size(&root.join("_transactions"));

    Footprint {
        rows,
        fragments,
        small_fragments,
        versions,
        data_bytes,
        // Transaction records are pruned alongside manifests and serve the
        // same purpose, so they belong on the same side of the ledger.
        version_bytes: manifest_bytes + txn_bytes,
    }
}

/// `(file count, total bytes)` for one directory, non-recursive — Lance
/// keeps these flat. Missing directory reads as zero.
fn count_and_size(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .fold((0, 0), |(n, bytes), m| (n + 1, bytes + m.len()))
}

/// What a completed pass did, for the log line.
#[derive(Debug, Clone, Copy)]
pub struct Report {
    pub before: Footprint,
    pub after: Footprint,
}

impl Report {
    /// Bytes given back. Saturating because compaction can legitimately
    /// leave a table *larger* — new merged fragments are written before
    /// the versions referencing the old ones age out, so a run whose
    /// prune window has not yet caught up nets negative.
    pub fn reclaimed_bytes(&self) -> u64 {
        self.before
            .total_bytes()
            .saturating_sub(self.after.total_bytes())
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fragments {}→{} · versions {}→{} · {} MB → {} MB (reclaimed {} MB)",
            self.before.fragments,
            self.after.fragments,
            self.before.versions,
            self.after.versions,
            self.before.total_bytes() / 1_048_576,
            self.after.total_bytes() / 1_048_576,
            self.reclaimed_bytes() / 1_048_576,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `small` is what Lance considers uncompacted — the compaction lever.
    fn fp(rows: u64, fragments: u64, small: u64, data_mb: u64, version_mb: u64) -> Footprint {
        Footprint {
            rows,
            fragments,
            small_fragments: small,
            versions: fragments,
            data_bytes: data_mb * 1_048_576,
            version_bytes: version_mb * 1_048_576,
        }
    }

    #[test]
    fn healthy_store_is_not_due() {
        // Few large fragments, none small, manifests a fraction of the data.
        assert!(!fp(100_000, 12, 0, 400, 4).maintenance_due());
    }

    #[test]
    fn small_fresh_store_is_never_due() {
        // Under the fragment floor and nothing worth reclaiming, even
        // though every fragment holds a single row.
        assert!(!fp(20, 20, 20, 1, 1).maintenance_due());
    }

    #[test]
    fn one_commit_per_row_is_due() {
        // The real pathology, measured on a live store: 1065 rows across
        // 1956 single-row fragments carrying 117 MB of manifests.
        assert!(fp(1065, 1956, 1956, 17, 117).maintenance_due());
    }

    #[test]
    fn fragmentation_alone_is_due() {
        // Read cost, even with trivial manifest bytes.
        assert!(fp(64, 512, 512, 1, 0).maintenance_due());
    }

    #[test]
    fn manifest_bloat_alone_is_due() {
        // Disk cost, even with a fully compacted fragment count.
        assert!(fp(100_000, 8, 0, 40, 64).maintenance_due());
    }

    /// Regression: a freshly compacted table must read as *not* due.
    ///
    /// The first build decided fragmentation from files in `data/`, and a
    /// real run exposed why that is wrong — compaction merged 1956
    /// fragments into one, but the unlinked files stayed on disk (Lance
    /// will not delete anything younger than 7 days it cannot verify), so
    /// the walk still counted 1957 and the trigger stayed true forever.
    /// Deciding on Lance's own live counts is what fixes it.
    #[test]
    fn compacted_table_is_not_due_while_orphan_files_remain() {
        // One live fragment, none small; disk still carries the merged-away
        // files and the manifests that have not yet aged out.
        assert!(!fp(1065, 1, 0, 22, 8).maintenance_due());
    }

    #[test]
    fn missing_table_measures_empty() {
        let dir = tempfile::tempdir().unwrap();
        let got = measure(dir.path(), "semantic", 0, 0, 0);
        assert_eq!(got, Footprint::default());
        assert!(!got.maintenance_due());
    }

    #[test]
    fn measure_counts_files_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = db_dir(dir.path()).join("semantic.lance");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("_versions")).unwrap();
        std::fs::write(root.join("data/a.lance"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("data/b.lance"), vec![0u8; 200]).unwrap();
        std::fs::write(root.join("_versions/1.manifest"), vec![0u8; 50]).unwrap();

        let got = measure(dir.path(), "semantic", 7, 2, 2);
        assert_eq!(got.rows, 7);
        // Fragment counts come from Lance, not the directory walk.
        assert_eq!(got.fragments, 2);
        assert_eq!(got.small_fragments, 2);
        assert_eq!(got.data_bytes, 300);
        assert_eq!(got.versions, 1);
        assert_eq!(got.version_bytes, 50);
    }

    #[test]
    fn reclaimed_never_underflows_when_compaction_grows_the_table() {
        let report = Report {
            before: fp(10, 10, 10, 1, 1),
            after: fp(10, 1, 0, 2, 2),
        };
        assert_eq!(report.reclaimed_bytes(), 0);
    }
}
