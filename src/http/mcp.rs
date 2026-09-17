//! MCP (Model Context Protocol) Streamable-HTTP endpoint.
//!
//! Exposes the daemon's memory operations as MCP tools so Claude Code,
//! Codex, Cursor, and any MCP client can register `ling-mem` as a
//! provider via `.mcp.json` without spawning a stdio subprocess.
//!
//! Single endpoint at `POST /mcp` accepting JSON-RPC 2.0. Methods:
//! - `initialize` — protocol handshake, returns server capabilities.
//! - `tools/list` — enumerate tools with their input schemas.
//! - `tools/call` — execute a tool. The handler loopback-POSTs to
//!   `http://127.0.0.1:<self_port>/api/memory/<verb>` so MCP calls share
//!   the same dispatch path as the CLI client and direct HTTP callers.
//!
//! Notifications (no `id`, no response expected) return HTTP 204.
//!
//! Tool schemas are deliberately narrow — only fields each verb actually
//! consumes — to defend against models that over-fill optional fields
//! with hallucinated defaults (see the past_ttl-sweep guard below and
//! the gpt-5.5 case in DESIGN.md).

use super::envelope::ApiError;
use super::state::SharedState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::time::Duration;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "ling-mem";
/// How long a tool call waits on the daemon's own HTTP API. A write can pay
/// for a LanceDB auto-cleanup inside its commit (9s once, 2026-09-15), so
/// 10s cut off adds that then landed anyway. Kept under the 30s the engine
/// and the CLI wait, so the caller hears this daemon's own timeout message.
const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(25);

/// Always-on primer injected into the client's system prompt at session
/// start. The MCP spec's `instructions` field is the daemon's way to teach
/// every connected host (Claude Code, Codex, Cursor, …) the ling-mem
/// doctrine without per-host CLAUDE.md edits.
///
/// HARD CAP: 2048 characters. Claude Code cuts server instructions there and
/// drops the rest silently — the old 9356-character protocol reached CC
/// sessions only through its tier definitions, so the routing rules (role →
/// core, search first, the merge law) never arrived. Only rules that decide
/// a save live here; the long form, with examples and the cleanup taxonomy,
/// is the memory skill. `instructions_fit_claude_codes_cap` holds the line.
const INSTRUCTIONS: &str = r#"ling-mem is this user's memory, shared by Claude Code, Codex, OpenClaw and Linggen. Save what makes a future session predict the user better — the person, not the task.

# Tiers
- core — who they are, always loaded: name, role/job, location, timezone, languages, family/pets. One short fact per row.
- semantic — durable, recalled on demand: goals, preferences ("always X / never Y" → semantic, type=preference, from=user; never core), decisions with their why, cross-project gotchas. State, not events: still useful in 3 months without the date?
- episodic — the default per-turn capture: milestones, events, project decisions, run learnings. No search first. The nightly dream promotes what lasts.

# Saving
- Before any core/semantic add, memory_search the subject. Same subject or same kind of claim in other words = conflict; when unsure, treat it as one.
- Your own rows (from=derived): merge into one current-truth row with memory_add + replace_ids. Never add then delete.
- The user's rows (from=user): ask first, showing each row's full text and date; then replace_ids + user_directed:true. Never set that flag from your own inference.
- A replacement keeps the loser's tier. A new status (shipped/fixed/dropped) replaces the old status row.
- "remember / forget / update X", in any language: search, act, user_directed:true.
- Anchor relative time to dates ("last month" → "2026-08").
- Never save secrets or file bodies you can re-read. Project internals stay episodic.
- Garbage you come across in your own rows, fix on sight.

# Recall
Search when the question may touch past preferences, decisions or gotchas. Show each fact you use: "From memory (3 months ago): …". Never pass type, from or outcome to memory_search unless asked.

Full rules and examples: the memory skill (linggen / shared-memory)."#;

pub fn router() -> Router<SharedState> {
    Router::new().route("/mcp", post(handler))
}

// ── JSON-RPC dispatch ───────────────────────────────────────────────────────

