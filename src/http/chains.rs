//! Chain scan — the condense stage's mechanical detector (see
//! `../../linggen/doc/memory-spec.md` §2 Condense).
//!
//! Semantic is append-mostly, so project truths accumulate **chains**:
//! same-subject rows where the newest completes or obsoletes the rest.
//! This endpoint computes them so the condense mission's agent (which is
//! Memory-tools-only and cannot script) never has to page the whole
//! store through its context. Read-only, zero-LLM — judgment and the
//! actual `replace_ids` merges belong to the caller.
//!
//! Three scan kinds, selected by `kind`:
//!
//! - **`cited`** (tier-1) — rows whose content cites another row's id
//!   verbatim. Citations are provable edges; connected components over
//!   them are auto-accept-quality chains.
//! - **`marker`** (tier-2) — rows carrying provisional-state language
//!   ("OPEN:", "uncommitted", "pending", …), each with its nearest
//!   stored-vector neighbors. Candidates only: the caller must confirm
//!   a real supersession before merging. Rows already named by a review
//!   issue (any status) are excluded, so each capped page surfaces fresh
//!   candidates instead of re-serving the queue.
//! - **`subject`** (v2) — star clusters over stored vectors: a seed row
//!   plus every row within `SUBJECT_FLOOR` cosine of it, minimum
//!   [`SUBJECT_MIN_ROWS`] members. Feeds the digest pass — N same-subject
//!   rows condensed into one focused per-subject row (never one mega
//!   state row: clusters are seed-anchored and size-capped, not
//!   transitive-closure blobs).
//!
//! Every cluster carries `derived_only` — true when all member rows are
//! the agent's own notes (`from=derived`, `tier=semantic`). The merge
//! law gates on it: derived-only chains may be merged unattended;
//! anything touching the user's voice may not.

use super::envelope::{ok, ApiError};
use super::state::SharedState;
use crate::memory::store::cosine_similarity;
use crate::memory::{Filters, Memory, Origin, SortOrder, Tier};
use axum::extract::State;
use axum::response::Response;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub fn router() -> Router<SharedState> {
    use axum::routing::post;
    Router::new().route("/api/memory/chains", post(chains))
}

/// Provisional-state markers, matched case-insensitively. Deliberately a
/// fixed list, not config: tier-2 hits are LLM-confirmed downstream, so
/// recall beats precision here — but every entry should still scream
/// "this row describes an unfinished state".
const MARKERS_CI: &[&str] = &[
    "uncommitted",
    "not committed",
    "not started",
    "unbuilt",
    "deferred",
    "in progress",
    "awaiting",
    "still pending",
    "build pending",
    "impl pending",
];

/// Markers matched case-sensitively — lowercase forms are too common in
/// ordinary prose ("open the file", "todo lists").
const MARKERS_CS: &[&str] = &["OPEN:", "OPEN(", "TODO:", "NEXT:", "PENDING"];

/// Neighbors below this cosine are noise, not chain candidates. Sits
/// under the restatement band (0.78–0.97 on the real Qwen3 distribution)
/// and above unrelated pairs (≤0.65) — chain members are related but not
/// restatements, so the floor leans permissive.
const NEIGHBOR_FLOOR: f32 = 0.60;
const NEIGHBORS_PER_ROW: usize = 5;

/// Subject clusters need members that genuinely share a subject, not
/// merely a domain — tighter than [`NEIGHBOR_FLOOR`], looser than the
/// restatement band (0.78+).
const SUBJECT_FLOOR: f32 = 0.70;
/// A 2-row near-pair is v1 (marker/cited) territory; digests earn their
/// keep at 3+.
const SUBJECT_MIN_ROWS: usize = 3;
/// Cap members per cluster — a digest over 15+ rows drifts toward the
/// forbidden mega state row; the tail stays for the next pass.
const SUBJECT_MAX_ROWS: usize = 12;

const DEFAULT_LIMIT: usize = 10;
/// The scan needs every semantic row in memory at once; the table is
/// low-volume by design (hundreds), so a flat fetch is fine.
const SCAN_FETCH_LIMIT: usize = 100_000;

#[derive(Debug, Deserialize)]
struct ChainsRequest {
    /// `cited` (default), `marker`, or `subject`.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    /// Only clusters mergeable unattended (every row an agent note).
    /// The condense mission passes true so user-voice clusters never
    /// pollute its worklist pages across runs.
    #[serde(default)]
    derived_only: bool,
}

