//! Two-table recall: query the curated `semantic` store and the staging
//! `episodic` store together, merge, and dedup.
//!
//! Locked design (memory-recall-redesign, fork 1 — 2026-05-19): recall is
//! irreducibly dual-table while the write path is irreducibly single-table,
//! so the merge policy lives in its own type rather than scattered across
//! call sites or smuggled into [`MemoryStore`]. Later steps (write-time
//! quality, conflict-at-recall) extend this one site.
//!
//! Merge contract:
//! - Query both tables concurrently, each for `limit` hits (over-fetch the
//!   union, then truncate).
//! - NO read-time re-rank: the union is ordered by the cosine score each
//!   row already carries. Same embedder for both tables → scores are
//!   directly comparable; sorting the union is ordering, not re-scoring.
//! - Cross-table + episodic-internal dedup: a hit that is a near-duplicate
//!   (cosine ≥ [`DEDUP_SIMILARITY_THRESHOLD`]) of an already-accepted hit
//!   is dropped. Semantic is processed first, so the curated copy always
//!   wins over its un-promoted episodic twin (the consolidator promotes
//!   with a fresh id, so id-equality cannot catch this — content vectors
//!   can).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::memory::store::cosine_similarity;
use crate::memory::{Filters, Memory, MemoryStore, DEDUP_SIMILARITY_THRESHOLD};

/// Dual-table read path. Holds shared handles to the `semantic` and
/// `episodic` stores and owns the merge/dedup policy.
pub struct Recall {
    semantic: Arc<MemoryStore>,
    episodic: Arc<MemoryStore>,
}

impl Recall {
    /// Build from existing store handles. The daemon shares its `semantic`
    /// handle with the CRUD write path (`AppState.store`), so recall and
    /// writes reuse one LanceDB connection for that table.
    pub fn new(semantic: Arc<MemoryStore>, episodic: Arc<MemoryStore>) -> Self {
        Self { semantic, episodic }
    }

    /// Open both tables under `data_dir`. Convenience for the CLI
    /// direct-fallback path and tests; the daemon uses [`Self::new`] to
    /// share the semantic handle with the write path.
    pub async fn open(data_dir: &Path) -> Result<Self> {
        let semantic = MemoryStore::open_semantic(data_dir)
            .await
            .with_context(|| format!("opening semantic store under {}", data_dir.display()))?;
        let episodic = MemoryStore::open_episodic(data_dir)
            .await
            .with_context(|| format!("opening episodic store under {}", data_dir.display()))?;
        Ok(Self::new(Arc::new(semantic), Arc::new(episodic)))
    }

    /// Recall across both tables. Mirrors [`MemoryStore::search_scored`]'s
    /// signature; `min_score` is applied per-table by the underlying call.
    pub async fn query(
        &self,
        query_vec: &[f32],
        filters: &Filters,
        limit: usize,
        min_score: Option<f32>,
    ) -> Result<Vec<(Memory, f32)>> {
        let (semantic, episodic) = tokio::try_join!(
            self.semantic
                .search_scored(query_vec, filters, limit, min_score),
            self.episodic
                .search_scored(query_vec, filters, limit, min_score),
        )?;

        // Semantic first so the curated copy wins any near-dup tie; episodic
        // appended. A candidate is kept only if it is not a near-duplicate of
        // one already accepted — covers cross-table dups *and* episodic's
        // raw, un-deduped internal dups.
        let mut merged: Vec<(Memory, f32)> = Vec::with_capacity(semantic.len() + episodic.len());
        for cand in semantic.into_iter().chain(episodic) {
            if !merged.iter().any(|(kept, _)| is_near_dup(kept, &cand.0)) {
                merged.push(cand);
            }
        }

        // Order the union by the score each row already carries (not a
        // re-rank), then truncate the over-fetched union to `limit`.
        merged.sort_by(|a, b| b.1.total_cmp(&a.1));
        merged.truncate(limit);
        Ok(merged)
    }
}

/// Near-duplicate test for the cross-table merge: cosine of the two content
/// vectors ≥ [`DEDUP_SIMILARITY_THRESHOLD`]. Falls back to exact content
/// equality when either row lacks a vector (search drops null-vector rows,
/// so this is just a totality guard).
fn is_near_dup(a: &Memory, b: &Memory) -> bool {
    match (&a.vector, &b.vector) {
        (Some(va), Some(vb)) => cosine_similarity(va, vb) >= DEDUP_SIMILARITY_THRESHOLD,
        _ => a.content == b.content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{MemoryType, Origin};
    use crate::memory::VECTOR_DIM;
    use tempfile::TempDir;

    /// One-hot unit vector; cosine with its twin is 1.0, with a
    /// disjoint-index vector 0.0. Sidesteps flaky float comparisons.
    fn vec_at(idx: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; VECTOR_DIM as usize];
        v[idx] = 1.0;
        v
    }

    fn mem(content: &str, idx: usize) -> Memory {
        let mut m = Memory::new(content, MemoryType::Fact, Origin::User);
        m.vector = Some(vec_at(idx));
        m
    }

    async fn recall_with(sem: Vec<Memory>, epi: Vec<Memory>) -> (Recall, TempDir) {
        let dir = TempDir::new().unwrap();
        let s = MemoryStore::open_semantic(dir.path()).await.unwrap();
        let e = MemoryStore::open_episodic(dir.path()).await.unwrap();
        if !sem.is_empty() {
            s.insert(&sem).await.unwrap();
        }
        if !epi.is_empty() {
            e.insert(&epi).await.unwrap();
        }
        (Recall::new(Arc::new(s), Arc::new(e)), dir)
    }

    #[tokio::test]
    async fn query_spans_both_tables() {
        let (r, _d) = recall_with(vec![mem("semantic hit", 0)], vec![mem("episodic hit", 1)]).await;
        let got = r
            .query(&vec_at(0), &Filters::default(), 10, None)
            .await
            .unwrap();
        let contents: Vec<_> = got.iter().map(|(m, _)| m.content.as_str()).collect();
        assert!(contents.contains(&"semantic hit"));
        assert!(contents.contains(&"episodic hit"));
    }

    #[tokio::test]
    async fn cross_table_dup_keeps_semantic() {
        // Same vector index => cosine 1.0 ≥ threshold => near-dup.
        let (r, _d) = recall_with(
            vec![mem("curated phrasing", 3)],
            vec![mem("raw phrasing", 3)],
        )
        .await;
        let got = r
            .query(&vec_at(3), &Filters::default(), 10, None)
            .await
            .unwrap();
        assert_eq!(got.len(), 1, "near-dup across tables collapses to one");
        assert_eq!(got[0].0.content, "curated phrasing", "semantic copy wins");
    }

    #[tokio::test]
    async fn distinct_rows_kept_and_limited() {
        let (r, _d) = recall_with(
            vec![mem("s0", 0), mem("s1", 1)],
            vec![mem("e2", 2), mem("e3", 3)],
        )
        .await;
        let got = r
            .query(&vec_at(0), &Filters::default(), 3, None)
            .await
            .unwrap();
        assert_eq!(got.len(), 3, "distinct union truncated to limit");
        assert_eq!(got[0].0.content, "s0", "highest-scored row first");
    }
}