/// Single entrypoint. Parses the JSON-RPC envelope, routes by `method`,
/// and returns either a JSON-RPC response (200) or 204 for notifications.
async fn handler(State(state): State<SharedState>, Json(req): Json<Value>) -> Response {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    // Notifications: no `id` → no response, just 204. The most common is
    // `notifications/initialized` from the client after the handshake.
    if id.is_none() {
        return StatusCode::NO_CONTENT.into_response();
    }

    let result = match method {
        "initialize" => Ok(handle_initialize()),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(&state, params).await,
        other => Err(rpc_error(-32601, format!("method not found: {other}"))),
    };

    let envelope = match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(err) => json!({ "jsonrpc": "2.0", "id": id, "error": err }),
    };
    Json(envelope).into_response()
}

fn rpc_error(code: i32, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

// ── initialize ──────────────────────────────────────────────────────────────

fn handle_initialize() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

// ── tools/list ──────────────────────────────────────────────────────────────

fn handle_tools_list() -> Value {
    json!({ "tools": tool_defs() })
}

/// Narrow schemas — only fields the corresponding `/api/memory/<verb>`
/// handler actually consumes for that operation. Speculative fields
/// (`type`, `from`, `outcome` on a sweep query, etc.) are deliberately
/// omitted from the schema so models can't over-fill them.
fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "memory_search",
            "description": "Semantic search over user-biography memory. Returns rows ranked by relevance to the query. Memory holds cross-session identity, preferences, and decisions — not codebase facts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query":    {"type": "string", "description": "Natural-language description of what to find."},
                    "contexts": {"type": "array", "items": {"type": "string"}, "description": "Filter to these scope tags (AND). Omit to search globally."},
                    "tier":     {"type": "string", "enum": ["core", "semantic", "episodic"], "description": "Restrict to one tier. Omit to span all."},
                    "cwd_scope": {"type": "string", "description": "HOST-FILLED — leave it out; the host stamps the session cwd to scope results to this project plus project-free rows."},
                    "limit":    {"type": "integer", "description": "Max rows. Default 10."}
                },
                "required": ["query"]
            }
            // `min_score` is deliberately absent. The REST handler accepts it
            // and the loopback passes it through, so a *programmatic* caller
            // (Linggen's auto-recall) can set a floor — but advertising a
            // relevance threshold to a model invites exactly the over-fill
            // failure these narrow schemas exist to prevent: a guessed value
            // silently narrows recall to zero rows. Omitted here, the daemon's
            // store-wide `recall_min_score` applies.
        }),
        json!({
            "name": "memory_list",
            "description": "Filter-only browse (no relevance ranking). For audits, sweeps, or showing recent rows. Pass minimal filters — narrow schemas avoid the over-constrain-to-zero failure.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "contexts": {"type": "array", "items": {"type": "string"}},
                    "tier":     {"type": "string", "enum": ["core", "semantic", "episodic"]},
                    "past_ttl": {"type": "boolean", "description": "Return only rows past the configured episodic TTL. Implies tier=episodic."},
                    "day":      {"type": "string", "description": "One local calendar day, YYYY-MM-DD — the remember stage lists a single day's worklist with this."},
                    "unjudged": {"type": "boolean", "description": "With day: only the rows no remember pass has judged yet — a re-opened day's new rows, not the whole day."},
                    "sort":     {"type": "string", "enum": ["newest", "oldest"]},
                    "limit":    {"type": "integer"},
                    "offset":   {"type": "integer"}
                }
            }
        }),
        json!({
            "name": "memory_get",
            "description": "Fetch one row by id.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": {"type": "string"} },
                "required": ["id"]
            }
        }),
        json!({
            "name": "memory_add",
            "description": "Insert a new memory row. Only durable, cross-session signal — identity facts, behavioural preferences, decisions with their reasoning. NOT project-internal architecture or implementation detail.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content":  {"type": "string", "description": "The fact text the model will see when this row is recalled."},
                    "type":     {"type": "string", "enum": ["fact", "preference", "decision", "tried", "fixed", "learned", "built"]},
                    "tier":     {"type": "string", "enum": ["core", "semantic", "episodic"], "description": "Destination tier: episodic = per-turn staging (default, no search-first); semantic = curated durable facts (search-first); core = tiny always-injected universals (search-first). Doctrine in the server instructions."},
                    "contexts": {"type": "array", "items": {"type": "string"}},
                    "host":     {"type": "string", "description": "HOST-FILLED — leave it out; the host stamps the committing runtime."},
                    "source_session": {"type": "string", "description": "HOST-FILLED — leave it out; pass only when a promote pass carries the original row's session forward."},
                    "cwd":      {"type": "string", "description": "HOST-FILLED — leave it out; the host stamps the session's own cwd, which scopes recall."},
                    "replace_ids": {"type": "array", "items": {"type": "string"}, "description": "Row ids this row replaces — inserted and deleted atomically. For merges and resolved conflicts; never separate add + delete calls."},
                    "user_directed": {"type": "boolean", "description": "Assert the user directed this change (a settled command/declaration, or they just answered your ask). Required when replace_ids targets from=user rows — the daemon BLOCKS such writes otherwise. Hedged reflections don't qualify; see the server instructions."}
                },
                "required": ["content"]
            }
        }),
        json!({
            "name": "memory_delete",
            "description": "Hard-delete one row by id. For explicit user forgets, exact duplicates, and secrets. For conflict or merge resolution prefer memory_add with replace_ids — one atomic call, never add + delete.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": {"type": "string"} },
                "required": ["id"]
            }
        }),
        json!({
            "name": "memory_update",
            "description": "Edit one row in place by id (content, type, tier, contexts, tags). A tier change moves the row across tables (episodic ↔ semantic/core) keeping its id. Rewriting content on a from=user row requires user_directed (same floor as memory_add's replace_ids). For merging MULTIPLE rows prefer memory_add with replace_ids.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id":       {"type": "string"},
                    "content":  {"type": "string", "description": "Replacement fact text."},
                    "type":     {"type": "string", "enum": ["fact", "preference", "decision", "tried", "fixed", "learned", "built"]},
                    "tier":     {"type": "string", "enum": ["core", "semantic", "episodic"], "description": "Moving tier relocates the row across tables, id preserved."},
                    "contexts": {"type": "array", "items": {"type": "string"}},
                    "tags":     {"type": "array", "items": {"type": "string"}},
                    "user_directed": {"type": "boolean", "description": "Required when rewriting content on a from=user row — see memory_add.user_directed."}
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "memory_harvest_day",
            "description": "Stamp a day harvested after a scan/backfill pass staged its episodic rows (does NOT mark it remembered — the dream's remember pass still judges it). Only past local days.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "date": {"type": "string", "description": "Local calendar day, YYYY-MM-DD."}
                },
                "required": ["date"]
            }
        }),
        json!({
            "name": "memory_days",
            "description": "Per-day dream-state rollup: each day's episodic row counts + per-verb flags (scanned = a scan walked its session logs; dreamed = a dream pass judged it, late rows clear the flag). Top level carries first_unscanned / first_undreamed / open_issues plus past-day summary counts (total_days / scanned_days / dreamed_days). Use undreamed_only to get the dream worklist — days awaiting a dream pass, oldest first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "undreamed_only": {"type": "boolean", "description": "Only days awaiting a dream pass (the worklist)."},
                    "from": {"type": "string", "description": "Inclusive YYYY-MM-DD lower bound."},
                    "to":   {"type": "string", "description": "Inclusive YYYY-MM-DD upper bound."}
                }
            }
        }),
        json!({
            "name": "memory_remember_day",
            "description": "Stamp a day remembered after judging its episodic rows (promote the durable ones first via memory_add, then stamp). Counts accumulate. Only past local days can be stamped.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "date":     {"type": "string", "description": "Local calendar day, YYYY-MM-DD."},
                    "judged":   {"type": "integer", "description": "Rows judged in this pass."},
                    "promoted": {"type": "integer", "description": "Rows promoted to semantic in this pass."}
                },
                "required": ["date"]
            }
        }),
        json!({
            "name": "memory_sweep",
            "description": "Forget sweep — mechanically evict episodic rows that are past TTL, belong to a remembered day, and were judged (created before the day's remembered_at). Never deletes un-judged rows; safe to call anytime.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "dry_run": {"type": "boolean", "description": "Report what would be evicted without deleting."}
                }
            }
        }),
        json!({
            "name": "memory_issues",
            "description": "Review queue — items the dream audit could not solve with confidence (uncertain merges, stale status claims, user-voice contradictions, doubtful digest clusters). Returns the facts only; YOU are the solver: solve each item yourself first — gather evidence (full rows, git history, code, docs) until it settles the question — and write the fix via memory_add + replace_ids, then close the item with memory_issue_resolve. Ask the user only when evidence cannot settle it or a user-voice row is involved: ONE item at a time, phrased as a simple fact question in plain words with your recommendation — no row ids, commit hashes, or cluster jargon.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {"type": "string", "enum": ["open", "resolved", "dismissed", "all"], "description": "Which items to list (default open)."},
                    "limit":  {"type": "integer", "description": "Max items (default 50)."}
                }
            }
        }),
        json!({
            "name": "memory_issue_add",
            "description": "Queue one review-queue item the audit could not solve with confidence. Idempotent per (kind, row_ids) — the nightly audit re-detects the same suspects until they are fixed, and re-queuing returns the existing item rather than growing the list. Prefer solving it now: queue only what genuinely needs the user, or evidence this pass cannot gather.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind":    {"type": "string", "enum": ["chain", "stale-status", "contradiction", "subject"], "description": "What you saw: `chain` = uncertain merge candidate; `stale-status` = a status claim likely overtaken by the world (verify against git/files at solve time); `contradiction` = conflicting rows needing the user's pick; `subject` = digest cluster of doubtful subject coherence (list ALL member ids)."},
                    "row_ids": {"type": "array", "items": {"type": "string"}, "description": "The memory row ids this item is about."},
                    "note":    {"type": "string", "description": "What you saw and what a solver should check — the item's whole context, since the solver starts from this line alone. Write it in plain words; it may become the user's question."}
                },
                "required": ["kind", "note"]
            }
        }),
        json!({
            "name": "memory_issue_resolve",
            "description": "Close one review-queue item by id after solving it (outcome=resolved) or deciding it isn't worth fixing (outcome=dismissed). Pass a one-line note of what was done. Closing an already-closed item is a no-op success.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id":      {"type": "string", "description": "The issue id from memory_issues."},
                    "outcome": {"type": "string", "enum": ["resolved", "dismissed"]},
                    "note":    {"type": "string", "description": "One-line record of what was done."}
                },
                "required": ["id", "outcome"]
            }
        }),
        json!({
            "name": "memory_chains",
            "description": "Condense scan — mechanical, read-only detection of stale same-subject clusters in long-term memory. kind=cited: rows citing another row's id verbatim, grouped into chains (auto-accept quality). kind=marker: rows with provisional-state language (\"OPEN:\", \"uncommitted\", …) plus nearest-neighbor rows — confirm a real supersession before merging. kind=subject: same-subject vector clusters (3+ rows) for the digest pass — condense into ONE focused per-subject row, never a mega state row. Each cluster carries derived_only: merge unattended ONLY when true (the merge law); user-voice clusters need the user. Apply merges via memory_add with replace_ids.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind":   {"type": "string", "enum": ["cited", "marker", "subject"], "description": "cited (default) = id-citation chains; marker = provisional-state candidates; subject = same-subject clusters for digests."},
                    "limit":  {"type": "integer", "description": "Clusters per page (default 10)."},
                    "offset": {"type": "integer", "description": "Pagination offset over the cluster list."},
                    "derived_only": {"type": "boolean", "description": "Only clusters mergeable unattended (every row from=derived, tier=semantic). Unattended condense passes MUST set true."}
                }
            }
        }),
    ]
}

