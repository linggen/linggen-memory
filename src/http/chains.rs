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
//! Two confidence classes, selected by `kind`:
//!
//! - **`cited`** (tier-1) — rows whose content cites another row's id
//!   verbatim. Citations are provable edges; connected components over
//!   them are auto-accept-quality chains.
//! - **`marker`** (tier-2) — rows carrying provisional-state language
//!   ("OPEN:", "uncommitted", "pending", …), each with its nearest
//!   stored-vector neighbors. Candidates only: the caller must confirm
//!   a real supersession before merging.
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
use std::collections::HashMap;

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

const DEFAULT_LIMIT: usize = 10;
/// The scan needs every semantic row in memory at once; the table is
/// low-volume by design (hundreds), so a flat fetch is fine.
const SCAN_FETCH_LIMIT: usize = 100_000;

#[derive(Debug, Deserialize)]
struct ChainsRequest {
    /// `cited` (default) or `marker`.
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
        "marker" => Ok(ok(marker_candidates(&rows, limit, offset, req.derived_only))),
        other => Err(ApiError::bad_request(format!(
            "invalid kind {other:?}: expected \"cited\" or \"marker\""
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
/// skipped — the tier-1 pass owns them.
fn marker_candidates(rows: &[Memory], limit: usize, offset: usize, derived_only: bool) -> Value {
    let in_cited: Vec<bool> = {
        let edges = citation_edges(rows);
        let mut flags = vec![false; rows.len()];
        for (a, b) in edges {
            flags[a] = true;
            flags[b] = true;
        }
        flags
    };

    let marked: Vec<(usize, &'static str)> = rows
        .iter()
        .enumerate()
        .filter(|(i, row)| !in_cited[*i] && row.tier != Tier::Core)
        .filter(|(_, row)| !derived_only || is_derived_note(row))
        .filter_map(|(i, row)| find_marker(&row.content).map(|m| (i, m)))
        .collect();

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

    #[test]
    fn marker_rows_flagged_without_cited_members() {
        let rows = vec![
            row("aaaaaaaaaa", "condense build OPEN: chains verb pending", Origin::Derived),
            row("bbbbbbbbbb", "merged, see aaaaaaaaaa", Origin::Derived),
            row("cccccccccc", "feature X design locked, impl not started", Origin::Derived),
        ];
        let out = marker_candidates(&rows, 10, 0, false);
        // aaaaaaaaaa is in a cited chain → excluded; cccccccccc matches.
        assert_eq!(out["total"], 1);
        assert_eq!(out["candidates"][0]["row"]["id"], "cccccccccc");
        assert_eq!(out["candidates"][0]["marker"], "not started");
    }
}
