//! `/api/memory/<method>` — RPC-style endpoints, 1:1 with the `Memory.*`
//! tools in Linggen. Each endpoint POSTs JSON args and returns
//! `{ok, data}` or `{ok:false, error, code}` via `envelope::ApiError`.
//!
//! Semantics mirror the CLI handlers in `crate::cli` — this module is
//! the network-facing path to the same `MemoryStore` operations. Once
//! Phase 4 lands in Linggen, the CLI data-ops wrappers are removed and
//! HTTP becomes the only dispatch path.

use super::envelope::{ok, ApiError};
use super::state::SharedState;
use crate::memory::{
    Filters, InsertOutcome, Memory, MemoryPatch, MemoryStore, MemoryType, Origin, Outcome,
    SortOrder, Tier,
};
use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, SubsecRound, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use std::sync::Arc;

/// Deserialize an `Option<T>` where empty strings, `null`, and missing
/// keys all collapse to `None`. Wraps any string-or-enum field that LLMs
/// commonly fill with `""` instead of omitting. Without this, a payload
/// like `{"type": "", "from": ""}` hits serde's enum parser and surfaces
/// as `422: premature end of input` — opaque, blocks the call.
fn deserialize_optional_lenient<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = serde_json::Value::deserialize(de)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(ref s) if s.trim().is_empty() => Ok(None),
        other => serde_json::from_value(other)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

/// Deserialize `Option<DateTime<Utc>>` while tolerating the shapes LLMs
/// commonly produce. Without this, chat-generated date strings hit
/// chrono's strict parser and surface as an opaque `422: premature end
/// of input`.
///
/// Accepts:
/// - Field omitted / `null` / `""` → `None`
/// - Full RFC-3339 (`"2026-04-27T16:00:00Z"`) → parsed
/// - Date-only (`"2026-04-27"`) → midnight UTC of that date
/// - Date + time without TZ (`"2026-04-27T16:00:00"`) → assumed UTC
fn deserialize_optional_datetime<'de, D>(de: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let s: Option<String> = Option::deserialize(de)?;
    let raw = match s.as_deref() {
        None | Some("") => return Ok(None),
        Some(s) => s.trim(),
    };
    if raw.is_empty() {
        return Ok(None);
    }

    // 1. Full RFC-3339 with timezone — the canonical form.
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(dt.with_timezone(&Utc)));
    }

    // 2. Date-only "YYYY-MM-DD" → midnight UTC.
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        if let Some(naive) = date.and_hms_opt(0, 0, 0) {
            return Ok(Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)));
        }
    }

    // 3. Date + time without timezone "YYYY-MM-DDTHH:MM:SS" → assume UTC.
    //    Some LLMs drop the trailing 'Z'. Accept it to avoid the 422.
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S") {
        return Ok(Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)));
    }

    Err(D::Error::custom(format!(
        "invalid timestamp {raw:?}: expected RFC-3339 (e.g. \"2026-04-27T16:00:00Z\") or date-only (\"2026-04-27\")"
    )))
}

/// Serialize a fact for HTTP response, stripping the 1024-dim embedding
/// vector. Callers never need the raw vector over the wire, and including
/// it bloats every response by ~13 KB / row (noisy for the model, for logs,
/// and for the data UI). The CLI's NDJSON output keeps vectors — they
/// matter for `add --stdin` round-trips.
fn fact_public(fact: &Memory) -> Value {
    let mut v = serde_json::to_value(fact).unwrap_or_else(|_| Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.remove("vector");
    }
    v
}

fn facts_public(facts: &[Memory]) -> Vec<Value> {
    facts.iter().map(fact_public).collect()
}

/// Like [`fact_public`] but adds the relevance fields for search responses.
/// Each input hit is `(memory, cosine, hybrid)`:
///
/// - `score` — the raw **cosine** similarity (`[0,1]`), the absolute
///   dense-relevance signal. Kept for the recall hook, CLI, and cross-host
///   comparisons that already read it.
/// - `hybrid_score` — the blended relevance (`cosine` + IDF-weighted keyword
///   boost, clamped to `[0,1]`; see [`crate::memory::hybrid`]). This is the
///   number the console displays: it is what the rows are ordered by, so it
///   is monotonic with rank, and it is absolute — an unrelated query does
///   not get a fake 1.0.
fn scored_facts_public(scored: &[(Memory, f32, f32)]) -> Vec<Value> {
    scored
        .iter()
        .map(|(f, cosine, hybrid)| {
            let mut v = fact_public(f);
            if let Some(obj) = v.as_object_mut() {
                obj.insert("score".into(), json!(cosine));
                obj.insert("hybrid_score".into(), json!(hybrid));
            }
            v
        })
        .collect()
}

/// Memory subrouter. Mounted at `/api/memory/` by the parent router.
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/memory/add", post(add))
        .route("/api/memory/add_batch", post(add_batch))
        .route("/api/memory/get", post(get))
        .route("/api/memory/search", post(search))
        .route("/api/memory/list", post(list))
        .route("/api/memory/count", post(count))
        .route("/api/memory/update", post(update))
        .route("/api/memory/delete", post(delete))
        .route("/api/memory/forget", post(forget))
}