// ── tools/call ──────────────────────────────────────────────────────────────

async fn handle_tools_call(state: &SharedState, params: Value) -> Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| rpc_error(-32602, "tools/call requires `name`"))?
        .to_string();
    let mut args = params.get("arguments").cloned().unwrap_or(json!({}));

    let verb = tool_name_to_verb(&name)
        .ok_or_else(|| rpc_error(-32602, format!("unknown tool: {name}")))?;

    apply_dispatch_fixes(verb, &mut args);

    match loopback(state, verb, args).await {
        Ok(mut data) => {
            apply_response_fixes(verb, &mut data);
            Ok(mcp_text_content(&data))
        }
        Err(err) => Err(rpc_error(-32603, err.message)),
    }
}

/// Map MCP tool name → daemon endpoint suffix.
fn tool_name_to_verb(name: &str) -> Option<&'static str> {
    match name {
        "memory_search" => Some("search"),
        "memory_list" => Some("list"),
        "memory_get" => Some("get"),
        "memory_add" => Some("add"),
        "memory_update" => Some("update"),
        "memory_delete" => Some("delete"),
        "memory_days" => Some("days"),
        "memory_remember_day" => Some("remember_day"),
        "memory_harvest_day" => Some("harvest_day"),
        "memory_sweep" => Some("sweep"),
        "memory_chains" => Some("chains"),
        "memory_issues" => Some("issues"),
        "memory_issue_add" => Some("issue_add"),
        "memory_issue_resolve" => Some("issue_resolve"),
        _ => None,
    }
}

