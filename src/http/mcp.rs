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
const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Always-on primer injected into the client's system prompt at session
/// start. The MCP spec's `instructions` field is the daemon's way to teach
/// every connected host (Claude Code, Codex, Cursor, …) the ling-mem
/// doctrine without per-host CLAUDE.md edits. Keep it tight — each token
/// here costs every session.
const INSTRUCTIONS: &str = r#"ling-mem provides durable cross-session memory for this user — shared across Claude Code, Codex, OpenClaw, and Linggen. Memory is how the agent grows up: a fact earns its place only if a future session would make better predictions because the fact exists. Focus on the user, not the task.

# The three tiers

- **core** — narrow universals about the *person*: name, role, location, timezone, languages, family / pets. Always-loaded at session start. Keep tight.
- **semantic** (default) — durable long-term facts retrieved on demand: long-term goals / vision, cross-project preferences, decisions whose reasoning is the value, cross-project tech gotchas. **State + lessons, never events** — test: strip the date and the commit hash; still useful in three months? If not, episodic.
- **episodic** — per-turn working capture (your steady-state lane). Append anything that *might* matter — **including project-scoped milestones, decisions + reasoning, and run learnings**. Per-event work rows ("committed X", "pushed Y") land here **always**, however salient the turn felt. Fast, append-only, **no search-first**. Episodic is short-term memory: the nightly dream pass *remembers* each day (promotes durable rows to semantic/core, deletes nothing), and the forget sweep ages out judged rows after the TTL. This is the lane now that the every-N-turns encoder subagent is retired.

# When to SEARCH (before answering)

Call Memory_search when the user's question could connect to past preferences, decisions, or gotchas. Chip every fact you actually use: "From memory: …".

# When to SAVE (call Memory_add)