// ── Request DTOs ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AddRequest {
    pub content: String,
    #[serde(default)]
    pub contexts: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub r#type: Option<MemoryType>,
    /// Origin. Canonical name is `from` (matches the `Memory` field);
    /// accept `origin` as an alias for callers that avoid reserved words.
    #[serde(
        default,
        alias = "origin",
        deserialize_with = "deserialize_optional_lenient"
    )]
    pub from: Option<Origin>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub outcome: Option<Outcome>,
    pub cwd: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_datetime")]
    pub occurred_at: Option<DateTime<Utc>>,
    pub source_session: Option<String>,
    /// Writing host identifier (`linggen`, `claude-code`, `codex`, …).
    /// Distinct from `from` (which captures whether the content came
    /// from `user` / `agent` / `derived`); this records which tool
    /// runtime committed the row. Optional — older callers omit it.
    pub host: Option<String>,
    /// Route the write to the **episodic** staging store instead of the
    /// default curated semantic table. Used by the per-session encoder
    /// subagent so episodic writes can flow through HTTP (no more direct
    /// LanceDB-via-CLI roundtrip from inside the engine).
    #[serde(default)]
    pub episodic: bool,
    /// Destination tier within the semantic table: `core` (always-on
    /// identity set) or `semantic` (default). `episodic` here is a
    /// dispatch-boundary alias for the `episodic` flag above (hosts pass
    /// `tier=episodic`; the engine converts, but direct callers may not).
    /// Before this field existed, `tier` in the body was silently
    /// dropped — `tier=core` over HTTP/MCP never worked.
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub tier: Option<Tier>,
    /// Bypass dedup: insert as a new row even if a near-duplicate exists.
    /// Accepts `skip_dedup` (canonical) or `force` (alias).
    #[serde(default, alias = "force")]
    pub skip_dedup: bool,
    /// Atomic contradiction-resolution helper. When set, the daemon
    /// inserts the new row AND deletes every id in this list in the same
    /// call — used by the agent after an `AskUser`-confirmed conflict
    /// resolution. Lookup spans both tables, so the caller doesn't need
    /// to know which tier each loser lives in. Empty / omitted = a plain
    /// add. Failed deletes are reported in the response under
    /// `replaced_failed` but never abort the insert.
    #[serde(default)]
    pub replace_ids: Vec<String>,
    /// Assert that the user directed this change — their current message
    /// states it as settled (a command, declaration, or commitment), or
    /// the host resolved the conflict with them via an ask. Required when
    /// `replace_ids` targets rows in the user's voice (`from=user`); the
    /// daemon refuses such writes otherwise (the merge law's floor,
    /// enforced at the store so no frontend can silently rewrite the
    /// user's voice). Derived-row replaces never need it.
    #[serde(default)]
    pub user_directed: bool,
}

/// Bulk insert. Each element is a plain [`AddRequest`]; the whole batch is
/// embedded in one serialized forward-pass sequence and written with a
/// single LanceDB commit per table. This is the `ling-mem add --stdin`
/// path: one HTTP call for the entire import instead of one POST (and one
/// commit, one version) per row — which is what made large imports degrade
/// super-linearly and eventually trip the client timeout.
///
/// Bulk semantics match the direct-store `cmd_add` stdin path: always plain
/// insert (no dedup), `replace_ids` is not honored here (it's a single-row
/// conflict-resolution helper).
#[derive(Debug, Deserialize)]
pub struct AddBatchRequest {
    pub facts: Vec<AddRequest>,
}

#[derive(Debug, Deserialize)]
pub struct GetRequest {
    pub id: String,
    /// `Some(true)` = episodic only, `Some(false)` = semantic only,
    /// **`None` (default) = span both tables**. Returns the first match.
    /// Callers used to need a 404-then-retry dance; the daemon now does
    /// that internally.
    #[serde(default)]
    pub episodic: Option<bool>,
}