/// Dispatch-boundary fixes ported from `linggen/src/engine/tools/memory_tool.rs`.
/// These compensate for shapes models commonly produce that the daemon
/// can't or shouldn't interpret as-is.
fn apply_dispatch_fixes(verb: &str, args: &mut Value) {
    let Some(obj) = args.as_object_mut() else {
        return;
    };

    // 1. Drop soft-empty fields. `until: ""` would crash the RFC-3339
    //    parser; empty arrays narrow unintentionally; nulls are noise.
    obj.retain(|_, v| match v {
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Null => false,
        _ => true,
    });

    // 2. `tier=episodic` → `episodic=true` on the wire (the daemon splits
    //    the episodic store into a separate table). Other tier values stay
    //    as filters within the semantic store.
    if let Some(tier) = obj.get("tier").and_then(|v| v.as_str()) {
        if tier == "episodic" {
            obj.insert("episodic".to_string(), Value::Bool(true));
            obj.remove("tier");
        }
    }

    // 3. `past_ttl=true` is a bulk-eviction sweep — it wants every past-TTL
    //    episodic row regardless of `type`/`from`/`outcome`. gpt-5.5 has
    //    been observed to over-fill these with hallucinated defaults
    //    (`type=fact, from=user, outcome=positive`) which narrows the
    //    sweep to zero rows. Strip them at the dispatch boundary; the
    //    schema doesn't expose them on list, but defend defensively.
    //    Not gated on `verb == "list"`: `past_ttl` is only meaningful on a
    //    sweep, so stripping it wherever it appears costs nothing and covers
    //    a model that reaches for the sweep shape on the wrong verb.
    let is_ttl_sweep = obj
        .get("past_ttl")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if is_ttl_sweep {
        for k in ["type", "from", "outcome"] {
            let _ = obj.remove(k);
        }
    }
}