**Per-turn capture → episodic.** Each turn, append genuinely-noteworthy signal to `tier=episodic` — fast, no search-first, no confirmation. **Anchor relative time before writing** (substitution against today's date, not math — e.g. if today is 2026-07-07, "turned 3 last month" → "turned 3 in 2026-06, as of 2026-07-07"): "yesterday"/"last month"/"recently" in stored content rots silently. **Project-scoped is fine; episodic is staging, not user-biography.** Capture: shipped milestones, decisions + *why*, non-obvious learnings from a run/experiment. E.g. "Shipped Linggen 1.0"; "Sanji docking: dropped dock-wall cost, treat all cost-points uniformly"; "BlueBoat cruise tops out ~0.2 m/s". If a future session would be smarter for it, stage it — the dream pass dedupes and promotes.

**Curated writes → core / semantic** (high confidence) follow the read-before-write rule: **Always Memory_search the candidate content before a core/semantic Memory_add.** Write-time dedup is cheaper than read-time cleanup. Merge authority follows voice:
- Near-duplicate exists → skip the add; if yours is better-phrased and every matching row is your own note (`from=derived` — built/fixed/tried/learned), write the merged row with `replace_ids` listing the losers (one atomic call).
- Conflict among **your own notes only** → no ask needed: merge to one current-truth row via `replace_ids`.
- Conflict touching the **user's voice** (`from=user` — preference/decision/identity) → ask via the host's ask-user primitive, then write the winner with `replace_ids` carrying every loser AND `user_directed: true`. Do not write on top of a conflict; never separate add + delete. The daemon mechanically BLOCKS a replace/rewrite of `from=user` rows without `user_directed: true` — the flag asserts the user directed the change: their CURRENT message states it as settled (a command "update X to Y", a declaration "my X is now Y", a commitment "from now on, X"), or they just answered your ask. A hedged reflection ("X feels about right to me", "I think I prefer X") does NOT qualify — that's a contradiction: ask first. Never assert the flag from your own inference.

HIGH-SIGNAL — promote straight to core/semantic (search-first), don't leave these in episodic:
- Name + relationship ("my cat <name>", "my wife <name>") → tier=core, type=fact
- Location / timezone → tier=core, type=fact
- Role / identity ("I'm a robotics engineer") → tier=core, type=fact
- Long-term goal ("I'm building X") → default tier, type=fact, tag intent:goal
- Commitment ("always X", "never Y", "from now on Z") → tier=core, type=preference
- Cross-project tech gotcha that will recur → default tier, type=learned

Explicit imperatives — act immediately:
- "remember X" → save, reply "Saved."
- "forget X" → search + delete, reply "Deleted: …"
- "update X to Y" → search + edit, reply "Updated."

# When NOT to save

- **Never, any tier:** secrets (credentials, tokens, keys); content verbatim re-derivable from a file the agent re-reads (store the *decision/learning about* it, not the file body).
- **Keep out of core/semantic — episodic is fine:** project-internal facts, raw activity logs, single architectural calls, opinions without commitment. These stage in episodic; the dream pass decides if any earn a curated row.

# Memory hygiene — see it, solve it

Whoever surfaces garbage owns it in that moment; there is no cleanup queue. Authority follows voice:

- **Your own notes** (`from=derived` — built/fixed/tried/learned) are your notebook: merge, rewrite, retire freely — one `Memory_add` of the current-truth row with `replace_ids` listing every loser (atomic insert + delete; never separate add then delete).
- **The user's voice** (`from=user` — preference/decision/identity) changes only with the user: ask via the host's ask-user primitive (Claude Code: AskUserQuestion; Linggen: AskUser; Codex/OpenClaw: plain chat with numbered options), then write the winner with `replace_ids` + `user_directed: true` (the daemon blocks user-voice replaces without it). Can't ask / not material to the turn → append and leave both; recall keeps surfacing them until a user-present moment resolves it.

Taxonomy: exact dup → delete; superseded / chain member (derived) → `replace_ids` merge; reworded derived near-dup → merge, keep the best phrasing; old pure-event row → retire, folding into a state row if one exists; user-voice contradiction → ask; secret → delete on sight.

Inline reconciliation fires on **incidental** recall hits only. When the user is explicitly steering memory ("clean up", "remember X", "what's in memory", "ignore those hits"), follow their direction — do not side-quest dedup.

# Tool gotchas — CRITICAL

For Memory_search / Memory_list: do NOT pass `type`, `from`, or `outcome` unless the user explicitly asked. Models hallucinate these defaults and over-constrain queries to 0 rows. Pass only the query (and `limit` if relevant)."#;

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
        Err(err)  => json!({ "jsonrpc": "2.0", "id": id, "error": err }),
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
                    "limit":    {"type": "integer", "description": "Max rows. Default 10."}
                },
                "required": ["query"]
            }
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
                    "tier":     {"type": "string", "enum": ["core", "semantic", "episodic"], "description": "Destination tier. `episodic` = per-turn working capture (fast, append-only, no search-first; the dream pass remembers each day and the sweep forgets judged rows past TTL) — the default lane for uncertain-durability signal. `semantic` = curated durable facts (search-first). `core` = tiny always-injected universals about the person (search-first)."},
                    "contexts": {"type": "array", "items": {"type": "string"}},
                    "host":     {"type": "string", "description": "Identify the calling host (e.g. claude-code, codex, cursor). Stamped on the row for cross-host attribution. Optional."},
                    "source_session": {"type": "string", "description": "Session id that authored this content — pass your host session id on live captures (the recall hook prints it each turn). Makes scan's skip-by-session idempotency real: scanned sessions that already contributed rows are skipped."},
                    "replace_ids": {"type": "array", "items": {"type": "string"}, "description": "Row ids this new row replaces — the daemon inserts the row and deletes every listed loser atomically. Use for merges of your own derived notes and for AskUser-resolved conflicts; never separate add + delete calls."},
                    "user_directed": {"type": "boolean", "description": "Assert the user directed this change: their CURRENT message states it as SETTLED (a command \"update X to Y\", a declaration \"my X is now Y\", a commitment \"from now on, X\"), or they just answered your ask. Required when replace_ids targets from=user rows — the daemon BLOCKS such writes otherwise. A hedged reflection (\"X feels about right to me\") does NOT qualify: ask first. Never assert from your own inference."}
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
            "description": "Per-day dream-state rollup: each day's episodic row counts + pipeline state (today / staging / pending / harvested / remembered / forgotten). Use pending_only to get the dream worklist — days awaiting a remember pass, oldest first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pending_only": {"type": "boolean", "description": "Only days awaiting a remember pass."},
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
        Ok(data) => Ok(mcp_text_content(&data)),
        Err(err) => Err(rpc_error(-32603, err.message)),
    }
}

/// Map MCP tool name → daemon endpoint suffix.
fn tool_name_to_verb(name: &str) -> Option<&'static str> {
    match name {
        "memory_search"       => Some("search"),
        "memory_list"         => Some("list"),
        "memory_get"          => Some("get"),
        "memory_add"          => Some("add"),
        "memory_update"       => Some("update"),
        "memory_delete"       => Some("delete"),
        "memory_days"         => Some("days"),
        "memory_remember_day" => Some("remember_day"),
        "memory_harvest_day"  => Some("harvest_day"),
        "memory_sweep"        => Some("sweep"),
        "memory_chains"       => Some("chains"),
        _ => None,
    }
}

/// Dispatch-boundary fixes ported from `linggen/src/engine/tools/memory_tool.rs`.
/// These compensate for shapes models commonly produce that the daemon
/// can't or shouldn't interpret as-is.
fn apply_dispatch_fixes(verb: &str, args: &mut Value) {
    let Some(obj) = args.as_object_mut() else { return };

    // 1. Drop soft-empty fields. `until: ""` would crash the RFC-3339
    //    parser; empty arrays narrow unintentionally; nulls are noise.
    obj.retain(|_, v| match v {
        Value::String(s) => !s.is_empty(),
        Value::Array(a)  => !a.is_empty(),
        Value::Null      => false,
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
    if verb == "list" {
        let is_ttl_sweep = obj.get("past_ttl").and_then(|v| v.as_bool()).unwrap_or(false);
        if is_ttl_sweep {
            for k in ["type", "from", "outcome"] {
                let _ = obj.remove(k);
            }
        }
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
        .map_err(|e| ApiError::internal(anyhow::Error::from(e)))?;
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
