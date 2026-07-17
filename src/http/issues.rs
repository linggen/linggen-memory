//! Review-queue issues — the audit stage's holding pen (see
//! `../../linggen/doc/memory-spec.md`).
//!
//! The dream audit queues what it cannot solve with confidence: uncertain
//! chain merges, stale status claims, contradictions touching user-voice
//! rows. This daemon only bookkeeps — no LLM lives here. Resolution happens
//! where intelligence does: a host agent (`/linggen:solve` on Claude Code,
//! the memory app's Review inbox) reads the facts, gathers evidence, asks
//! the user when needed, writes the fix through the normal memory verbs,
//! then marks the issue resolved.
//!
//! Three endpoints under `/api/memory/`:
//!
//! - **`issues`** — list (default `status=open`) + the open count.
//! - **`issue_add`** — queue one item. Idempotent per `(kind, row_ids)`:
//!   the nightly audit re-detects the same suspects until they are fixed,
//!   and re-queuing must not grow the list.
//! - **`issue_resolve`** — close one item by id (`resolved` | `dismissed`).
//!
//! State is a JSON sidecar at `<data_dir>/memory/.issues.json` (same
//! pattern as `.days.json`) — not a LanceDB table, the frozen store schema
//! is untouched.

use super::envelope::{ok, ApiError};
use super::state::SharedState;
use crate::memory::types::short_id;
use axum::extract::State;
use axum::response::Response;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

pub fn router() -> Router<SharedState> {
    use axum::routing::post;
    Router::new()
        .route("/api/memory/issues", post(issues))
        .route("/api/memory/issue_add", post(issue_add))
        .route("/api/memory/issue_resolve", post(issue_resolve))
}

// ── Issues sidecar ──────────────────────────────────────────────────────────

const KINDS: &[&str] = &["chain", "stale-status", "contradiction"];
const OUTCOMES: &[&str] = &["resolved", "dismissed"];

/// One queued review item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueRecord {
    pub id: String,
    /// What the audit saw: `chain` (uncertain merge candidate),
    /// `stale-status` (a status claim older than the world), or
    /// `contradiction` (conflicting rows needing the user's pick).
    pub kind: String,
    /// The memory row ids this item is about.
    #[serde(default)]
    pub row_ids: Vec<String>,
    /// What the finder saw / proposes — the fact a solver starts from.
    pub note: String,
    pub created_at: DateTime<Utc>,
    /// `open` | `resolved` | `dismissed`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resolved_at: Option<DateTime<Utc>>,
    /// Solver's one-line record of what was done.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resolution: Option<String>,
}

/// The `.issues.json` sidecar — `{"issues": [record, …]}` in queue order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssuesState {
    #[serde(default)]
    pub issues: Vec<IssueRecord>,
}

fn issues_path(data_dir: &Path) -> PathBuf {
    data_dir.join("memory").join(".issues.json")
}

/// Read-or-default, mirroring `days::load` — a missing/corrupt file never
/// blocks the daemon.
pub(crate) async fn load(data_dir: &Path) -> IssuesState {
    let Ok(bytes) = tokio::fs::read(issues_path(data_dir)).await else {
        return IssuesState::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

async fn save(data_dir: &Path, state: &IssuesState) -> Result<(), std::io::Error> {
    let path = issues_path(data_dir);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(state).expect("IssuesState serializes");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(tmp, path).await?;
    Ok(())
}

/// Open-item count for the stats/days rollups.
pub(crate) async fn open_count(data_dir: &Path) -> usize {
    load(data_dir).await.issues.iter().filter(|i| i.status == "open").count()
}

// ── Handlers ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct IssuesRequest {
    /// `open` (default) | `resolved` | `dismissed` | `all`.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// POST /api/memory/issues — list the queue.
async fn issues(
    State(state): State<SharedState>,
    Json(req): Json<IssuesRequest>,
) -> Result<Response, ApiError> {
    let want = req.status.as_deref().unwrap_or("open");
    if !matches!(want, "open" | "resolved" | "dismissed" | "all") {
        return Err(ApiError::bad_request(format!(
            "invalid status {want:?}: expected open | resolved | dismissed | all"
        )));
    }
    let all = load(&state.data_dir).await.issues;
    let open = all.iter().filter(|i| i.status == "open").count();
    let limit = req.limit.unwrap_or(50);
    let items: Vec<&IssueRecord> = all
        .iter()
        .filter(|i| want == "all" || i.status == want)
        .take(limit)
        .collect();
    Ok(ok(json!({ "open_count": open, "issues": items })))
}

#[derive(Debug, Deserialize)]
struct IssueAddRequest {
    kind: String,
    #[serde(default)]
    row_ids: Vec<String>,
    note: String,
}

/// POST /api/memory/issue_add — queue one review item. Re-adding an open
/// item with the same `(kind, row_ids)` returns the existing record — the
/// nightly audit re-detects unfixed suspects and must not grow the queue.
async fn issue_add(
    State(state): State<SharedState>,
    Json(req): Json<IssueAddRequest>,
) -> Result<Response, ApiError> {
    let kind = req.kind.trim();
    if !KINDS.contains(&kind) {
        return Err(ApiError::bad_request(format!(
            "invalid kind {kind:?}: expected one of {KINDS:?}"
        )));
    }
    if req.note.trim().is_empty() {
        return Err(ApiError::bad_request("note is required".to_string()));
    }
    let mut rows = req.row_ids.clone();
    rows.sort();
    let mut all = load(&state.data_dir).await;
    if let Some(existing) = all.issues.iter().find(|i| {
        i.status == "open" && i.kind == kind && {
            let mut theirs = i.row_ids.clone();
            theirs.sort();
            theirs == rows
        }
    }) {
        return Ok(ok(json!({ "issue": existing, "deduped": true })));
    }
    let record = IssueRecord {
        id: short_id(),
        kind: kind.to_string(),
        row_ids: req.row_ids,
        note: req.note.trim().to_string(),
        created_at: Utc::now(),
        status: "open".to_string(),
        resolved_at: None,
        resolution: None,
    };
    all.issues.push(record.clone());
    save(&state.data_dir, &all)
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("persisting .issues.json: {e}")))?;
    Ok(ok(json!({ "issue": record, "deduped": false })))
}