/// The mirror of `apply_dispatch_fixes` on the way back out: shapes the
/// daemon returns that a *model* reliably misreads.
///
/// This layer, not the REST handler, is the right home for them for the same
/// reason the request-side fixes live here — CLI arguments come from clap,
/// typed, from a human, and REST callers are programs. Only the MCP surface
/// is talking to something that infers intent from JSON it did not write.
fn apply_response_fixes(verb: &str, value: &mut Value) {
    if verb != "delete" {
        return;
    }
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    // Deleting an already-absent row is success, not an anomaly — the row is
    // gone either way (commonly this daemon's own cross-tier dedup removed
    // the episodic copy during a promote add). A bare `removed:false` reads
    // as an error signal to LLM callers: three dream runs were observed
    // aborting over it, claiming "store inconsistency". Say what it means.
    if obj.get("removed").and_then(|v| v.as_bool()) == Some(false) {
        obj.insert("already_gone".to_string(), Value::Bool(true));
        obj.insert(
            "note".to_string(),
            Value::String(
                "row was already absent — treat as success; do not retry or verify".to_string(),
            ),
        );
    }
}

/// Wrap a JSON value into MCP's `content` array. We always return a
/// single text item containing the serialized JSON — the model sees the
/// raw structured data and can reason about it directly.
fn mcp_text_content(data: &Value) -> Value {
    let text = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
    json!({
        "content": [
            { "type": "text", "text": text }
        ]
    })
}