async fn chains(
    State(state): State<SharedState>,
    Json(req): Json<ChainsRequest>,
) -> Result<Response, ApiError> {
    let kind = req.kind.as_deref().unwrap_or("cited");
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).max(1);
    let offset = req.offset.unwrap_or(0);

    let rows = state
        .store
        .list(&Filters::default(), SortOrder::Oldest, SCAN_FETCH_LIMIT, 0)
        .await
        .map_err(|e| ApiError::internal(e.context("listing semantic rows")))?;

    match kind {
        "cited" => Ok(ok(cited_chains(&rows, limit, offset, req.derived_only))),
        "marker" => {
            let issued = super::issues::issued_row_ids(&state.data_dir).await;
            Ok(ok(marker_candidates(&rows, limit, offset, req.derived_only, &issued)))
        }
        "subject" => Ok(ok(subject_clusters(&rows, limit, offset, req.derived_only))),
        other => Err(ApiError::bad_request(format!(
            "invalid kind {other:?}: expected \"cited\", \"marker\", or \"subject\""
        ))),
    }
}

// ── Tier-1: citation chains ─────────────────────────────────────────────────

/// Verbatim id-citation edges → connected components of size ≥ 2.
fn cited_chains(rows: &[Memory], limit: usize, offset: usize, derived_only: bool) -> Value {
    let edges = citation_edges(rows);

    // Union-find over row indices.
    let mut parent: Vec<usize> = (0..rows.len()).collect();
    fn find(parent: &mut Vec<usize>, i: usize) -> usize {
        let mut root = i;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = i;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }
    for &(citer, cited) in &edges {
        let (a, b) = (find(&mut parent, citer), find(&mut parent, cited));
        if a != b {
            parent[a] = b;
        }
    }

    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..rows.len() {
        components.entry(find(&mut parent, i)).or_default().push(i);
    }

    // Rows arrive oldest-first, so members and chains are already in a
    // stable oldest-first order — deterministic pagination for free.
    let mut chains: Vec<Vec<usize>> = components
        .into_values()
        .filter(|members| members.len() >= 2)
        .filter(|members| !derived_only || members.iter().all(|&i| is_derived_note(&rows[i])))
        .collect();
    chains.sort_by_key(|members| members[0]);

    let total = chains.len();
    let page: Vec<Value> = chains
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|members| {
            let chain_edges: Vec<Value> = edges
                .iter()
                .filter(|(citer, cited)| members.contains(citer) && members.contains(cited))
                .map(|&(citer, cited)| {
                    json!({ "citer": rows[citer].id, "cited": rows[cited].id })
                })
                .collect();
            json!({
                "derived_only": members.iter().all(|&i| is_derived_note(&rows[i])),
                "rows": members.iter().map(|&i| public_row(&rows[i])).collect::<Vec<_>>(),
                "edges": chain_edges,
            })
        })
        .collect();

    json!({
        "kind": "cited",
        "scanned": rows.len(),
        "total": total,
        "offset": offset,
        "chains": page,
    })
}

/// Every `(citer, cited)` index pair where one row's content contains
/// another row's id verbatim. Ids are 10-char base62 (or legacy UUIDs),
/// so a hit is essentially proof of reference — the boundary check only
/// guards the theoretical id-inside-a-longer-token case.
fn citation_edges(rows: &[Memory]) -> Vec<(usize, usize)> {
    let mut edges = Vec::new();
    for (cited_idx, cited) in rows.iter().enumerate() {
        if cited.id.len() < 8 {
            continue; // defensive: too short to be an unambiguous citation
        }
        for (citer_idx, citer) in rows.iter().enumerate() {
            if citer_idx != cited_idx && contains_id(&citer.content, &cited.id) {
                edges.push((citer_idx, cited_idx));
            }
        }
    }
    edges
}

/// `content` contains `id` with non-alphanumeric characters (or string
/// ends) on both sides.
fn contains_id(content: &str, id: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = content[start..].find(id) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !content[..abs]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let after_ok = !content[abs + id.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        start = abs + id.len();
    }
    false
}

// ── Tier-2: marker candidates ───────────────────────────────────────────────

