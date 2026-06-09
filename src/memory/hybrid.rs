//! Hybrid retrieval: fuse dense (vector / cosine) and lexical (BM25)
//! rankings so a query gets the best of both — semantic matches *and*
//! exact-keyword matches.
//!
//! ## Why
//!
//! Pure vector search has no exact-keyword guarantee. A bare query like
//! `"dog"` embeds to a point whose cosine to a long, multi-topic passage
//! ("…male dog named Yinyue … the cat Xiaoman … mascot …") is diluted
//! below that of shorter, unrelated rows — so the row that literally
//! contains the word can rank *below* rows that don't, or fall under the
//! recall floor entirely. BM25 fixes exactly this: a literal term match
//! scores high regardless of passage length or topic spread.
//!
//! ## How (Phase 3b)
//!
//! Each table is fetched in full (the store already runs flat — no ANN
//! index — at this scale; see [`store::MemoryStore::scored_candidates`]),
//! cosine is computed per row, and BM25 is computed over the same candidate
//! set as the lexical corpus. The two rankings are fused with **Reciprocal
//! Rank Fusion** (RRF) — rank-based, so it needs no score normalization and
//! is robust to the incomparable scales of cosine (`[-1, 1]`) and BM25
//! (`[0, ∞)`).
//!
//! ## Floor with lexical override
//!
//! The `min_score` recall floor stays a **cosine** gate (its 0.6 default is
//! calibrated to Qwen3 cosine, not to RRF), but a row that is a genuine
//! lexical hit (`bm25 > 0`) bypasses it. That is the whole point: the "dog"
//! row (cosine ~0.55, below the 0.6 floor) is admitted because it literally
//! matches, then floated up by RRF.
//!
//! The score each row *carries out* is still its **cosine** — familiar
//! `[0, 1]`, what the console column and recall-hook display already show,
//! and what cross-table merges compare. RRF governs *ordering only*; it is
//! intentionally not surfaced as the row score.
//!
//! Why in-process and not LanceDB's native FTS index: an FTS index is not
//! updated on append, so a freshly-written memory would be invisible to
//! keyword search until a reindex/optimize — reintroducing the very
//! "my memory isn't findable" bug this fixes. In-process BM25 over the live
//! rows is always current. Revisit if the store outgrows flat scans
//! (~100k rows), same threshold the vector path and `list` already flag.

use super::types::Memory;

/// Okapi BM25 term-frequency saturation. Standard default.
const BM25_K1: f32 = 1.2;
/// Okapi BM25 length-normalization strength. Standard default.
const BM25_B: f32 = 0.75;
/// Reciprocal Rank Fusion constant. 60 is the value from the original RRF
/// paper and the de-facto default; it damps the contribution of any single
/// ranker's top spot so a row strong in *both* lists wins over one that is
/// merely #1 in a single list.
const RRF_K: f32 = 60.0;

/// A search candidate: a stored row plus its cosine similarity to the query
/// (already computed by the store from the row's vector; `0.0` for rows
/// with a null vector, which can still be admitted as lexical hits).
pub struct Candidate {
    pub memory: Memory,
    pub cosine: f32,
}

/// Lowercase, Unicode-aware word tokenizer. Splits on any non-alphanumeric
/// boundary and drops empties. No stemming or stopword list — BM25's IDF
/// already downweights terms common across the corpus, and the store is
/// small enough that the extra machinery is not worth the surprise.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// BM25 score of every document against `query_terms`, using `docs` as the
/// corpus (so IDF and average length reflect exactly the rows being
/// searched). Returns one score per doc, in input order; a doc with no
/// query-term overlap scores `0.0`.
fn bm25_scores(query_terms: &[String], docs: &[Vec<String>]) -> Vec<f32> {
    let n = docs.len();
    if n == 0 || query_terms.is_empty() {
        return vec![0.0; n];
    }

    let avgdl = docs.iter().map(|d| d.len()).sum::<usize>() as f32 / n as f32;
    let avgdl = if avgdl == 0.0 { 1.0 } else { avgdl };

    // Dedup query terms: a term repeated in the query shouldn't double-count.
    let mut terms: Vec<&String> = query_terms.iter().collect();
    terms.sort();
    terms.dedup();

    // Document frequency per (unique) query term.
    let df: Vec<usize> = terms
        .iter()
        .map(|t| docs.iter().filter(|d| d.contains(*t)).count())
        .collect();

    docs.iter()
        .map(|doc| {
            let dl = doc.len() as f32;
            terms
                .iter()
                .enumerate()
                .map(|(i, term)| {
                    let df_t = df[i];
                    if df_t == 0 {
                        return 0.0;
                    }
                    let tf = doc.iter().filter(|w| *w == *term).count() as f32;
                    if tf == 0.0 {
                        return 0.0;
                    }
                    // IDF with +1 smoothing — always positive, so a term
                    // present in every doc contributes a small amount
                    // rather than going negative.
                    let idf = (1.0 + (n as f32 - df_t as f32 + 0.5) / (df_t as f32 + 0.5)).ln();
                    let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avgdl);
                    idf * (tf * (BM25_K1 + 1.0)) / denom
                })
                .sum()
        })
        .collect()
}

