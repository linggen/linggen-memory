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
//! - Cross-table + episodic-internal dedup: a hit whose content is
//!   byte-identical to an already-accepted hit is dropped. Semantic is
//!   processed first, so the curated copy wins over its un-promoted
//!   episodic twin. Cosine is **not** a sameness test here — it can't
//!   separate a restatement from a contradiction, and collapsing a
//!   high-cosine pair would *hide* a contradiction that the Reconcile
//!   contract needs surfaced for the user/LLM to resolve.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::memory::{Candidate, Filters, Memory, MemoryStore};

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

    /// Hybrid recall across both tables. Fuses dense (cosine) and lexical
    /// (BM25) rankings over the *combined* candidate pool so the RRF order is
    /// global, not per-table-then-merged. `query_text` drives BM25;
    /// `min_score` is the cosine floor, bypassed for lexical hits (see
    /// [`crate::memory::hybrid`]). Rows carry their cosine score.
    pub async fn query(
        &self,
        query_vec: &[f32],
        query_text: &str,
        filters: &Filters,
        limit: usize,
        min_score: Option<f32>,
    ) -> Result<Vec<(Memory, f32)>> {
        // Pull the full filtered candidate pool from both tables. Fusion and
        // the floor happen once over the union, not per table — so a row's
        // rank reflects the whole corpus.
        let (semantic, episodic) = tokio::try_join!(
            self.semantic.scored_candidates(query_vec, filters),
            self.episodic.scored_candidates(query_vec, filters),
        )?;

        // Cross-table + episodic-internal dedup BEFORE fusion. Semantic
        // first so the curated copy wins an exact-content tie; a candidate is
        // dropped only if its content is byte-identical to one already kept.
        // A high-cosine *contradiction* (different content) is intentionally
        // kept so recall can surface it (Reconcile, memory-spec §2).
        let mut pool: Vec<Candidate> = Vec::with_capacity(semantic.len() + episodic.len());
        for cand in semantic.into_iter().chain(episodic) {
            if !pool.iter().any(|kept| is_near_dup(&kept.memory, &cand.memory)) {
                pool.push(cand);
            }
        }

        Ok(crate::memory::hybrid::fuse(pool, query_text, limit, min_score))
    }
}

/// Union dedup test: byte-identical content (trimmed). Cosine is
/// deliberately not used — it cannot tell a restatement from a
/// contradiction, and collapsing a high-cosine pair here would hide a
/// contradiction recall must surface (Reconcile, memory-spec §2).
fn is_near_dup(a: &Memory, b: &Memory) -> bool {
    a.content.trim() == b.content.trim()
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
            .query(&vec_at(0), "", &Filters::default(), 10, None)
            .await
            .unwrap();
        let contents: Vec<_> = got.iter().map(|(m, _)| m.content.as_str()).collect();
        assert!(contents.contains(&"semantic hit"));
        assert!(contents.contains(&"episodic hit"));
    }

    #[tokio::test]
    async fn cross_table_exact_dup_keeps_semantic() {
        // Byte-identical content across tables => collapses to one.
        let (r, _d) = recall_with(
            vec![mem("user has a cat named Xiaoman", 3)],
            vec![mem("user has a cat named Xiaoman", 3)],
        )
        .await;
        let got = r
            .query(&vec_at(3), "", &Filters::default(), 10, None)
            .await
            .unwrap();
        assert_eq!(got.len(), 1, "exact-content dup across tables collapses");
        assert_eq!(
            got[0].0.content, "user has a cat named Xiaoman",
            "semantic copy wins"
        );
    }

    #[tokio::test]
    async fn contradiction_not_collapsed() {
        // High cosine (same vector idx), DIFFERENT content = a contradiction.
        // Must NOT collapse — recall has to surface both so Reconcile can
        // resolve it. This is the whole point of dropping cosine-dedup.
        let (r, _d) = recall_with(
            vec![mem("the cat is male", 5)],
            vec![mem("the cat is female", 5)],
        )
        .await;
        let got = r
            .query(&vec_at(5), "", &Filters::default(), 10, None)
            .await
            .unwrap();
        assert_eq!(got.len(), 2, "a contradiction must surface both rows");
    }

    #[tokio::test]
    async fn distinct_rows_kept_and_limited() {
        let (r, _d) = recall_with(
            vec![mem("s0", 0), mem("s1", 1)],
            vec![mem("e2", 2), mem("e3", 3)],
        )
        .await;
        let got = r
            .query(&vec_at(0), "", &Filters::default(), 3, None)
            .await
            .unwrap();
        assert_eq!(got.len(), 3, "distinct union truncated to limit");
        assert_eq!(got[0].0.content, "s0", "highest-scored row first");
    }
}
