//! `/api/memory/<method>` — RPC-style endpoints, 1:1 with the `Memory.*`
//! tools in Linggen. Each endpoint POSTs JSON args and returns
//! `{ok, data}` or `{ok:false, error, code}` via `envelope::ApiError`.
//!
//! Semantics mirror the CLI handlers in `crate::cli` — this module is
//! the network-facing path to the same `FactsStore` operations. Once
//! Phase 4 lands in Linggen, the CLI data-ops wrappers are removed and
//! HTTP becomes the only dispatch path.

use super::envelope::{ok, ApiError};
use super::state::SharedState;
use crate::facts::{
    Fact, FactPatch, FactType, Filters, InsertOutcome, Origin, Outcome, SortOrder,
};
use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use uuid::Uuid;

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

/// Serialize a fact for HTTP response, stripping the 384-dim embedding
/// vector. Callers never need the raw vector over the wire, and including
/// it bloats every response by ~5 KB / row (noisy for the model, for logs,
/// and for the data UI). The CLI's NDJSON output keeps vectors — they
/// matter for `add --stdin` round-trips.
fn fact_public(fact: &Fact) -> Value {
    let mut v = serde_json::to_value(fact).unwrap_or_else(|_| Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.remove("vector");
    }
    v
}

fn facts_public(facts: &[Fact]) -> Vec<Value> {
    facts.iter().map(fact_public).collect()
}

/// Memory subrouter. Mounted at `/api/memory/` by the parent router.
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/memory/add", post(add))
        .route("/api/memory/get", post(get))
        .route("/api/memory/search", post(search))
        .route("/api/memory/list", post(list))
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
    pub r#type: Option<FactType>,
    /// Origin. Canonical name is `from` (matches the `Fact` field);
    /// accept `origin` as an alias for callers that avoid reserved words.
    #[serde(default, alias = "origin", deserialize_with = "deserialize_optional_lenient")]
    pub from: Option<Origin>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub outcome: Option<Outcome>,
    pub cwd: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_datetime")]
    pub occurred_at: Option<DateTime<Utc>>,
    pub source_session: Option<String>,
    /// Bypass dedup: insert as a new row even if a near-duplicate exists.
    /// Accepts `skip_dedup` (canonical) or `force` (alias).
    #[serde(default, alias = "force")]
    pub skip_dedup: bool,
}

#[derive(Debug, Deserialize)]
pub struct GetRequest {
    pub id: Uuid,
}

/// Filter block shared by `search`, `list`, and `forget`. All fields
/// optional; an empty block matches every row. Every enum-typed field
/// accepts the lowercase variant name (`"fact"`, `"positive"`, …).
#[derive(Debug, Default, Deserialize)]
pub struct FilterDTO {
    #[serde(default)]
    pub contexts: Vec<String>,
    /// Narrow to one `FactType`. Linggen's tool schema is singular;
    /// internally we convert to `Filters.types: Vec<FactType>`.
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub r#type: Option<FactType>,
    #[serde(default, alias = "origin", deserialize_with = "deserialize_optional_lenient")]
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
}

impl FilterDTO {
    fn into_filters(self) -> Filters {
        Filters {
            contexts: self.contexts,
            types: self.r#type.into_iter().collect(),
            origin: self.from,
            outcome: self.outcome,
            since: self.since,
            until: self.until,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(flatten)]
    pub filters: FilterDTO,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
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
}

fn default_list_limit() -> usize {
    50
}

/// Update semantics mirror the CLI: explicit set-vs-clear via twin
/// fields (`outcome` / `clear_outcome`, `cwd` / `clear_cwd`). Absent
/// fields mean "leave unchanged." Set wins over clear if both are given.
#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    pub id: Uuid,
    pub content: Option<String>,
    pub contexts: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub r#type: Option<FactType>,
    #[serde(default, alias = "origin", deserialize_with = "deserialize_optional_lenient")]
    pub from: Option<Origin>,
    #[serde(default, deserialize_with = "deserialize_optional_lenient")]
    pub outcome: Option<Outcome>,
    #[serde(default)]
    pub clear_outcome: bool,
    pub cwd: Option<String>,
    #[serde(default)]
    pub clear_cwd: bool,
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    pub id: Uuid,
}