/// Filter block shared by `search`, `list`, and `forget`. All fields
/// optional; an empty block matches every row. Every enum-typed field
/// accepts the lowercase variant name (`"fact"`, `"positive"`, …).
#[derive(Debug, Default, Deserialize)]
pub struct FilterDTO {
    #[serde(default)]
    pub contexts: Vec<String>,
    /// Narrow to one `MemoryType`. Linggen's tool schema is singular;
    /// internally we convert to `Filters.types: Vec<MemoryType>`.
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub r#type: Option<MemoryType>,
    /// Narrow to one tier (`core` or `semantic`). Within the semantic
    /// table both tiers coexist; this filter lets callers ask for "just
    /// the always-on identity set" (`tier=core`) or "everything else"
    /// (`tier=semantic`). Ignored for episodic queries — episodic rows
    /// don't carry a meaningful tier.
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub tier: Option<Tier>,
    #[serde(
        default,
        alias = "origin",
        deserialize_with = "deserialize_optional_lenient"
    )]
    pub from: Option<Origin>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub outcome: Option<Outcome>,
    #[serde(default, deserialize_with = "deserialize_optional_datetime")]
    pub since: Option<DateTime<Utc>>,
    /// Upper bound on `COALESCE(occurred_at, created_at)`. `older_than`
    /// is accepted as an alias (legacy shape from the v0.1 translate_args
    /// table in Linggen core).
    #[serde(
        default,
        alias = "older_than",
        deserialize_with = "deserialize_optional_datetime"
    )]
    pub until: Option<DateTime<Utc>>,
    /// Narrow to one `source_session` id. Powers dashboard deep-links
    /// like `?session=<sid>` so the user can drill into rows the agent
    /// wrote during one engine session.
    #[serde(default)]
    pub source_session: Option<String>,
    /// Scope recall to the work being done at this path — rows written under
    /// it, plus every row that belongs to no project (see `Filters::cwd_scope`).
    ///
    /// Deliberately NOT accepted by `forget` as a standalone filter: it matches
    /// every unscoped row by design, so a delete carrying only this would take
    /// most of the store. The empty-filter guard below does not list it.
    #[serde(default)]
    pub cwd_scope: Option<String>,
    /// `true` = apply the daemon's configured `episodic_ttl_days` as an
    /// upper bound on `occurred_at` (i.e. "rows that are past their
    /// TTL"). Resolved at handler entry and folded into `until`. The
    /// dream consolidator sets this so the mission body never has to
    /// know the live TTL value. Opt-in: default `false` keeps every
    /// existing caller (dashboard, CLI list, etc.) unchanged.
    #[serde(default)]
    pub past_ttl: bool,
    /// One **local calendar day**, `YYYY-MM-DD` — sugar over
    /// `since`/`until` covering exactly that day. The dream pipeline's
    /// remember stage lists one day's worklist with this. Explicit
    /// `since`/`until` win; `day` only fills what's unset.
    #[serde(default)]
    pub day: Option<String>,
    /// Include archived rows (`expired_at IS NOT NULL`) — losers a
    /// `replace_ids` merge or digest expired out of live memory. Default
    /// `false`: the archive serves provenance and unpack, never recall.
    #[serde(default)]
    pub include_expired: bool,
    /// Unpack query: only the archived rows this survivor id replaced.
    /// Implies `include_expired`.
    #[serde(default)]
    pub superseded_by: Option<String>,
}