#[derive(Debug, Deserialize)]
struct IssueResolveRequest {
    id: String,
    /// `resolved` | `dismissed`.
    outcome: String,
    /// Solver's one-line record of what was done.
    #[serde(default)]
    note: Option<String>,
}

/// POST /api/memory/issue_resolve — close one item by id. Closing an
/// already-closed item is success (idempotent for retrying agents).
async fn issue_resolve(
    State(state): State<SharedState>,
    Json(req): Json<IssueResolveRequest>,
) -> Result<Response, ApiError> {
    let outcome = req.outcome.trim();
    if !OUTCOMES.contains(&outcome) {
        return Err(ApiError::bad_request(format!(
            "invalid outcome {outcome:?}: expected resolved | dismissed"
        )));
    }
    let mut all = load(&state.data_dir).await;
    let Some(item) = all.issues.iter_mut().find(|i| i.id == req.id) else {
        return Err(ApiError::bad_request(format!("no issue with id {:?}", req.id)));
    };
    if item.status != "open" {
        let already = item.clone();
        return Ok(ok(json!({ "issue": already, "already_closed": true })));
    }
    item.status = outcome.to_string();
    item.resolved_at = Some(Utc::now());
    if let Some(note) = req.note.filter(|n| !n.trim().is_empty()) {
        item.resolution = Some(note.trim().to_string());
    }
    let updated = item.clone();
    save(&state.data_dir, &all)
        .await
        .map_err(|e| ApiError::internal(anyhow::anyhow!("persisting .issues.json: {e}")))?;
    Ok(ok(json!({ "issue": updated, "already_closed": false })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: &str, rows: &[&str], status: &str) -> IssueRecord {
        IssueRecord {
            id: short_id(),
            kind: kind.to_string(),
            row_ids: rows.iter().map(|s| s.to_string()).collect(),
            note: "n".to_string(),
            created_at: Utc::now(),
            status: status.to_string(),
            resolved_at: None,
            resolution: None,
        }
    }

    #[tokio::test]
    async fn sidecar_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = IssuesState::default();
        state.issues.push(record("chain", &["a", "b"], "open"));
        save(dir.path(), &state).await.unwrap();
        let loaded = load(dir.path()).await;
        assert_eq!(loaded.issues.len(), 1);
        assert_eq!(loaded.issues[0].kind, "chain");
        assert_eq!(open_count(dir.path()).await, 1);
    }

    #[tokio::test]
    async fn missing_sidecar_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).await.issues.is_empty());
        assert_eq!(open_count(dir.path()).await, 0);
    }

    #[tokio::test]
    async fn open_count_ignores_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = IssuesState::default();
        state.issues.push(record("chain", &["a"], "open"));
        state.issues.push(record("stale-status", &["b"], "resolved"));
        state.issues.push(record("contradiction", &["c"], "dismissed"));
        save(dir.path(), &state).await.unwrap();
        assert_eq!(open_count(dir.path()).await, 1);
    }
}
