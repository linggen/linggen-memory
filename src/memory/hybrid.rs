//! Hybrid retrieval: rank by **semantic similarity lifted by keyword
//! matches**, so a query gets dense (cosine) relevance *plus* an
//! exact-keyword boost — without the keyword side ever inventing relevance
//! that isn't there.
//!
//! ## The score
//!
//! ```text
//! hybrid = clamp01( cosine + keyword_boost )
//! keyword_boost = W · (Σ idf of matched query terms) / (Σ idf of all query terms)
//! ```
//!
//! - **cosine** is the absolute dense-relevance signal (`[0,1]` in practice
//!   for Qwen3). It anchors the score: an unrelated row stays low.
//! - **keyword_boost** ∈ `[0, W]` is the IDF-weighted *fraction* of the
//!   query's words the row contains. Rare, high-signal words ("yinyue")
//!   dominate the weighting; a common word ("name", present in most rows)
//!   has near-zero IDF and so adds almost nothing. A row matching every
//!   query term gets the full `W`; a row matching nothing gets `0`.
//!
//! Both the **ordering** and the **displayed number** are this one value, so
//! the score is monotonic with rank *and* honest:
//!
//! - A keyword hit with mediocre cosine is lifted above non-matching rows
//!   (the "dog" / "yinyue" case) — fixing pure-keyword queries.
//! - An unrelated query never shows a fake `1.0`: with low cosine and no
//!   real keyword match the top row scores low. (The old RRF approach
//!   normalized to the result-set max, so its top was *always* 1.0 even when
//!   nothing matched — that is what this replaces.)
//! - A match on only a common word barely moves the score, so it can't
//!   inflate an otherwise-irrelevant row.
//!
//! ## Floor
//!
//! `min_score` gates the **hybrid** score (not raw cosine). The default
//! (`recall_min_score`, 0.6) thus admits a keyword hit whose cosine alone
//! would fall under it — because the boost lifts it over the line — while
//! still dropping low-cosine rows with no real keyword match. The console
//! passes `min_score: 0` to show every match for inspection.
//!
//! ## Why in-process
//!
//! Both halves run over the live, flat-scanned candidate pool (the store has
//! no ANN/FTS index at this scale). Not LanceDB's native FTS: an FTS index
//! is not updated on append, so a freshly-written memory would be
//! unfindable by keyword until a reindex — the very bug this fixes. Revisit
//! at ~100k rows (same threshold the vector path and `list` flag).

use super::types::Memory;

/// Maximum lift a perfect lexical match (every query term present, weighted
/// by IDF) adds on top of cosine. Tuned so a real keyword hit clears the
/// recall floor and ranks above non-matches, while a match on only a common
/// (low-IDF) word adds almost nothing. The score is clamped to `[0,1]`, so
/// only a strong semantic match *and* a full keyword match reaches 1.0.
const KEYWORD_BOOST: f32 = 0.3;

/// A search candidate: a stored row plus its cosine similarity to the query
/// (already computed by the store from the row's vector; `0.0` for rows
/// with a null vector, which can still gain a keyword boost).
pub struct Candidate {
    pub memory: Memory,
    pub cosine: f32,
}

/// Lowercase, Unicode-aware word tokenizer. Splits on any non-alphanumeric
/// boundary and drops empties. No stemming or stopword list — IDF weighting
/// already neutralizes common words, and the store is small enough that the
/// extra machinery is not worth the surprise.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Inverse document frequency with `+1` smoothing — always positive, and a
/// term present in every doc lands near `0` (so common words barely count).
fn idf(df: usize, n: usize) -> f32 {
    (1.0 + (n as f32 - df as f32 + 0.5) / (df as f32 + 0.5)).ln()
}

/// IDF-weighted keyword boost per doc, scaled to `[0, KEYWORD_BOOST]`.
///
/// For each unique query term that actually occurs in the corpus (`df > 0`),
/// weight it by its IDF; a doc's boost is `W · (matched IDF mass / total IDF
/// mass)`. So matching the rare, meaningful word counts for most of the
/// boost and matching only a common word counts for almost none. Presence-
/// based: term frequency within a doc doesn't change the lift.
fn keyword_boosts(query_terms: &[String], docs: &[Vec<String>]) -> Vec<f32> {
    let n = docs.len();
    if n == 0 {
        return Vec::new();
    }

    // Unique query terms, then keep only those that occur in ≥1 doc and pair
    // each with its IDF weight. A term in no doc is unmatchable and must not
    // dilute the denominator.
    let mut terms: Vec<&String> = query_terms.iter().collect();
    terms.sort();
    terms.dedup();
    let weighted: Vec<(&String, f32)> = terms
        .into_iter()
        .filter_map(|t| {
            let df = docs.iter().filter(|d| d.contains(t)).count();
            (df > 0).then(|| (t, idf(df, n)))
        })
        .collect();

    let idf_total: f32 = weighted.iter().map(|(_, w)| *w).sum();
    if idf_total <= 0.0 {
        return vec![0.0; n];
    }

    docs.iter()
        .map(|doc| {
            let matched: f32 = weighted
                .iter()
                .filter(|(t, _)| doc.contains(*t))
                .map(|(_, w)| *w)
                .sum();
            KEYWORD_BOOST * (matched / idf_total)
        })
        .collect()
}