impl FilterDTO {
    /// Fold the `day` sugar into `since`/`until`. Called by every
    /// handler before `into_filters` — a bad day string is a 400, not a
    /// silently-ignored filter.
    fn resolve_day(&mut self) -> Result<(), ApiError> {
        let Some(day) = self.day.take() else {
            return Ok(());
        };
        let date = chrono::NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d").map_err(|_| {
            ApiError::bad_request(format!("invalid day {day:?}: expected YYYY-MM-DD"))
        })?;
        let (start, end) = super::days::local_day_bounds(date);
        if self.since.is_none() {
            self.since = Some(start);
        }
        if self.until.is_none() {
            self.until = Some(end);
        }
        Ok(())
    }

    fn into_filters(mut self) -> Result<Filters, ApiError> {
        self.resolve_day()?;
        Ok(Filters {
            contexts: self.contexts,
            types: self.r#type.into_iter().collect(),
            origin: self.from,
            outcome: self.outcome,
            since: self.since,
            until: self.until,
            tier: self.tier,
            source_session: self.source_session,
            cwd_scope: self.cwd_scope,
            include_expired: self.include_expired,
            superseded_by: self.superseded_by,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(flatten)]
    pub filters: FilterDTO,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    /// Drop rows whose cosine similarity to the query falls below this
    /// threshold. Range `[-1.0, 1.0]`; in practice Qwen3-Embedding-0.6B
    /// outputs land in `[0.0, 1.0]`. Omit to disable filtering.
    #[serde(default)]
    pub min_score: Option<f32>,
    /// Which table(s) to query. Default is `both` (cross-table recall —
    /// the consolidator subagent relies on this). `semantic` / `episodic`
    /// constrain to a single table so a tab-scoped UI does not double-
    /// fetch and merge the same rows twice. Note: this selects the
    /// **table**, not the `tier` field; `tier=core` filtering inside the
    /// semantic table is a separate parameter.
    #[serde(default)]
    pub table: SearchTable,
    /// Table-scope alias matching every other CRUD DTO: `true` = episodic
    /// only, `false` = semantic only. Overrides `table` when set. This is
    /// the field the MCP dispatch shim produces from `tier=episodic` —
    /// before it existed, an episodic-scoped MCP search silently searched
    /// BOTH tables (the flag landed as an ignored unknown field).
    #[serde(default)]
    pub episodic: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchTable {
    #[default]
    Both,
    Semantic,
    Episodic,
}

fn default_search_limit() -> usize {
    10
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDTO {
    #[default]
    Newest,
    Oldest,
}

impl From<SortDTO> for SortOrder {
    fn from(v: SortDTO) -> Self {
        match v {
            SortDTO::Newest => SortOrder::Newest,
            SortDTO::Oldest => SortOrder::Oldest,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListRequest {
    #[serde(flatten)]
    pub filters: FilterDTO,
    #[serde(default)]
    pub sort: SortDTO,
    #[serde(default = "default_list_limit")]
    pub limit: usize,
    /// Number of rows to skip in the sorted result. `0` = first page.
    #[serde(default)]
    pub offset: usize,
    /// `Some(true)` = episodic only, `Some(false)` = semantic only,
    /// **`None` (default) = span both tables**. Merged + sorted +
    /// limited as one result set.
    #[serde(default)]
    pub episodic: Option<bool>,
}

fn default_list_limit() -> usize {
    50
}

/// Lightweight summary endpoint used by the memory dashboard. Returns
/// the row count + the most recent row's `created_at` per filter set,
/// so the on-open UI can render tier cards (Core / Semantic / Episodic)
/// without paging through actual rows. `count` is metadata-only at the
/// LanceDB layer; `latest_created_at` costs one row fetch.
///
/// Same scope rules as `list`: `episodic` `None` spans both stores,
/// `Some(true)` episodic only, `Some(false)` semantic only.
#[derive(Debug, Deserialize)]
pub struct CountRequest {
    #[serde(flatten)]
    pub filters: FilterDTO,
    #[serde(default)]
    pub episodic: Option<bool>,
}

/// Update semantics mirror the CLI: explicit set-vs-clear via twin
/// fields (`outcome` / `clear_outcome`, `cwd` / `clear_cwd`). Absent
/// fields mean "leave unchanged." Set wins over clear if both are given.
#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    pub id: String,
    pub content: Option<String>,
    pub contexts: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub r#type: Option<MemoryType>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub tier: Option<Tier>,
    #[serde(
        default,
        alias = "origin",
        deserialize_with = "deserialize_optional_lenient"
    )]
    pub from: Option<Origin>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub outcome: Option<Outcome>,
    #[serde(default)]
    pub clear_outcome: bool,
    pub cwd: Option<String>,
    #[serde(default)]
    pub clear_cwd: bool,
    pub host: Option<String>,
    #[serde(default)]
    pub clear_host: bool,
    /// When the remembered thing happened (recall sorts by it). Was silently
    /// dropped for the endpoint's whole life — the store's patch always
    /// carried it, only this DTO never asked — which is how a backdate
    /// "succeeded" while changing nothing.
    #[serde(default, deserialize_with = "deserialize_optional_datetime")]
    pub occurred_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub clear_occurred_at: bool,
    /// `Some(true)` = episodic only, `Some(false)` = semantic only,
    /// **`None` (default) = locate the id in whichever table holds it**
    /// before applying the patch.
    #[serde(default)]
    pub episodic: Option<bool>,
    /// See [`AddRequest::user_directed`]. Required when this update
    /// rewrites `content` on a `from=user` row; metadata-only patches
    /// (tier, contexts, tags) stay unguarded.
    #[serde(default)]
    pub user_directed: bool,
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    pub id: String,
    /// `Some(true)` = episodic only, `Some(false)` = semantic only,
    /// **`None` (default) = locate the id in whichever table holds it**
    /// before deleting.
    #[serde(default)]
    pub episodic: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ForgetRequest {
    #[serde(flatten)]
    pub filters: FilterDTO,
    /// Target the episodic table instead of the default semantic table.
    #[serde(default)]
    pub episodic: bool,
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// Pick the semantic or episodic store based on the request's `episodic`
/// flag. Centralized so every CRUD endpoint routes consistently.
fn pick_store(state: &SharedState, episodic: bool) -> Arc<MemoryStore> {
    if episodic {
        Arc::clone(&state.episodic)
    } else {
        Arc::clone(&state.store)
    }
}

/// Resolve which store(s) a read/edit/delete should hit.
///   - `Some(true)`  → episodic only
///   - `Some(false)` → semantic only
///   - `None`        → both (caller decides how to merge)
///
/// Returned in the order semantic-first so locate-first-hit semantics
/// preserve the higher-tier preference when an id collision exists
/// across tables.
fn stores_for_read(state: &SharedState, episodic: Option<bool>) -> Vec<Arc<MemoryStore>> {
    match episodic {
        Some(true) => vec![Arc::clone(&state.episodic)],
        Some(false) => vec![Arc::clone(&state.store)],
        None => vec![Arc::clone(&state.store), Arc::clone(&state.episodic)],
    }
}

async fn add(
    State(state): State<SharedState>,
    Json(req): Json<AddRequest>,
) -> Result<Response, ApiError> {
    if req.content.trim().is_empty() {
        return Err(ApiError::bad_request("content must not be empty"));
    }
    guard_user_voice(&state, &req.replace_ids, req.user_directed).await?;

    let skip_dedup = req.skip_dedup;
    // `tier=episodic` in the body is equivalent to `episodic: true` —
    // both route to the staging table.
    let episodic = req.episodic || req.tier == Some(crate::memory::Tier::Episodic);
    let replace_ids = req.replace_ids.clone();
    let mut fact = Memory::new(
        req.content,
        req.r#type.unwrap_or(MemoryType::Fact),
        req.from.unwrap_or_default(),
    );
    fact.contexts = req.contexts;
    fact.tags = req.tags;
    fact.outcome = req.outcome;
    fact.cwd = req.cwd;
    fact.occurred_at = req.occurred_at;
    fact.source_session = req.source_session;
    fact.host = req.host;
    if episodic {
        // The row's table marks it as episodic; keep `tier` in sync so
        // callers filtering by tier alone don't see lies. Overrides any
        // stale Core / Semantic carried in from a generic Memory::new().
        fact.tier = crate::memory::Tier::Episodic;
    } else if let Some(tier) = req.tier {
        // Core vs semantic within the semantic table.
        fact.tier = tier;
    }

    // Embed the content so the row is immediately searchable. Serialized +
    // off the async workers so concurrent adds can't stack forward passes.
    let vector = state
        .embedder
        .clone()
        .embed_passage(fact.content.clone())
        .await
        .map_err(ApiError::internal)?;
    fact.vector = Some(vector);

    let store = pick_store(&state, episodic);
    if skip_dedup {
        store.insert(std::slice::from_ref(&fact)).await?;
        let body = json!({
            "action": "added",
            "fact": fact_public(&fact),
        });
        return Ok(ok(apply_replace_ids(&state, &replace_ids, &fact.id, body).await));
    }

    // Cross-tier dedup. The single-table `insert_with_dedup` below only
    // catches duplicates inside the chosen store; without this step, a
    // semantic copy and an episodic copy of byte-identical (content, type)
    // would coexist. Tier rank: Core > Semantic > Episodic. The higher-
    // tier row wins; on equal rank (e.g. both semantic) the in-table
    // dedup handles it. See linggen `agents/ling-mem.md` tier-discipline.
    let other = if episodic {
        Arc::clone(&state.store)
    } else {
        Arc::clone(&state.episodic)
    };
    if let Some(existing) = other
        .find_exact_content_public(&fact.content, fact.r#type)
        .await?
    {
        let new_rank = tier_rank(fact.tier);
        let existing_rank = tier_rank(existing.tier);
        if existing_rank >= new_rank {
            // Existing row is at the same or higher tier — keep it.
            // Merge contexts/tags so the new write's metadata isn't lost,
            // then return as if dedup'd.
            let merged = merge_with_existing(&existing, &fact);
            other.update_full_public(&existing.id, &merged).await?;
            let body = json!({
                "action": "merged",
                "similarity": 1.0,
                "previous_id": existing.id,
                "fact": fact_public(&merged),
            });
            return Ok(ok(apply_replace_ids(&state, &replace_ids, &existing.id, body).await));
        }
        // New row is at a higher tier — promote: delete the lower-tier
        // copy from the other table, then proceed with single-table insert.
        let _ = other.delete(&existing.id).await?;
    }

    let outcome = store.insert_with_dedup(fact).await?;
    let survivor = match &outcome {
        crate::memory::InsertOutcome::Added(f) => f.id.clone(),
        crate::memory::InsertOutcome::Merged { fact, .. } => fact.id.clone(),
    };
    let body = apply_replace_ids(&state, &replace_ids, &survivor, outcome_public(&outcome)).await;
    Ok(ok(body))
}

/// Bulk insert N rows in one call: build every row, batch-embed all
/// contents (one gate acquisition), then commit each table's rows with a
/// single `MemoryStore::insert` (one LanceDB version per table, not per
/// row). Empty-content rows are skipped rather than failing the batch.
async fn add_batch(
    State(state): State<SharedState>,
    Json(req): Json<AddBatchRequest>,
) -> Result<Response, ApiError> {
    // Partition by target table up front so each table commits exactly once.
    let mut semantic: Vec<Memory> = Vec::new();
    let mut episodic: Vec<Memory> = Vec::new();
    for r in req.facts {
        if r.content.trim().is_empty() {
            continue; // skip blanks; don't abort the whole import for one
        }
        let mut fact = Memory::new(
            r.content,
            r.r#type.unwrap_or(MemoryType::Fact),
            r.from.unwrap_or_default(),
        );
        fact.contexts = r.contexts;
        fact.tags = r.tags;
        fact.outcome = r.outcome;
        fact.cwd = r.cwd;
        fact.occurred_at = r.occurred_at;
        fact.source_session = r.source_session;
        fact.host = r.host;
        if r.episodic {
            fact.tier = Tier::Episodic;
            episodic.push(fact);
        } else {
            semantic.push(fact);
        }
    }

    let mut added = Vec::new();
    for (mut rows, is_episodic) in [(semantic, false), (episodic, true)] {
        if rows.is_empty() {
            continue;
        }
        // One serialized batch embed for the whole group (chunked to the
        // embedder's MAX_EMBED_BATCH internally), then one commit.
        let texts: Vec<String> = rows.iter().map(|f| f.content.clone()).collect();
        let vectors = state
            .embedder
            .clone()
            .embed_passages(texts)
            .await
            .map_err(ApiError::internal)?;
        for (f, v) in rows.iter_mut().zip(vectors) {
            f.vector = Some(v);
        }
        pick_store(&state, is_episodic).insert(&rows).await?;
        added.extend(rows.iter().map(fact_public));
    }

    Ok(ok(json!({
        "action": "added",
        "count": added.len(),
        "facts": added,
    })))
}

/// The merge law's floor, enforced at the store: a write that replaces
/// (`add` + `replace_ids`) or rewrites (`update` + `content`) rows in the
/// USER'S VOICE (`from=user`) is refused unless the caller asserts
/// `user_directed: true` — the user's current message states the change
/// as settled, or the host resolved the conflict with the user via an
/// ask. Hosts justify the flag; the daemon guarantees no silent
/// user-voice rewrite reaches the store regardless of which frontend
/// wrote it (the Linggen engine has its own pre-flight guard and
/// forwards the flag; CLI/MCP hosts pass it directly). Derived rows
/// pass free. A missing id is not the guard's problem — the real call
/// reports it.
async fn guard_user_voice(
    state: &SharedState,
    target_ids: &[String],
    user_directed: bool,
) -> Result<(), ApiError> {
    if user_directed || target_ids.is_empty() {
        return Ok(());
    }
    let mut offending = Vec::new();
    for id in target_ids {
        for store in [Arc::clone(&state.store), Arc::clone(&state.episodic)] {
            if let Some(fact) = store.get(id).await? {
                if fact.origin == crate::memory::Origin::User {
                    let gist: String = fact.content.chars().take(60).collect();
                    offending.push(format!("{id} \"{gist}\""));
                }
                break;
            }
        }
    }
    if offending.is_empty() {
        return Ok(());
    }
    Err(ApiError::bad_request(format!(
        "BLOCKED by the user-voice merge guard: this write replaces/rewrites rows in the USER'S VOICE (from=user): {}. The user's voice changes only with the user (merge law). Recovery — pick ONE: (1) if the user's CURRENT message states this change as SETTLED — a command (\"update X to Y\", \"forget X\"), a declaration (\"my X is now Y\"), or a commitment (\"from now on, X\") — retry the exact same call with \"user_directed\": true; a hedged reflection (\"X feels about right to me\") does NOT qualify; (2) otherwise ask the user to choose (the host's ask-user primitive, or a plain question in chat), then retry with \"user_directed\": true after their answer. Do NOT work around it by writing a new row without replace_ids — that creates the drift this guard exists to prevent.",
        offending.join(", ")
    )))
}

/// Apply the caller's `replace_ids` deletion sweep across both tables and
/// fold the outcome into `body`.
///
/// `replace_ids` are the losers the user picked against via AskUser; they
/// are deleted regardless of which tier they live in (the caller doesn't
/// track tier). Failures are surfaced (`replaced_failed`) but never abort
/// the add — the insert is the load-bearing operation.
///
/// This runs on **every** add return path (added / merged / promoted). It
/// used to be inline before only the final return, so an AskUser-resolved
/// conflict whose winner happened to dedup-merge into an existing row would
/// silently keep all the losers. Hoisting it into a shared helper closes
/// that gap.
/// Retire the losers of a merge. Semantic-table losers are EXPIRED, not
/// deleted (2026-08-17, Liang's call): stamped `expired_at` +
/// `superseded_by = successor`, they leave every default read but stay on
/// disk — a merge or digest is reversible via the unpack query
/// (`superseded_by` filter). Episodic losers stay hard-deleted: staging
/// is disposable by design, and an immortal staged row would outlive the
/// sweep that exists to kill it.
async fn apply_replace_ids(
    state: &SharedState,
    replace_ids: &[String],
    successor: &str,
    mut body: serde_json::Value,
) -> serde_json::Value {
    if replace_ids.is_empty() {
        return body;
    }
    let mut replaced = Vec::new();
    let mut replaced_failed = Vec::new();
    for id in replace_ids {
        let retired = state.store.expire(id, successor).await.unwrap_or(false)
            || state.episodic.delete(id).await.unwrap_or(false);
        if retired {
            replaced.push(id.clone());
        } else {
            replaced_failed.push(id.clone());
        }
    }
    if let Some(obj) = body.as_object_mut() {
        obj.insert("replaced".to_string(), json!(replaced));
        if !replaced_failed.is_empty() {
            obj.insert("replaced_failed".to_string(), json!(replaced_failed));
        }
    }
    body
}

/// Tier rank for the "higher wins on cross-tier dedup" rule.
/// Higher number = higher tier. Core (2) > Semantic (1) > Episodic (0).
fn tier_rank(tier: crate::memory::Tier) -> u8 {
    use crate::memory::Tier::*;
    match tier {
        Core => 2,
        Semantic => 1,
        Episodic => 0,
    }
}

/// Same merge logic as `store::merge_fact` but at the HTTP layer so we
/// can compose it with a cross-store update. Unions contexts + tags into
/// the existing row, takes the longer content, fills missing optional
/// fields from the candidate.
fn merge_with_existing(
    existing: &crate::memory::Memory,
    candidate: &crate::memory::Memory,
) -> crate::memory::Memory {
    let mut merged = existing.clone();
    if candidate.content.len() > existing.content.len() {
        merged.content = candidate.content.clone();
        merged.vector = candidate.vector.clone();
    }
    for c in &candidate.contexts {
        if !merged.contexts.contains(c) {
            merged.contexts.push(c.clone());
        }
    }
    for t in &candidate.tags {
        if !merged.tags.contains(t) {
            merged.tags.push(t.clone());
        }
    }
    if candidate.outcome.is_some() {
        merged.outcome = candidate.outcome;
    }
    if candidate.cwd.is_some() {
        merged.cwd = candidate.cwd.clone();
    }
    if candidate.occurred_at.is_some() {
        merged.occurred_at = candidate.occurred_at;
    }
    if candidate.source_session.is_some() {
        merged.source_session = candidate.source_session.clone();
    }
    if candidate.host.is_some() {
        merged.host = candidate.host.clone();
    }
    merged
}

/// Wrap an [`InsertOutcome`] as the JSON payload returned by the add
/// endpoint. Always includes `action` and `fact`; on a merge also
/// includes `similarity` and `previous_id`.
fn outcome_public(outcome: &InsertOutcome) -> Value {
    match outcome {
        InsertOutcome::Added(f) => json!({
            "action": "added",
            "fact": fact_public(f),
        }),
        InsertOutcome::Merged {
            fact,
            similarity,
            previous_id,
        } => json!({
            "action": "merged",
            "similarity": similarity,
            "previous_id": previous_id,
            "fact": fact_public(fact),
        }),
    }
}

async fn get(
    State(state): State<SharedState>,
    Json(req): Json<GetRequest>,
) -> Result<Response, ApiError> {
    for store in stores_for_read(&state, req.episodic) {
        if let Some(fact) = store.get(&req.id).await? {
            return Ok(ok(fact_public(&fact)));
        }
    }
    Err(ApiError::not_found(format!("no fact with id {}", req.id)))
}

async fn search(
    State(state): State<SharedState>,
    Json(req): Json<SearchRequest>,
) -> Result<Response, ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError::bad_request("query must not be empty"));
    }

    let vector = state
        .embedder
        .clone()
        .embed_query_serialized(req.query.clone())
        .await
        .map_err(ApiError::internal)?;
    let filters = req.filters.into_filters()?;

    // Store-wide recall floor: when the client omits `min_score`, apply the
    // daemon's configured `recall_min_score` so every host shares one recall
    // selectivity. An explicit per-call `min_score` overrides. (Same
    // server-owns-it pattern as `episodic_ttl_days`.)
    let min_score = match req.min_score {
        Some(s) => Some(s),
        None => Some(
            crate::http::config::load(&state.data_dir)
                .await
                .recall_min_score,
        ),
    };

    // Hybrid retrieval: each row scored as cosine + an IDF-weighted keyword
    // boost, so exact-keyword queries rank correctly without inventing
    // relevance for unrelated rows. `min_score` gates the hybrid score,
    // applied inside the fuse step (see crate::memory::hybrid).
    let table = match req.episodic {
        Some(true) => SearchTable::Episodic,
        Some(false) => SearchTable::Semantic,
        None => req.table,
    };
    let results = match table {
        SearchTable::Both => {
            state
                .recall
                .query(&vector, &req.query, &filters, req.limit, min_score)
                .await?
        }
        SearchTable::Semantic => {
            state
                .store
                .hybrid_scored(&vector, &req.query, &filters, req.limit, min_score)
                .await?
        }
        SearchTable::Episodic => {
            state
                .episodic
                .hybrid_scored(&vector, &req.query, &filters, req.limit, min_score)
                .await?
        }
    };
    Ok(ok(scored_facts_public(&results)))
}

async fn list(
    State(state): State<SharedState>,
    Json(mut req): Json<ListRequest>,
) -> Result<Response, ApiError> {
    // Resolve `past_ttl: true` into a concrete `until` cutoff. The mission
    // body sends `past_ttl: true` so it doesn't have to hardcode the TTL
    // number; the daemon owns the live value and applies it here.
    // An explicit `until`/`older_than` from the caller wins (caller knows
    // best); we only fill in when nothing is set.
    if req.filters.past_ttl && req.filters.until.is_none() {
        let cfg = crate::http::config::load(&state.data_dir).await;
        let cutoff = chrono::Utc::now() - chrono::Duration::days(cfg.episodic_ttl_days as i64);
        req.filters.until = Some(cutoff);
    }
    // `past_ttl` is an episodic-TTL concept and the tool schema promises
    // "implies tier=episodic" — scope the read accordingly unless the
    // caller explicitly asked for a table, so old semantic rows don't
    // masquerade as evictable staging.
    if req.filters.past_ttl && req.episodic.is_none() {
        req.episodic = Some(true);
    }
    let filters = req.filters.into_filters()?;
    let sort = req.sort.into();
    let mut combined = Vec::new();
    // For each in-scope store, pull `limit + offset` rows so the post-
    // merge sort+page still has enough material; bounded by the per-
    // store list cap.
    let take = req.limit.saturating_add(req.offset);
    for store in stores_for_read(&state, req.episodic) {
        let rows = store.list(&filters, sort, take, 0).await?;
        combined.extend(rows);
    }
    sort_combined(&mut combined, sort);
    let paged: Vec<_> = combined
        .into_iter()
        .skip(req.offset)
        .take(req.limit)
        .collect();
    Ok(ok(facts_public(&paged)))
}

fn sort_combined(rows: &mut [crate::memory::Memory], sort: crate::memory::SortOrder) {
    use crate::memory::SortOrder::*;
    // Match the store-layer order (and the UI's age badge): sort by
    // activity_timestamp (updated_at ?? created_at), NOT effective_timestamp.
    // effective_timestamp prefers the consolidator's back-dated occurred_at,
    // which buries freshly-written rows below their save date.
    rows.sort_by(|a, b| match sort {
        Newest => b.activity_timestamp().cmp(&a.activity_timestamp()),
        Oldest => a.activity_timestamp().cmp(&b.activity_timestamp()),
    });
}

async fn count(
    State(state): State<SharedState>,
    Json(req): Json<CountRequest>,
) -> Result<Response, ApiError> {
    let filters = req.filters.into_filters()?;
    let stores = stores_for_read(&state, req.episodic);

    // Sum row counts across in-scope stores (metadata-only LanceDB call).
    let mut total: usize = 0;
    for store in &stores {
        total = total.saturating_add(store.count_filtered(&filters).await?);
    }

    // "Latest" = max activity_timestamp (updated_at ?? created_at) across
    // in-scope stores — the same key the list order and UI badge use.
    // `list(Newest, 1)` returns each store's true newest row (list sorts the
    // full filtered set before slicing), so the top row's activity timestamp
    // is the answer. Skipped when count is 0 to avoid an empty fetch.
    let latest = if total == 0 {
        None
    } else {
        let mut newest: Option<chrono::DateTime<chrono::Utc>> = None;
        for store in &stores {
            let rows = store
                .list(&filters, crate::memory::SortOrder::Newest, 1, 0)
                .await?;
            if let Some(ts) = rows.first().map(|r| r.activity_timestamp()) {
                newest = Some(newest.map_or(ts, |cur| cur.max(ts)));
            }
        }
        newest
    };

    Ok(ok(json!({
        // Key kept for the dashboard's existing contract; value is now the
        // activity timestamp (updated_at ?? created_at), so an edited row
        // counts as "latest".
        "count": total,
        "latest_created_at": latest,
    })))
}

async fn update(
    State(state): State<SharedState>,
    Json(req): Json<UpdateRequest>,
) -> Result<Response, ApiError> {
    // A content rewrite of an existing row is the same violation class
    // as an add-with-replace_ids; metadata-only patches stay unguarded.
    if req.content.is_some() {
        guard_user_voice(&state, std::slice::from_ref(&req.id), req.user_directed).await?;
    }
    let outcome_patch = match (req.outcome, req.clear_outcome) {
        (Some(o), _) => Some(Some(o)),
        (None, true) => Some(None),
        (None, false) => None,
    };
    let cwd_patch = match (req.cwd, req.clear_cwd) {
        (Some(v), _) => Some(Some(v)),
        (None, true) => Some(None),
        (None, false) => None,
    };
    let host_patch = match (req.host, req.clear_host) {
        (Some(v), _) => Some(Some(v)),
        (None, true) => Some(None),
        (None, false) => None,
    };
    let occurred_patch = match (req.occurred_at, req.clear_occurred_at) {
        (Some(v), _) => Some(Some(v)),
        (None, true) => Some(None),
        (None, false) => None,
    };

    let patch = MemoryPatch {
        content: req.content,
        contexts: req.contexts,
        tags: req.tags,
        r#type: req.r#type,
        tier: req.tier,
        origin: req.from,
        outcome: outcome_patch,
        cwd: cwd_patch,
        host: host_patch,
        occurred_at: occurred_patch,
        ..Default::default()
    };

    // Locate which table currently holds the row, so a tier patch that
    // crosses the semantic/episodic boundary can be honored by moving the
    // row across tables instead of leaving a tier=episodic row stranded
    // in the semantic table (or vice-versa).
    let mut located: Option<(Arc<MemoryStore>, Memory)> = None;
    for store in stores_for_read(&state, req.episodic) {
        if let Some(fact) = store.get(&req.id).await? {
            located = Some((store, fact));
            break;
        }
    }
    let Some((current_store, existing)) = located else {
        return Err(ApiError::not_found(format!("no fact with id {}", req.id)));
    };

    let target_tier = patch.tier.unwrap_or(existing.tier);
    let target_is_episodic = matches!(target_tier, Tier::Episodic);
    let current_is_episodic = Arc::ptr_eq(&current_store, &state.episodic);

    if current_is_episodic == target_is_episodic {
        // Same-table update — delete+reinsert inside one store.
        let updated = current_store
            .update(&req.id, &patch)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("no fact with id {}", req.id)))?;
        return Ok(ok(fact_public(&updated)));
    }

    // Cross-table move: apply patch in memory, delete from source, insert
    // into target. Preserves the id and the existing embedding vector.
    let target_store = if target_is_episodic {
        Arc::clone(&state.episodic)
    } else {
        Arc::clone(&state.store)
    };
    let mut moved = existing;
    patch.apply(&mut moved);
    moved.tier = target_tier;
    moved.updated_at = Some(Utc::now().trunc_subsecs(6));
    current_store.delete(&req.id).await?;
    target_store.insert(std::slice::from_ref(&moved)).await?;
    Ok(ok(fact_public(&moved)))
}

async fn delete(
    State(state): State<SharedState>,
    Json(req): Json<DeleteRequest>,
) -> Result<Response, ApiError> {
    for store in stores_for_read(&state, req.episodic) {
        if store.delete(&req.id).await? {
            return Ok(ok(json!({"id": req.id, "removed": true})));
        }
    }
    Ok(ok(json!({"id": req.id, "removed": false})))
}

async fn forget(
    State(state): State<SharedState>,
    Json(req): Json<ForgetRequest>,
) -> Result<Response, ApiError> {
    let filters = req.filters.into_filters()?;
    // Refuse empty filters — bulk delete must be intentional. Matches the
    // CLI's refusal when no filter flags are passed.
    if filters.contexts.is_empty()
        && filters.types.is_empty()
        && filters.origin.is_none()
        && filters.outcome.is_none()
        && filters.tier.is_none()
        && filters.since.is_none()
        && filters.until.is_none()
        && filters.source_session.is_none()
    {
        return Err(ApiError::bad_request(
            "forget refuses an empty filter — supply at least one of \
             contexts, type, tier, from, outcome, since, until, source_session",
        ));
    }
    let store = pick_store(&state, req.episodic);
    let removed = store.forget(&filters).await?;
    Ok(ok(json!({"removed": removed})))
}