#[derive(Debug, Default, Deserialize)]
pub struct ForgetRequest {
    #[serde(flatten)]
    pub filters: FilterDTO,
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn add(
    State(state): State<SharedState>,
    Json(req): Json<AddRequest>,
) -> Result<Response, ApiError> {
    if req.content.trim().is_empty() {
        return Err(ApiError::bad_request("content must not be empty"));
    }

    let skip_dedup = req.skip_dedup;
    let mut fact = Fact::new(
        req.content,
        req.r#type.unwrap_or(FactType::Fact),
        req.from.unwrap_or_default(),
    );
    fact.contexts = req.contexts;
    fact.tags = req.tags;
    fact.outcome = req.outcome;
    fact.cwd = req.cwd;
    fact.occurred_at = req.occurred_at;
    fact.source_session = req.source_session;

    // Embed the content so the row is immediately searchable.
    let vector = state
        .embedder
        .embed_one(&fact.content)
        .map_err(ApiError::internal)?;
    fact.vector = Some(vector);

    if skip_dedup {
        state.store.insert(std::slice::from_ref(&fact)).await?;
        return Ok(ok(json!({
            "action": "added",
            "fact": fact_public(&fact),
        })));
    }

    let outcome = state.store.insert_with_dedup(fact).await?;
    Ok(ok(outcome_public(&outcome)))
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
    match state.store.get(req.id).await? {
        Some(fact) => Ok(ok(fact_public(&fact))),
        None => Err(ApiError::not_found(format!("no fact with id {}", req.id))),
    }
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
        .embed_one(&req.query)
        .map_err(ApiError::internal)?;
    let results = state
        .store
        .search(&vector, &req.filters.into_filters(), req.limit)
        .await?;
    Ok(ok(facts_public(&results)))
}

async fn list(
    State(state): State<SharedState>,
    Json(req): Json<ListRequest>,
) -> Result<Response, ApiError> {
    let results = state
        .store
        .list(
            &req.filters.into_filters(),
            req.sort.into(),
            req.limit,
            req.offset,
        )
        .await?;
    Ok(ok(facts_public(&results)))
}

async fn update(
    State(state): State<SharedState>,
    Json(req): Json<UpdateRequest>,
) -> Result<Response, ApiError> {
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

    let patch = FactPatch {
        content: req.content,
        contexts: req.contexts,
        tags: req.tags,
        r#type: req.r#type,
        origin: req.from,
        outcome: outcome_patch,
        cwd: cwd_patch,
        ..Default::default()
    };

    match state.store.update(req.id, &patch).await? {
        Some(fact) => Ok(ok(fact_public(&fact))),
        None => Err(ApiError::not_found(format!("no fact with id {}", req.id))),
    }
}

async fn delete(
    State(state): State<SharedState>,
    Json(req): Json<DeleteRequest>,
) -> Result<Response, ApiError> {
    let removed = state.store.delete(req.id).await?;
    Ok(ok(json!({"id": req.id, "removed": removed})))
}

async fn forget(
    State(state): State<SharedState>,
    Json(req): Json<ForgetRequest>,
) -> Result<Response, ApiError> {
    let filters = req.filters.into_filters();
    // Refuse empty filters — bulk delete must be intentional. Matches the
    // CLI's refusal when no filter flags are passed.
    if filters.contexts.is_empty()
        && filters.types.is_empty()
        && filters.origin.is_none()
        && filters.outcome.is_none()
        && filters.since.is_none()
        && filters.until.is_none()
    {
        return Err(ApiError::bad_request(
            "forget refuses an empty filter — supply at least one of \
             contexts, type, from, outcome, since, until",
        ));
    }
    let removed = state.store.forget(&filters).await?;
    Ok(ok(json!({"removed": removed})))
}