/// Score and rank `candidates`, returning the admitted rows ordered by
/// hybrid relevance.
///
/// - `query_text` is tokenized for the keyword boost; with no terms (or no
///   corpus matches) the result degrades to pure-cosine ordering.
/// - `min_score` gates the **hybrid** score (`None` = no floor).
/// - Result is truncated to `limit`.
///
/// Each hit is `(memory, cosine, hybrid)`: `cosine` is the raw dense
/// similarity (kept for the recall hook / CLI / cross-host comparison);
/// `hybrid` is the blended `[0,1]` relevance the console displays and the
/// rows are ordered by.
pub fn fuse(
    candidates: Vec<Candidate>,
    query_text: &str,
    limit: usize,
    min_score: Option<f32>,
) -> Vec<(Memory, f32, f32)> {
    if candidates.is_empty() || limit == 0 {
        return Vec::new();
    }

    let cosines: Vec<f32> = candidates.iter().map(|c| c.cosine).collect();
    let query_terms = tokenize(query_text);
    let docs: Vec<Vec<String>> = candidates
        .iter()
        .map(|c| tokenize(&c.memory.content))
        .collect();
    let boosts = keyword_boosts(&query_terms, &docs);

    let mut scored: Vec<(usize, f32, f32)> = (0..candidates.len())
        .map(|i| {
            let hybrid = (cosines[i] + boosts[i]).clamp(0.0, 1.0);
            (i, cosines[i], hybrid)
        })
        .filter(|(_, _, hybrid)| match min_score {
            Some(floor) => *hybrid >= floor,
            None => true,
        })
        .collect();

    // Order by hybrid; tie-break by cosine so equal-hybrid rows stay stable.
    scored.sort_by(|a, b| b.2.total_cmp(&a.2).then(b.1.total_cmp(&a.1)));
    scored.truncate(limit);

    // Reclaim the owned Memory values in ranked order.
    let mut by_idx: Vec<Option<Memory>> =
        candidates.into_iter().map(|c| Some(c.memory)).collect();
    scored
        .into_iter()
        .filter_map(|(i, cosine, hybrid)| by_idx[i].take().map(|m| (m, cosine, hybrid)))
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
        assert_eq!(out.len(), 1, "only the boosted keyword hit clears the floor");
        assert!(out[0].0.content.contains("dog"), "the dog row ranks first");
        assert!(out[0].2 > 0.6, "its hybrid score is lifted over the floor");
    }

    #[test]
    fn floor_admits_lexical_hit_below_cosine_threshold() {
        // cosine 0.55 < floor 0.6, but the keyword boost lifts hybrid over it.
        let out = fuse(vec![cand("my dog Yinyue", 0.55)], "dog", 10, Some(0.6));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, 0.55, "carried cosine is unchanged");
        assert!(out[0].2 >= 0.6, "hybrid (cosine + boost) clears the floor");
    }

    #[test]
    fn floor_still_drops_low_cosine_non_matches() {
        let out = fuse(vec![cand("totally unrelated row", 0.40)], "dog", 10, Some(0.6));
        assert!(out.is_empty(), "no keyword match and below floor → dropped");
    }

    #[test]
    fn unrelated_query_does_not_score_top_as_one() {
        // The bug the user caught: with the old RRF max-normalization the top
        // row was always 1.0. Now an unmatched query just shows raw cosine.
        let cands = vec![cand("foo bar", 0.30), cand("baz qux", 0.22)];
        let out = fuse(cands, "zzz", 10, None);
        assert_eq!(out[0].0.content, "foo bar", "highest cosine first");
        assert!(out[0].2 < 0.5, "no fake 1.0 for an unrelated query");
        assert_eq!(out[0].2, 0.30, "hybrid == cosine when nothing matches");
    }

    #[test]
    fn common_term_adds_little_boost() {
        // Query "yinyue name": "name" is in every row (low IDF), "yinyue" in
        // one (high IDF). The row with the rare word must win clearly; a
        // row matching only the common word barely moves.
        let mut cands = vec![cand("yinyue is the dog name", 0.50)];
        for _ in 0..4 {
            cands.push(cand("some other memory with a name", 0.50));
        }
        let out = fuse(cands, "yinyue name", 10, None);
        assert!(out[0].0.content.contains("yinyue"), "rare-word row ranks first");
        assert!(out[0].2 > 0.7, "rare-word match earns most of the boost");
        assert!(
            out[1].2 < 0.6,
            "common-word-only match barely lifts above its cosine"
        );
    }

    #[test]
    fn empty_query_terms_degrade_to_cosine_order() {
        // Punctuation-only query → no terms → pure cosine ordering + floor.
        let cands = vec![cand("alpha", 0.9), cand("beta", 0.7), cand("gamma", 0.3)];
        let out = fuse(cands, "!!!", 10, Some(0.5));
        let got: Vec<&str> = out.iter().map(|(m, _, _)| m.content.as_str()).collect();
        assert_eq!(got, vec!["alpha", "beta"], "cosine order, gamma floored out");
    }
}