/// Map each index to its 0-based rank under `desc_key` (highest key = rank
/// 0). Indices not passing `keep` are omitted from the ranking entirely.
fn rank_by(keys: &[f32], keep: impl Fn(usize) -> bool) -> std::collections::HashMap<usize, usize> {
    let mut idx: Vec<usize> = (0..keys.len()).filter(|&i| keep(i)).collect();
    idx.sort_by(|&a, &b| keys[b].total_cmp(&keys[a]));
    idx.into_iter().enumerate().map(|(rank, i)| (i, rank)).collect()
}

/// Fuse the vector and lexical rankings over `candidates` and return the
/// admitted rows ordered by RRF, each paired with its **cosine** score.
///
/// - `query_text` is tokenized for BM25; if it yields no terms the result
///   degrades gracefully to pure-cosine ordering with the floor applied.
/// - `min_score` is the cosine floor (`None` = no floor). A row below the
///   floor is still admitted if it is a lexical hit (`bm25 > 0`).
/// - Result is truncated to `limit`.
pub fn fuse(
    candidates: Vec<Candidate>,
    query_text: &str,
    limit: usize,
    min_score: Option<f32>,
) -> Vec<(Memory, f32)> {
    if candidates.is_empty() || limit == 0 {
        return Vec::new();
    }

    let cosines: Vec<f32> = candidates.iter().map(|c| c.cosine).collect();
    let query_terms = tokenize(query_text);
    let docs: Vec<Vec<String>> = candidates
        .iter()
        .map(|c| tokenize(&c.memory.content))
        .collect();
    let bm25 = bm25_scores(&query_terms, &docs);

    // Rank each retriever. Vector ranks every candidate (cosine always
    // defined); lexical ranks only true hits (bm25 > 0).
    let vec_rank = rank_by(&cosines, |_| true);
    let lex_rank = rank_by(&bm25, |i| bm25[i] > 0.0);

    let mut fused: Vec<(usize, f32)> = (0..candidates.len())
        .filter(|&i| {
            // Admission: pass the cosine floor, OR be a lexical hit.
            match min_score {
                Some(floor) => cosines[i] >= floor || lex_rank.contains_key(&i),
                None => true,
            }
        })
        .map(|i| {
            let mut score = 0.0;
            if let Some(r) = vec_rank.get(&i) {
                score += 1.0 / (RRF_K + *r as f32);
            }
            if let Some(r) = lex_rank.get(&i) {
                score += 1.0 / (RRF_K + *r as f32);
            }
            (i, score)
        })
        .collect();

    // Order by RRF; tie-break by cosine so identical-rank rows stay stable
    // and intuitive.
    fused.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| cosines[b.0].total_cmp(&cosines[a.0]))
    });
    fused.truncate(limit);

    // Reclaim the owned Memory values in fused order. Build an index→Memory
    // map by draining once, then emit in order.
    let mut by_idx: Vec<Option<Memory>> =
        candidates.into_iter().map(|c| Some(c.memory)).collect();
    fused
        .into_iter()
        .filter_map(|(i, _)| by_idx[i].take().map(|m| (m, cosines[i])))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{MemoryType, Origin};

    fn cand(content: &str, cosine: f32) -> Candidate {
        Candidate {
            memory: Memory::new(content, MemoryType::Fact, Origin::User),
            cosine,
        }
    }

    #[test]
    fn tokenize_splits_and_lowercases() {
        assert_eq!(tokenize("Yinyue's dog!"), vec!["yinyue", "s", "dog"]);
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn keyword_hit_beats_higher_cosine_non_match() {
        // The motivating bug: a row that literally contains "dog" but has a
        // lower cosine must outrank higher-cosine rows that don't match.
        let cands = vec![
            cand("agents can ask the user a question", 0.58),
            cand("the responses api reasoning effort", 0.56),
            cand("user has a male dog named Yinyue, separate from the cat", 0.55),
        ];
        let out = fuse(cands, "dog", 10, Some(0.6));
        assert_eq!(out.len(), 1, "only the lexical hit clears the floor");
        assert!(out[0].0.content.contains("dog"), "the dog row ranks first");
    }

    #[test]
    fn floor_admits_lexical_hit_below_threshold() {
        // cosine 0.55 < floor 0.6, but it's the keyword match → admitted.
        let out = fuse(vec![cand("my dog Yinyue", 0.55)], "dog", 10, Some(0.6));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, 0.55, "carried score stays cosine, not RRF");
    }

    #[test]
    fn floor_still_drops_low_cosine_non_matches() {
        let out = fuse(vec![cand("totally unrelated row", 0.40)], "dog", 10, Some(0.6));
        assert!(out.is_empty(), "no keyword match and below floor → dropped");
    }

    #[test]
    fn empty_query_terms_degrade_to_cosine_order() {
        // Punctuation-only query → no terms → pure cosine ordering + floor.
        let cands = vec![cand("alpha", 0.9), cand("beta", 0.7), cand("gamma", 0.3)];
        let out = fuse(cands, "!!!", 10, Some(0.5));
        let got: Vec<&str> = out.iter().map(|(m, _)| m.content.as_str()).collect();
        assert_eq!(got, vec!["alpha", "beta"], "cosine order, gamma floored out");
    }

    #[test]
    fn both_lists_boost_a_row() {
        // A row strong in BOTH cosine and lexical should win over one strong
        // in only the vector list.
        let cands = vec![
            cand("dog dog dog", 0.80),                 // top lexical + high cosine
            cand("unrelated but high cosine", 0.82),   // top cosine, no keyword
        ];
        let out = fuse(cands, "dog", 10, None);
        assert_eq!(out[0].0.content, "dog dog dog", "dual-list row wins via RRF");
    }
}