/// Rows carrying provisional-state markers, each with its nearest
/// stored-vector neighbors. Rows already inside a cited chain are
/// skipped — the tier-1 pass owns them. Rows named by a review issue
/// (`issued`, any status) are skipped too — queueing consumes nothing,
/// so without this the same oldest candidates re-queue as "already
/// queued" forever and the scan never reaches the rest of the store.
fn marker_candidates(
    rows: &[Memory],
    limit: usize,
    offset: usize,
    derived_only: bool,
    issued: &HashSet<String>,
) -> Value {
    let in_cited: Vec<bool> = {
        let edges = citation_edges(rows);
        let mut flags = vec![false; rows.len()];
        for (a, b) in edges {
            flags[a] = true;
            flags[b] = true;
        }
        flags
    };

    let (marked, queued_skipped): (Vec<(usize, &'static str)>, usize) = {
        let mut kept = Vec::new();
        let mut skipped = 0usize;
        let candidates = rows
            .iter()
            .enumerate()
            .filter(|(i, row)| !in_cited[*i] && row.tier != Tier::Core)
            .filter(|(_, row)| !derived_only || is_derived_note(row))
            .filter_map(|(i, row)| find_marker(&row.content).map(|m| (i, m)));
        for (i, marker) in candidates {
            if issued.contains(&rows[i].id) {
                skipped += 1;
            } else {
                kept.push((i, marker));
            }
        }
        (kept, skipped)
    };

    let total = marked.len();
    // Neighbors are computed for the requested page only — the O(N)
    // cosine sweep per row is cheap, but not worth doing 200× per call.
    let page: Vec<Value> = marked
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(i, marker)| {
            json!({
                "marker": marker,
                "derived_only": is_derived_note(&rows[i]),
                "row": public_row(&rows[i]),
                "neighbors": neighbors_of(rows, i),
            })
        })
        .collect();

    json!({
        "kind": "marker",
        "scanned": rows.len(),
        "total": total,
        "offset": offset,
        // Marker rows hidden because a review issue already names them —
        // reported so a caller never mistakes a filtered scan for a clean
        // store.
        "queued_skipped": queued_skipped,
        "candidates": page,
    })
}

fn find_marker(content: &str) -> Option<&'static str> {
    if let Some(m) = MARKERS_CS.iter().find(|m| content.contains(*m)) {
        return Some(m);
    }
    let lower = content.to_lowercase();
    MARKERS_CI.iter().find(|m| lower.contains(*m)).copied()
}