/// HTTP loopback to this daemon's own `/api/memory/<verb>` handler.
/// Reuses the same dispatch path the CLI client uses so MCP and CLI
/// share behaviour. Unwraps the `{ok, data}` envelope.
/// A timeout says what it means for a write: the store may have finished the
/// work after we stopped waiting, so look before doing it again.
fn loopback_error(verb: &str, err: reqwest::Error) -> ApiError {
    if !err.is_timeout() {
        return ApiError::internal(anyhow::Error::from(err));
    }
    ApiError::internal(anyhow::anyhow!(
        "{verb} timed out after {}s — a write may still have landed; search before retrying",
        LOOPBACK_TIMEOUT.as_secs()
    ))
}

async fn loopback(state: &SharedState, verb: &str, body: Value) -> Result<Value, ApiError> {
    let url = format!("http://127.0.0.1:{}/api/memory/{}", state.port, verb);
    let client = reqwest::Client::builder()
        .timeout(LOOPBACK_TIMEOUT)
        .build()
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| loopback_error(verb, e))?;
    let value: Value = resp
        .json()
        .await
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
    if value.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(value.get("data").cloned().unwrap_or(Value::Null))
    } else {
        let msg = value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("upstream error")
            .to_string();
        Err(ApiError::internal(anyhow::anyhow!(msg)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store that takes the request and never answers: the timeout must
    /// tell the caller its write may have landed.
    #[tokio::test]
    async fn a_timed_out_call_says_the_write_may_have_landed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/memory/add", listener.local_addr().unwrap());
        let _held = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let err = reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap()
            .post(&url)
            .json(&json!({}))
            .send()
            .await
            .unwrap_err();
        let message = loopback_error("add", err).message;
        assert!(message.starts_with("add timed out after 25s"), "{message}");
        assert!(message.contains("may still have landed"), "{message}");
    }

    /// `until: ""` used to reach the RFC-3339 parser and crash it; empty
    /// arrays silently narrowed a query to nothing.
    #[test]
    fn soft_empty_fields_never_reach_the_daemon() {
        let mut args = json!({
            "query": "real",
            "until": "",
            "contexts": [],
            "tier": null,
            "limit": 5
        });
        apply_dispatch_fixes("search", &mut args);
        let obj = args.as_object().unwrap();
        assert_eq!(obj.keys().collect::<Vec<_>>(), ["limit", "query"]);
    }

    /// The episodic store is a separate table, so the tier is a routing
    /// decision on the wire, not a filter value.
    #[test]
    fn episodic_tier_becomes_a_routing_flag() {
        let mut args = json!({"content": "x", "tier": "episodic"});
        apply_dispatch_fixes("add", &mut args);
        assert_eq!(args["episodic"], json!(true));
        assert!(args.get("tier").is_none());

        // Any other tier stays a filter within the semantic store.
        let mut semantic = json!({"content": "x", "tier": "core"});
        apply_dispatch_fixes("add", &mut semantic);
        assert_eq!(semantic["tier"], json!("core"));
        assert!(semantic.get("episodic").is_none());
    }

    /// A sweep wants every past-TTL row. gpt-5.5 was observed filling these
    /// with hallucinated defaults, narrowing it to zero.
    #[test]
    fn a_ttl_sweep_is_not_narrowed_by_invented_filters() {
        let mut args = json!({
            "past_ttl": true,
            "type": "fact",
            "from": "user",
            "outcome": "positive",
            "contexts": ["keep-me"]
        });
        apply_dispatch_fixes("list", &mut args);
        assert!(args.get("type").is_none());
        assert!(args.get("from").is_none());
        assert!(args.get("outcome").is_none());
        // A caller legitimately scoping by tag keeps it.
        assert_eq!(args["contexts"], json!(["keep-me"]));

        // Not gated on the verb: the same shape on the wrong one is stripped.
        let mut wrong_verb = json!({"past_ttl": true, "type": "fact"});
        apply_dispatch_fixes("search", &mut wrong_verb);
        assert!(wrong_verb.get("type").is_none());
    }

    /// Three dream runs aborted claiming "store inconsistency" over a bare
    /// `removed:false`. The row is gone either way.
    #[test]
    fn deleting_an_absent_row_reads_as_success() {
        let mut resp = json!({"id": "abc", "removed": false});
        apply_response_fixes("delete", &mut resp);
        assert_eq!(resp["already_gone"], json!(true));
        assert!(resp["note"].as_str().unwrap().contains("treat as success"));

        // A real delete is left alone…
        let mut real = json!({"id": "abc", "removed": true});
        apply_response_fixes("delete", &mut real);
        assert!(real.get("already_gone").is_none());

        // …and so is every other verb's response.
        let mut search = json!({"removed": false});
        apply_response_fixes("search", &mut search);
        assert!(search.get("already_gone").is_none());
    }

    /// An advertised tool nobody can route is a phantom, and a routable verb
    /// nobody advertises is a capability the model cannot reach — which is
    /// exactly how `issue_add` went missing while every other verb had a
    /// tool. The two lists are one surface; test them as one.
    #[test]
    fn every_advertised_tool_routes_and_every_route_is_advertised() {
        let advertised: Vec<String> = tool_defs()
            .iter()
            .map(|t| t["name"].as_str().expect("a tool has a name").to_string())
            .collect();

        for name in &advertised {
            assert!(
                tool_name_to_verb(name).is_some(),
                "advertised tool `{name}` has no verb to route to"
            );
        }

        // Every verb the map knows must be advertised. Kept as the map's own
        // inverse rather than a hand-written list, so adding a route without
        // a schema fails here.
        for verb in [
            "search",
            "list",
            "get",
            "add",
            "update",
            "delete",
            "days",
            "remember_day",
            "harvest_day",
            "sweep",
            "chains",
            "issues",
            "issue_add",
            "issue_resolve",
        ] {
            let advertised_for_verb = advertised
                .iter()
                .any(|name| tool_name_to_verb(name) == Some(verb));
            assert!(
                advertised_for_verb,
                "verb `{verb}` is routable but not advertised"
            );
        }
    }

    /// The audit queues what it cannot solve; without this tool the whole
    /// review queue is write-only from a model's point of view.
    #[test]
    fn the_audit_can_queue_a_review_item() {
        assert_eq!(tool_name_to_verb("memory_issue_add"), Some("issue_add"));
        let def = tool_defs()
            .into_iter()
            .find(|t| t["name"] == json!("memory_issue_add"))
            .expect("memory_issue_add is advertised");
        // `kind` + `note` are what /api/memory/issue_add rejects a call for
        // missing, so they are the schema's required pair.
        assert_eq!(def["inputSchema"]["required"], json!(["kind", "note"]));
    }

    /// Claude Code cuts server instructions at 2048 characters and drops the
    /// rest without a word. Past the cap, the rules at the end are the ones
    /// a CC session never sees.
    #[test]
    fn instructions_fit_claude_codes_cap() {
        let n = INSTRUCTIONS.chars().count();
        assert!(
            n <= 2048,
            "instructions are {n} characters; Claude Code keeps 2048"
        );
    }

    /// The routing rule whose loss started this: a role reaches core.
    #[test]
    fn instructions_route_a_role_to_core() {
        let core = INSTRUCTIONS
            .lines()
            .find(|l| l.starts_with("- core"))
            .expect("a core tier line");
        assert!(core.contains("role/job"), "{core}");
    }
}