/// Top stored-vector neighbors of `rows[idx]` above the floor.
fn neighbors_of(rows: &[Memory], idx: usize) -> Vec<Value> {
    let Some(query) = rows[idx].vector.as_deref() else {
        return Vec::new();
    };
    let mut scored: Vec<(f32, usize)> = rows
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .filter_map(|(i, row)| {
            let v = row.vector.as_deref()?;
            let score = cosine_similarity(query, v);
            (score >= NEIGHBOR_FLOOR).then_some((score, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(NEIGHBORS_PER_ROW)
        .map(|(score, i)| {
            json!({
                "score": (score * 1000.0).round() / 1000.0,
                "row": public_row(&rows[i]),
            })
        })
        .collect()
}

// ── V2: subject clusters ────────────────────────────────────────────────────

/// Star clusters over stored vectors: walk rows oldest-first; each
/// unassigned row seeds a cluster of the unassigned rows within
/// [`SUBJECT_FLOOR`] cosine of it (closest first, capped at
/// [`SUBJECT_MAX_ROWS`]). Seed-anchored on purpose — transitive closure
/// would chain drifting subjects into exactly the mega-blob the digest
/// rule forbids. Clusters below [`SUBJECT_MIN_ROWS`] dissolve (their
/// members stay available to later seeds).
///
/// Rows already in a cited chain are excluded — collapse provable
/// chains (v1) before digesting what remains.
fn subject_clusters(rows: &[Memory], limit: usize, offset: usize, derived_only: bool) -> Value {
    let in_cited: Vec<bool> = {
        let edges = citation_edges(rows);
        let mut flags = vec![false; rows.len()];
        for (a, b) in edges {
            flags[a] = true;
            flags[b] = true;
        }
        flags
    };

    let pool: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(i, row)| {
            !in_cited[*i]
                && row.tier != Tier::Core
                && row.vector.is_some()
                && (!derived_only || is_derived_note(row))
        })
        .map(|(i, _)| i)
        .collect();

    let mut assigned = vec![false; rows.len()];
    let mut clusters: Vec<(usize, Vec<(f32, usize)>)> = Vec::new(); // (seed, members-with-score)

    for &seed in &pool {
        if assigned[seed] {
            continue;
        }
        let seed_vec = rows[seed].vector.as_deref().expect("pool rows have vectors");
        let mut members: Vec<(f32, usize)> = pool
            .iter()
            .filter(|&&i| i != seed && !assigned[i])
            .filter_map(|&i| {
                let v = rows[i].vector.as_deref()?;
                let score = cosine_similarity(seed_vec, v);
                (score >= SUBJECT_FLOOR).then_some((score, i))
            })
            .collect();
        if members.len() + 1 < SUBJECT_MIN_ROWS {
            continue; // seed stays unassigned — a later seed may absorb it
        }
        members.sort_by(|a, b| b.0.total_cmp(&a.0));
        members.truncate(SUBJECT_MAX_ROWS - 1);
        assigned[seed] = true;
        for &(_, i) in &members {
            assigned[i] = true;
        }
        members.insert(0, (1.0, seed));
        clusters.push((seed, members));
    }

    let total = clusters.len();
    let page: Vec<Value> = clusters
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(seed, members)| {
            json!({
                "seed_id": rows[seed].id,
                "derived_only": members.iter().all(|&(_, i)| is_derived_note(&rows[i])),
                "rows": members.iter().map(|&(score, i)| {
                    let mut row = public_row(&rows[i]);
                    row["score"] = json!((score * 1000.0).round() / 1000.0);
                    row
                }).collect::<Vec<_>>(),
            })
        })
        .collect();

    json!({
        "kind": "subject",
        "scanned": rows.len(),
        "total": total,
        "offset": offset,
        "clusters": page,
    })
}

// ── Shared helpers ──────────────────────────────────────────────────────────

/// The merge law's unattended-merge bar: the agent's own note, not the
/// user's voice, and never a core row.
fn is_derived_note(row: &Memory) -> bool {
    row.origin == Origin::Derived && row.tier == Tier::Semantic
}

/// Row as the caller sees it — no vector (1024 floats of context noise).
fn public_row(row: &Memory) -> Value {
    json!({
        "id": row.id,
        "content": row.content,
        "type": row.r#type.as_str(),
        "from": row.origin.as_str(),
        "tier": row.tier.as_str(),
        "contexts": row.contexts,
        "created_at": row.created_at,
        "occurred_at": row.occurred_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryType;

    fn row(id: &str, content: &str, origin: Origin) -> Memory {
        let mut m = Memory::new(content, MemoryType::Built, origin);
        m.id = id.to_string();
        m
    }

    #[test]
    fn contains_id_respects_boundaries() {
        assert!(contains_id("see row 7gOUF6ryq6 for detail", "7gOUF6ryq6"));
        assert!(contains_id("(id=7gOUF6ryq6)", "7gOUF6ryq6"));
        assert!(!contains_id("x7gOUF6ryq6", "7gOUF6ryq6"));
        assert!(!contains_id("7gOUF6ryq6a", "7gOUF6ryq6"));
        assert!(contains_id("7gOUF6ryq6", "7gOUF6ryq6"));
    }

    #[test]
    fn cited_chain_found_and_derived_flagged() {
        let rows = vec![
            row("aaaaaaaaaa", "design locked, impl not started", Origin::Derived),
            row("bbbbbbbbbb", "build shipped (supersedes aaaaaaaaaa)", Origin::Derived),
            row("cccccccccc", "unrelated fact", Origin::User),
        ];
        let out = cited_chains(&rows, 10, 0, false);
        assert_eq!(out["total"], 1);
        let chain = &out["chains"][0];
        assert_eq!(chain["rows"].as_array().unwrap().len(), 2);
        assert_eq!(chain["derived_only"], true);
        assert_eq!(chain["edges"][0]["citer"], "bbbbbbbbbb");
        assert_eq!(chain["edges"][0]["cited"], "aaaaaaaaaa");
    }

    #[test]
    fn user_voice_row_breaks_derived_only() {
        let rows = vec![
            row("aaaaaaaaaa", "pref v1", Origin::User),
            row("bbbbbbbbbb", "updates aaaaaaaaaa", Origin::Derived),
        ];
        let out = cited_chains(&rows, 10, 0, false);
        assert_eq!(out["chains"][0]["derived_only"], false);
    }

    fn vrow(id: &str, content: &str, origin: Origin, vector: Vec<f32>) -> Memory {
        let mut m = row(id, content, origin);
        m.vector = Some(vector);
        m
    }

    #[test]
    fn subject_clusters_group_by_cosine_with_min_rows() {
        let rows = vec![
            vrow("aaaaaaaaaa", "sanji planner one", Origin::Derived, vec![1.0, 0.0]),
            vrow("bbbbbbbbbb", "sanji planner two", Origin::Derived, vec![0.98, 0.199]),
            vrow("cccccccccc", "sanji planner three", Origin::Derived, vec![0.95, 0.312]),
            vrow("dddddddddd", "unrelated topic", Origin::Derived, vec![0.0, 1.0]),
        ];
        let out = subject_clusters(&rows, 10, 0, true);
        assert_eq!(out["total"], 1);
        let cluster = &out["clusters"][0];
        assert_eq!(cluster["rows"].as_array().unwrap().len(), 3);
        assert_eq!(cluster["seed_id"], "aaaaaaaaaa");
        assert_eq!(cluster["derived_only"], true);
    }

    #[test]
    fn subject_pair_below_min_rows_dissolves() {
        let rows = vec![
            vrow("aaaaaaaaaa", "topic a", Origin::Derived, vec![1.0, 0.0]),
            vrow("bbbbbbbbbb", "topic a again", Origin::Derived, vec![0.99, 0.141]),
            vrow("dddddddddd", "unrelated", Origin::Derived, vec![0.0, 1.0]),
        ];
        let out = subject_clusters(&rows, 10, 0, true);
        assert_eq!(out["total"], 0);
    }

    #[test]
    fn subject_derived_only_excludes_user_rows_from_pool() {
        let rows = vec![
            vrow("aaaaaaaaaa", "pref one", Origin::User, vec![1.0, 0.0]),
            vrow("bbbbbbbbbb", "pref two", Origin::Derived, vec![0.98, 0.199]),
            vrow("cccccccccc", "pref three", Origin::Derived, vec![0.95, 0.312]),
        ];
        // With the user row excluded, only 2 remain — below min → no cluster.
        let out = subject_clusters(&rows, 10, 0, true);
        assert_eq!(out["total"], 0);
        // Without derived_only, the mixed cluster forms and is flagged.
        let out = subject_clusters(&rows, 10, 0, false);
        assert_eq!(out["total"], 1);
        assert_eq!(out["clusters"][0]["derived_only"], false);
    }

    #[test]
    fn marker_rows_flagged_without_cited_members() {
        let rows = vec![
            row("aaaaaaaaaa", "condense build OPEN: chains verb pending", Origin::Derived),
            row("bbbbbbbbbb", "merged, see aaaaaaaaaa", Origin::Derived),
            row("cccccccccc", "feature X design locked, impl not started", Origin::Derived),
        ];
        let out = marker_candidates(&rows, 10, 0, false, &HashSet::new());
        // aaaaaaaaaa is in a cited chain → excluded; cccccccccc matches.
        assert_eq!(out["total"], 1);
        assert_eq!(out["candidates"][0]["row"]["id"], "cccccccccc");
        assert_eq!(out["candidates"][0]["marker"], "not started");
        assert_eq!(out["queued_skipped"], 0);
    }

    #[test]
    fn marker_rows_named_by_issues_are_skipped_and_counted() {
        let rows = vec![
            row("aaaaaaaaaa", "migration deferred until the schema lands", Origin::Derived),
            row("bbbbbbbbbb", "feature X design locked, impl not started", Origin::Derived),
        ];
        let issued: HashSet<String> = ["aaaaaaaaaa".to_string()].into();
        let out = marker_candidates(&rows, 10, 0, false, &issued);
        // The queued row no longer burns a slot; the fresh one surfaces.
        assert_eq!(out["total"], 1);
        assert_eq!(out["queued_skipped"], 1);
        assert_eq!(out["candidates"][0]["row"]["id"], "bbbbbbbbbb");
    }
}
