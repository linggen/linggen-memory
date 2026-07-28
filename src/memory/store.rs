//! [`MemoryStore`] — LanceDB wrapper around one memory table.
//!
//! The store is opened once per `ling-mem` invocation (CLI is one-shot in
//! v0.1). It owns a LanceDB [`Connection`] and a [`Table`] handle for one
//! table; the table is auto-created on first open if missing. `open_semantic`
//! binds the `semantic` table (curated long-term memory), `open_episodic`
//! the `episodic` table (staged short-term memory) — same connection, same
//! schema, per-table ANN-index isolation.
//!
//! See `doc/tech-spec.md` for the full CLI contract. This module implements
//! just the storage primitives: `open_semantic`, `insert`, `get`. Search,
//! list, and mutation ops land in subsequent commits.

use super::schema::{
    build_schema, memories_to_record_batch, record_batch_to_memories, EPISODIC_TABLE_NAME,
    SEMANTIC_TABLE_NAME,
};
use super::types::{Memory, MemoryType, Origin, Outcome, Tier};
use anyhow::{anyhow, Context, Result};
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader};
use chrono::{DateTime, SubsecRound, Utc};
use futures::TryStreamExt;
use lancedb::{
    connect,
    query::{ExecutableQuery, QueryBase},
    table::NewColumnTransform,
    Connection, DistanceType, Table,
};
use std::path::Path;
use std::sync::Arc;

/// Filter criteria shared by [`MemoryStore::search`] and [`MemoryStore::list`].
///
/// All filter fields combine with AND. Within `contexts`, every entry must
/// appear in the fact's `contexts` array (AND semantics). Within `types`,
/// any one entry matches (OR). An empty filter matches every row.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub contexts: Vec<String>,
    pub types: Vec<MemoryType>,
    pub origin: Option<Origin>,
    pub outcome: Option<Outcome>,
    pub tier: Option<Tier>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Narrow to a single `source_session` id. Used by dashboard
    /// deep-links (`?session=<sid>`) to show only rows the agent wrote
    /// during one engine session.
    pub source_session: Option<String>,
}

impl Filters {
    /// Render this filter set as a SQL WHERE-clause fragment (without the
    /// `WHERE` keyword itself). Returns `None` when nothing is filtered —
    /// callers skip `only_if` in that case.
    ///
    /// Time fields compare against `COALESCE(occurred_at, created_at)` so
    /// `since` / `until` match whichever timestamp the fact actually carries.
    fn to_sql(&self) -> Option<String> {
        let mut clauses: Vec<String> = Vec::new();

        for ctx in &self.contexts {
            clauses.push(format!("array_has(contexts, '{}')", escape_sql(ctx)));
        }

        if !self.types.is_empty() {
            let or = self
                .types
                .iter()
                .map(|t| format!("type = '{}'", t.as_str()))
                .collect::<Vec<_>>()
                .join(" OR ");
            clauses.push(format!("({or})"));
        }

        // NOTE: `from` is intentionally NOT pushed as a SQL clause here.
        // The column name `from` is a SQL keyword, and LanceDB / DataFusion
        // returns 0 rows for `"from" = 'value'` even though the column does
        // hold values that match — likely a parser bug in the keyword path.
        // Origin filtering happens post-fetch in `apply_origin_filter` below;
        // skipping it in SQL avoids the keyword collision entirely. If the
        // upstream parser is fixed later, restore the SQL clause and drop the
        // post-filter for one less Rust pass over the rows.

        if let Some(o) = self.outcome {
            clauses.push(format!("outcome = '{}'", o.as_str()));
        }

        // Unlike `from`, `tier` is not a SQL keyword, so the plain equality
        // clause is safe to push down — no post-fetch filter needed.
        if let Some(t) = self.tier {
            clauses.push(format!("tier = '{}'", t.as_str()));
        }

        if let Some(since) = self.since {
            clauses.push(format!(
                "COALESCE(occurred_at, created_at) >= TIMESTAMP '{}'",
                since.format("%Y-%m-%d %H:%M:%S%.6f")
            ));
        }

        if let Some(until) = self.until {
            clauses.push(format!(
                "COALESCE(occurred_at, created_at) < TIMESTAMP '{}'",
                until.format("%Y-%m-%d %H:%M:%S%.6f")
            ));
        }

        if let Some(sid) = &self.source_session {
            clauses.push(format!("source_session = '{}'", escape_sql(sid)));
        }

        if clauses.is_empty() {
            None
        } else {
            Some(clauses.join(" AND "))
        }
    }
}

/// Apply the `origin` filter post-fetch. `Filters::to_sql` deliberately
/// skips `from` because LanceDB / DataFusion mis-handles the SQL keyword
/// even when double-quoted (returns 0 rows). Cheap O(N) over the limited
/// row set we already pulled.
fn apply_origin_filter(facts: &mut Vec<Memory>, origin: Option<Origin>) {
    if let Some(o) = origin {
        facts.retain(|f| f.origin == o);
    }
}

/// Sort order for [`MemoryStore::list`]. Search is ordered by similarity and
/// doesn't use this.
#[derive(Debug, Clone, Copy, Default)]
pub enum SortOrder {
    /// Newest first (by `activity_timestamp`, i.e. `updated_at ?? created_at`).
    #[default]
    Newest,
    /// Oldest first.
    Oldest,
}

/// Per-field changes for [`MemoryStore::update`].
///
/// Each field carries **three** states: `None` means "leave unchanged";
/// `Some(value)` applies the value. For nullable schema fields (`outcome`,
/// `cwd`, `occurred_at`, `source_session`, `vector`) the inner value is
/// itself an `Option` — `Some(Some(x))` sets, `Some(None)` clears to null.
#[derive(Debug, Clone, Default)]
pub struct MemoryPatch {
    pub content: Option<String>,
    pub contexts: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub r#type: Option<MemoryType>,
    pub tier: Option<Tier>,
    pub origin: Option<Origin>,
    pub outcome: Option<Option<Outcome>>,
    pub cwd: Option<Option<String>>,
    pub occurred_at: Option<Option<DateTime<Utc>>>,
    pub source_session: Option<Option<String>>,
    pub host: Option<Option<String>>,
    pub vector: Option<Option<Vec<f32>>>,
}

impl MemoryPatch {
    /// Apply this patch in place to a fact. Used internally by
    /// [`MemoryStore::update`]; exposed for tests.
    pub fn apply(&self, f: &mut Memory) {
        if let Some(v) = &self.content {
            f.content = v.clone();
        }
        if let Some(v) = &self.contexts {
            f.contexts = v.clone();
        }
        if let Some(v) = &self.tags {
            f.tags = v.clone();
        }
        if let Some(v) = &self.r#type {
            f.r#type = *v;
        }
        if let Some(v) = &self.tier {
            f.tier = *v;
        }
        if let Some(v) = &self.origin {
            f.origin = *v;
        }
        if let Some(v) = &self.outcome {
            f.outcome = *v;
        }
        if let Some(v) = &self.cwd {
            f.cwd = v.clone();
        }
        if let Some(v) = &self.occurred_at {
            f.occurred_at = *v;
        }
        if let Some(v) = &self.source_session {
            f.source_session = v.clone();
        }
        if let Some(v) = &self.host {
            f.host = v.clone();
        }
        if let Some(v) = &self.vector {
            f.vector = v.clone();
        }
    }
}

/// Auto-add columns added after the initial schema (e.g. `host`, 2026-05-20)
/// when an existing on-disk table predates them. Idempotent — skips when
/// the column is already present. Uses LanceDB's `add_columns(AllNulls)`
/// so existing rows get `host=NULL` rather than being wiped.
///
/// Limited to nullable additions; non-nullable / type-changing migrations
/// still fall back to the wipe-and-restart policy (`memory_recall_redesign`,
/// 2026-05-15). A single safe column add stays well under the complexity
/// bar that rejected the broader migration scheme.
async fn ensure_late_schema_additions(table: &lancedb::Table) -> Result<()> {
    use arrow_schema::{Field, Schema as ArrowSchema};
    let current = table
        .schema()
        .await
        .context("reading existing memory table schema")?;
    if current.field_with_name("host").is_ok() {
        return Ok(());
    }
    let add_schema = Arc::new(ArrowSchema::new(vec![Field::new(
        "host",
        arrow_schema::DataType::Utf8,
        true,
    )]));
    table
        .add_columns(NewColumnTransform::AllNulls(add_schema), None)
        .await
        .context("adding `host` column to existing memory table")?;
    tracing::info!("memory table: added `host` column (NULL for pre-existing rows)");
    Ok(())
}

/// Verify the existing table's `vector` column dimension matches the dim
/// the current build expects ([`super::schema::VECTOR_DIM`]). When a user
/// upgrades from a release indexed with a different embedding model
/// (e.g. v0.4.x's MiniLM-L6-v2 = 384-dim → v0.5+'s Qwen3-Embedding-0.6B
/// = 1024-dim), the data is incompatible and continuing would surface as
/// cryptic LanceDB errors on insert / search. Bail with a clear message
/// instead.
async fn check_schema_dim(table: &lancedb::Table, lancedb_dir: &Path) -> Result<()> {
    use arrow_schema::DataType;

    let schema = table
        .schema()
        .await
        .context("reading existing memory table schema")?;
    let vector_field = schema
        .field_with_name("vector")
        .context("existing memory table is missing the `vector` column")?;

    let actual_dim = match vector_field.data_type() {
        DataType::FixedSizeList(_, dim) => *dim,
        other => {
            return Err(anyhow!(
                "memory table `vector` column has unexpected type {other:?}; expected FixedSizeList<Float32>"
            ));
        }
    };

    let expected = super::schema::VECTOR_DIM;
    if actual_dim != expected {
        return Err(anyhow!(
            "ling-mem schema mismatch: existing data at `{}` was indexed with {actual_dim}-dim embeddings, but this build uses {expected}-dim Qwen3-Embedding-0.6B. \
             The two are incompatible and there is no forward migration — start fresh by removing the data dir:\n\n  rm -rf {}",
            lancedb_dir.display(),
            lancedb_dir.display()
        ));
    }
    Ok(())
}

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

/// Empirical cosine-distribution reference from the Qwen3-Embedding-0.6B
/// retune (`embed::tests::dedup_threshold_probe`, 2026-05-15): reworded
/// restatements scored 0.78–0.97, related-but-distinct facts 0.53–0.65.
///
/// **No longer an automatic dedup/merge gate.** Cosine cannot separate a
/// restatement from a single-token *contradiction* ("cat is male" vs
/// "cat is female" both score ~0.9) — using it to merge silently
/// corrupts a user fact. Mechanical dedup is now **exact-content** only
/// (see [`MemoryStore::insert_with_dedup`]); fuzzy "same fact?" is LLM
/// judgment (the Reconcile contract, `linggen/doc/memory-spec.md` §2).
/// Retained as a relevance reference and for the tuning probe.
pub const DEDUP_SIMILARITY_THRESHOLD: f32 = 0.75;

/// Outcome of [`MemoryStore::insert_with_dedup`]. Either the candidate was
/// inserted as a new row, or it collapsed into an existing one.
#[derive(Debug, Clone)]
pub enum InsertOutcome {
    /// No existing row had the same exact content; inserted fresh.
    Added(Memory),
    /// An existing row (same type) had byte-identical content; the
    /// candidate's contexts/tags were unioned into it and its id kept.
    Merged {
        /// The post-merge fact (same id as the existing row).
        fact: Memory,
        /// Always `1.0` — an exact-content match. Kept for telemetry/CLI
        /// shape compat; cosine is no longer the merge criterion.
        similarity: f32,
        /// Id of the existing row that absorbed the candidate (same as
        /// `fact.id`; surfaced separately for clarity in telemetry).
        previous_id: String,
    },
}

/// Dot-product cosine similarity for two equal-length vectors. Guards
/// against zero-length or mismatched-dim inputs by returning 0.0.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Merge a near-duplicate `candidate` into an `existing` row. Keeps the
/// existing id, created_at, and origin (original authorship); unions
/// contexts and tags; prefers the longer content (more signal). Scalar
/// optional fields on the candidate overwrite the existing value only
/// when the candidate actually carries a value — a `None` never clears.
fn merge_fact(existing: &Memory, candidate: &Memory) -> Memory {
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

/// The LanceDB-backed memory store.
pub struct MemoryStore {
    _conn: Connection,
    table: Table,
    /// Serializes read-modify-write mutations (dedup merge, update) against
    /// this table. The daemon shares one `MemoryStore` per table across a
    /// multi-thread Tokio runtime, so without this two concurrent
    /// `insert_with_dedup` calls on the same (content, type) could both
    /// observe "no existing row" (TOCTOU) and write duplicates, or a
    /// concurrent update+update on one id could lose a patch. Per-table:
    /// the semantic and episodic stores hold independent locks, so a
    /// cross-tier write touching both never self-deadlocks.
    write_lock: tokio::sync::Mutex<()>,
}

impl MemoryStore {
    /// Open (or auto-create) the curated long-term (semantic) store
    /// rooted at `data_dir`.
    ///
    /// The actual LanceDB directory is `data_dir/memory/memory.lancedb/` —
    /// appending `memory/memory.lancedb/` to the Linggen-per-user data dir.
    /// If the dir doesn't exist, both the directories and an empty
    /// `semantic` table are created.
    pub async fn open_semantic(data_dir: &Path) -> Result<Self> {
        Self::open_named(data_dir, SEMANTIC_TABLE_NAME).await
    }

    /// Open (or auto-create) the short-term (episodic) store rooted at
    /// `data_dir`.
    ///
    /// Lives in the same `data_dir/memory/memory.lancedb/` connection as
    /// [`Self::open_semantic`] but is a separate LanceDB table reusing the
    /// identical Memory schema. Separate tables give per-table ANN-index
    /// isolation, so staged episodic rows never pollute the curated
    /// semantic index.
    pub async fn open_episodic(data_dir: &Path) -> Result<Self> {
        Self::open_named(data_dir, EPISODIC_TABLE_NAME).await
    }

    /// Shared open-or-create body for [`Self::open_semantic`] /
    /// [`Self::open_episodic`]. Connects to the single
    /// `data_dir/memory/memory.lancedb/` directory and opens-or-creates
    /// `table_name` against [`build_schema`].
    async fn open_named(data_dir: &Path, table_name: &str) -> Result<Self> {
        let lancedb_dir = data_dir.join("memory").join("memory.lancedb");
        tokio::fs::create_dir_all(&lancedb_dir)
            .await
            .with_context(|| format!("creating memory dir at {}", lancedb_dir.display()))?;

        // Store schema-version gate (see schema_version + doc/schema-versioning.md).
        // Refuse a store written by a newer release, or one too old to migrate,
        // before touching it. Openable cases fall through and are stamped below.
        let compat = super::schema_version::classify(data_dir);
        if let Some(msg) = super::schema_version::refuse_message(compat, &lancedb_dir) {
            return Err(anyhow!(msg));
        }

        let uri = lancedb_dir
            .to_str()
            .ok_or_else(|| anyhow!("memory path is not valid UTF-8: {}", lancedb_dir.display()))?;

        let conn = connect(uri)
            .execute()
            .await
            .with_context(|| format!("opening LanceDB at {uri}"))?;

        let names = conn
            .table_names()
            .execute()
            .await
            .context("listing LanceDB tables")?;

        let table = if names.iter().any(|n| n == table_name) {
            let t = conn
                .open_table(table_name)
                .execute()
                .await
                .with_context(|| format!("opening `{table_name}` table"))?;
            check_schema_dim(&t, &lancedb_dir).await?;
            ensure_late_schema_additions(&t).await?;
            t
        } else {
            let schema = build_schema();
            // lancedb 0.29's create_table takes data implementing
            // Scannable, which covers a boxed RecordBatchReader but not
            // bare iterators — so box it.
            let empty: Box<dyn RecordBatchReader + Send> = Box::new(RecordBatchIterator::new(
                std::iter::empty::<arrow::error::Result<RecordBatch>>(),
                schema,
            ));
            conn.create_table(table_name, empty)
                .execute()
                .await
                .with_context(|| format!("creating `{table_name}` table"))?
        };

        // A store from an older (still-readable) schema generation must run
        // its registered migrations before being stamped current — stamping
        // without migrating would mark an unmigrated store as up to date.
        if let super::schema_version::Compat::Migrate { from } = compat {
            super::schema_version::run_migrations(from)?;
        }

        // Record the store schema version that opened this store. Idempotent —
        // writes only when the sidecar differs (so patch/minor binaries sharing
        // a STORE_SCHEMA_VERSION never churn it). For an adopted legacy store,
        // the per-table additive migrations above have already run.
        super::schema_version::stamp_current(data_dir)?;

        Ok(Self {
            _conn: conn,
            table,
            write_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Atomic upsert keyed on `id`: replace the row with a matching id if one
    /// exists, insert otherwise — in a single LanceDB `merge_insert`
    /// transaction. Replaces the old delete-then-insert round-trip, which
    /// left a window where a crash/cancel between the two ops destroyed the
    /// row with no replacement (the one true data-loss class in this layer).
    /// All three RMW callers (`insert_with_dedup` merge, `update`,
    /// `update_full_public`) preserve the existing id, so the id-keyed merge
    /// updates the row in place rather than duplicating it.
    async fn upsert(&self, fact: &Memory) -> Result<()> {
        let batch = memories_to_record_batch(std::slice::from_ref(fact))?;
        let schema = batch.schema();
        let batches: Box<dyn RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(std::iter::once(Ok(batch)), schema));

        let mut builder = self.table.merge_insert(&["id"]);
        builder.when_matched_update_all(None);
        builder.when_not_matched_insert_all();
        builder
            .execute(batches)
            .await
            .context("upserting memory row (merge_insert on id)")?;
        Ok(())
    }

    /// Insert one or more facts. Returns the number of rows written.
    ///
    /// Treats the store as append-only — existing IDs are not detected or
    /// deduplicated here. Callers who need upsert semantics should use
    /// `update()` (next commit) or check with `get()` first.
    pub async fn insert(&self, facts: &[Memory]) -> Result<usize> {
        if facts.is_empty() {
            return Ok(0);
        }

        let batch = memories_to_record_batch(facts)?;
        let schema = batch.schema();
        let batches: Box<dyn RecordBatchReader + Send> =
            Box::new(RecordBatchIterator::new(std::iter::once(Ok(batch)), schema));

        self.table
            .add(batches)
            .execute()
            .await
            .context("adding batch to memory table")?;

        Ok(facts.len())
    }

    /// Insert a single fact with **exact-content** dedup protection.
    ///
    /// If a row of the same `type` already has byte-identical content, the
    /// candidate's contexts/tags are unioned into it (see [`merge_fact`])
    /// and the existing id is preserved. Otherwise it is inserted fresh.
    ///
    /// Cosine similarity is deliberately **not** the merge criterion: it
    /// cannot tell a restatement from a single-token contradiction
    /// ("cat is male" vs "cat is female" both score ~0.9), so a cosine
    /// merge silently corrupts a user fact. Fuzzy "is this the same
    /// fact?" is LLM judgment — the Reconcile contract
    /// (`linggen/doc/memory-spec.md` §2), not this mechanical path.
    ///
    /// Vector-less candidates take the same path: the exact-content lookup
    /// is pure SQL, and an exact match has equal-length content, so
    /// [`merge_fact`] never replaces the existing row's vector with `None`.
    pub async fn insert_with_dedup(&self, fact: Memory) -> Result<InsertOutcome> {
        // Serialize the find→merge→write sequence. Without the lock, two
        // concurrent adds of the same (content, type) could both miss the
        // existing row and each insert a fresh copy.
        let _guard = self.write_lock.lock().await;

        if let Some(existing) = self.find_exact_content(&fact.content, fact.r#type).await? {
            let previous_id = existing.id.clone();
            let merged = merge_fact(&existing, &fact);
            // `merged` keeps `existing.id`, so the id-keyed upsert replaces the
            // row in place — atomic, no delete-then-insert window.
            self.upsert(&merged).await?;
            return Ok(InsertOutcome::Merged {
                fact: merged,
                similarity: 1.0,
                previous_id,
            });
        }

        self.insert(std::slice::from_ref(&fact)).await?;
        Ok(InsertOutcome::Added(fact))
    }

    /// Public wrapper for [`Self::find_exact_content`] so the HTTP
    /// layer can run cross-store dedup against the sibling table
    /// (semantic ↔ episodic) before calling `insert_with_dedup`.
    pub async fn find_exact_content_public(
        &self,
        content: &str,
        ty: MemoryType,
    ) -> Result<Option<Memory>> {
        self.find_exact_content(content, ty).await
    }

    /// Public wrapper for replacing a row by id — used by the cross-tier
    /// dedup path to merge contexts/tags into the existing higher-tier
    /// row when a lower-tier write hit the same (content, type).
    pub async fn update_full_public(&self, id: &str, replacement: &Memory) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        // Every current caller passes `replacement.id == id` (the merge
        // helpers clone the existing row), so the id-keyed upsert replaces
        // the right row in place — atomic, no delete window.
        self.upsert(replacement).await?;
        // Defensive: if a future caller ever changes the id, the upsert keyed
        // on the *new* id won't touch the old row — delete it so a stale
        // duplicate can't leak. Insert-then-delete here means the worst case
        // is a transient duplicate, never a lost row.
        if replacement.id != id {
            self.delete_one(id).await?;
        }
        Ok(())
    }

    /// Find one existing row of the same `type` whose content is
    /// byte-identical to `content`. Exact match only — see
    /// [`Self::insert_with_dedup`] for why cosine is never a sameness
    /// decision. `content` is a normal column (not a SQL keyword); `type`
    /// equality mirrors [`Filters::to_sql`].
    async fn find_exact_content(
        &self,
        content: &str,
        ty: MemoryType,
    ) -> Result<Option<Memory>> {
        let filter = format!(
            "content = '{}' AND type = '{}'",
            escape_sql(content),
            ty.as_str()
        );
        let q = self.table.query().only_if(filter).limit(1);
        let mut hits = self.collect_query(q).await?;
        Ok(hits.pop())
    }

    /// Fetch one fact by id. Returns `None` if the id is not present.
    pub async fn get(&self, id: &str) -> Result<Option<Memory>> {
        // LanceDB SQL filter — id is Utf8, so we compare against a literal.
        // Escape single quotes for safety even though base62 ids never carry
        // one (defensive — keeps the path robust if id format ever changes).
        let filter = format!("id = '{}'", escape_sql(id));

        let mut stream = self
            .table
            .query()
            .only_if(filter)
            .limit(1)
            .execute()
            .await
            .context("querying facts by id")?;

        while let Some(batch) = stream
            .try_next()
            .await
            .context("reading next batch from facts query")?
        {
            if batch.num_rows() == 0 {
                continue;
            }
            let facts = record_batch_to_memories(&batch)?;
            if let Some(fact) = facts.into_iter().next() {
                return Ok(Some(fact));
            }
        }
        Ok(None)
    }

    /// Total row count in the memory table. Useful for tests and dashboards.
    pub async fn count(&self) -> Result<usize> {
        self.table
            .count_rows(None)
            .await
            .context("counting rows in memory table")
    }

    /// Row count constrained by `filters`. Pushes filter SQL into LanceDB
    /// so the count is a metadata-only operation (no row materialization).
    /// Empty filters → same as [`count`]. `origin` is ignored at the SQL
    /// layer (see [`Filters::to_sql`] on the `from` keyword bug); pre-existing
    /// callers that need an exact origin-scoped count still have to fall
    /// back to `list` + post-filter.
    pub async fn count_filtered(&self, filters: &Filters) -> Result<usize> {
        let predicate = filters.to_sql();
        self.table
            .count_rows(predicate)
            .await
            .context("counting filtered rows in memory table")
    }

    /// Nearest-neighbor search over `vector`, constrained by `filters`.
    ///
    /// Returns up to `limit` facts sorted by vector similarity (closest
    /// first). Rows with null vectors are never returned — they wouldn't
    /// have a similarity score.
    ///
    /// Thin wrapper around [`Self::search_scored`] that drops the per-row
    /// score. Callers that want to gate on similarity should use
    /// `search_scored` directly.
    pub async fn search(
        &self,
        query_vec: &[f32],
        filters: &Filters,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        let scored = self.search_scored(query_vec, filters, limit, None).await?;
        Ok(scored.into_iter().map(|(f, _)| f).collect())
    }

    /// Nearest-neighbor search that also returns the cosine similarity of
    /// each row against the query, with an optional `min_score` floor that
    /// drops rows below the threshold.
    ///
    /// Score range is `[-1.0, 1.0]` (cosine of unit-normalized embeddings,
    /// effectively `[0.0, 1.0]` in practice for Qwen3-Embedding-0.6B outputs). A
    /// `min_score` of `Some(0.5)` keeps moderately-relevant hits and drops
    /// noise; `None` disables filtering. Computed post-fetch from the
    /// stored vectors — same arithmetic as the dedup path.
    pub async fn search_scored(
        &self,
        query_vec: &[f32],
        filters: &Filters,
        limit: usize,
        min_score: Option<f32>,
    ) -> Result<Vec<(Memory, f32)>> {
        if query_vec.len() != super::schema::VECTOR_DIM as usize {
            return Err(anyhow!(
                "query vector has len {} but schema dim is {}",
                query_vec.len(),
                super::schema::VECTOR_DIM
            ));
        }

        // Pin lancedb's metric to Cosine so its top-K selection and
        // ordering match the `cosine_similarity` we recompute post-fetch
        // (and the dedup threshold). lancedb defaults to L2; for the
        // unit-normalized Qwen3 embeddings L2 and Cosine rank identically,
        // so this is a no-op behaviorally — but it makes the absence of a
        // post-fetch re-sort correct by construction rather than by the
        // normalization coincidence. Safe here because no ANN index is
        // trained (flat search), so there is no index metric to mismatch;
        // revisit if an IVF/PQ index is added.
        let mut q = self
            .table
            .vector_search(query_vec.to_vec())
            .context("starting vector search")?
            .distance_type(DistanceType::Cosine)
            .limit(limit);

        if let Some(sql) = filters.to_sql() {
            q = q.only_if(sql);
        }

        let mut facts = self.collect_query(q).await?;
        apply_origin_filter(&mut facts, filters.origin);

        let mut scored: Vec<(Memory, f32)> = facts
            .into_iter()
            .map(|f| {
                let score = f
                    .vector
                    .as_ref()
                    .map(|v| cosine_similarity(query_vec, v))
                    .unwrap_or(0.0);
                (f, score)
            })
            .collect();

        if let Some(threshold) = min_score {
            scored.retain(|(_, score)| *score >= threshold);
        }
        Ok(scored)
    }

    /// Fetch the full filtered row set and pair each with its cosine
    /// similarity to `query_vec` — the candidate pool for hybrid retrieval.
    ///
    /// Unlike [`Self::search_scored`] this does **not** push a top-K limit
    /// into LanceDB: BM25 (the lexical half of hybrid search, computed by the
    /// caller) needs the whole corpus for its document-frequency / average-
    /// length statistics, and a keyword-only match must remain a candidate
    /// even when its cosine would rank it well outside the vector top-K.
    /// Rows with a null vector get cosine `0.0` (they can still be lexical
    /// hits). Same flat-scan posture as [`Self::list`]; revisit past ~100k
    /// rows.
    pub async fn scored_candidates(
        &self,
        query_vec: &[f32],
        filters: &Filters,
    ) -> Result<Vec<super::Candidate>> {
        if query_vec.len() != super::schema::VECTOR_DIM as usize {
            return Err(anyhow!(
                "query vector has len {} but schema dim is {}",
                query_vec.len(),
                super::schema::VECTOR_DIM
            ));
        }

        let mut q = self.table.query();
        if let Some(sql) = filters.to_sql() {
            q = q.only_if(sql);
        }
        let mut facts = self.collect_query(q).await?;
        apply_origin_filter(&mut facts, filters.origin);

        Ok(facts
            .into_iter()
            .map(|f| {
                let cosine = f
                    .vector
                    .as_ref()
                    .map(|v| cosine_similarity(query_vec, v))
                    .unwrap_or(0.0);
                super::Candidate { memory: f, cosine }
            })
            .collect())
    }

    /// Hybrid (dense + lexical) search over this one table. Fetches the
    /// candidate pool ([`Self::scored_candidates`]) and scores each row as
    /// cosine + an IDF-weighted keyword boost (see [`super::hybrid::fuse`]).
    /// Returns up to `limit` `(memory, cosine, hybrid)` hits ordered by the
    /// hybrid score; `min_score` gates that hybrid score.
    pub async fn hybrid_scored(
        &self,
        query_vec: &[f32],
        query_text: &str,
        filters: &Filters,
        limit: usize,
        min_score: Option<f32>,
    ) -> Result<Vec<(Memory, f32, f32)>> {
        let candidates = self.scored_candidates(query_vec, filters).await?;
        Ok(super::hybrid::fuse(candidates, query_text, limit, min_score))
    }

    /// Non-semantic browse. Returns up to `limit` facts matching `filters`,
    /// skipping the first `offset` in sort order.
    ///
    /// Sorting is done in-process after the batch returns — LanceDB's query
    /// builder doesn't expose an ORDER BY equivalent for list-style queries
    /// without a vector, and our sort key is `activity_timestamp`
    /// (`COALESCE(updated_at, created_at)`), not a plain column. Because the
    /// sort happens after the fetch, a DB-side `limit` would cap the scan to
    /// an ARBITRARY set of rows before ordering — `list(Newest, 1)` would
    /// return a random row, and page 1 of a store larger than the page size
    /// would be a random page sorted among itself, not the global newest.
    /// So we fetch the FULL filtered set, sort, then slice the
    /// `[offset .. offset+limit]` window. For v0.1 row counts this is fine
    /// (same posture as `core_facts`); revisit with a DB-side sort if files
    /// grow past ~100k rows.
    pub async fn list(
        &self,
        filters: &Filters,
        order: SortOrder,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Memory>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        // No LanceDB-side `limit`: the in-process sort below must see every
        // matching row, or pagination returns an arbitrary page (see the
        // doc comment). Filtering is pushed into SQL where possible; the
        // origin filter is applied post-fetch (`apply_origin_filter`) due to
        // a LanceDB keyword bug.
        let mut q = self.table.query();
        if let Some(sql) = filters.to_sql() {
            q = q.only_if(sql);
        }

        let mut facts = self.collect_query(q).await?;
        apply_origin_filter(&mut facts, filters.origin);

        // List = "what was written recently" — sort by activity_timestamp
        // (updated_at ?? created_at), the SAME key the UI shows as the
        // "N ago" age, so sort position and badge agree. NOT
        // effective_timestamp: that falls back to occurred_at, which the
        // LLM/consolidator routinely back-dates to the source session
        // ("today midnight UTC" for same-day facts), collapsing many
        // newest rows to one key and burying freshly-written rows.
        facts.sort_by(|a, b| match order {
            SortOrder::Newest => b.activity_timestamp().cmp(&a.activity_timestamp()),
            SortOrder::Oldest => a.activity_timestamp().cmp(&b.activity_timestamp()),
        });

        if offset >= facts.len() {
            return Ok(Vec::new());
        }
        facts.drain(..offset);
        facts.truncate(limit);
        Ok(facts)
    }

    /// Update a single fact by id. Applies `patch` on top of the existing
    /// row, then writes the result back. Returns the post-patch fact, or
    /// `None` if the id doesn't exist.
    ///
    /// v0.1 uses read-modify-write (delete + insert). LanceDB supports
    /// column-level updates for simple types, but our nested schema (lists,
    /// FixedSizeList vectors, timezone-tagged timestamps) is cleaner to
    /// round-trip through [`Memory`] than to express as SQL column setters.
    pub async fn update(&self, id: &str, patch: &MemoryPatch) -> Result<Option<Memory>> {
        // Serialize the read-modify-write so a concurrent update on the same
        // id can't clobber this patch (last-writer-wins data loss).
        let _guard = self.write_lock.lock().await;
        let Some(mut existing) = self.get(id).await? else {
            return Ok(None);
        };
        patch.apply(&mut existing);
        // Touch the decay/TTL clock. Microsecond-truncated to match the
        // `created_at` precision (see `Memory::new`) so the value round-trips
        // through LanceDB's `Timestamp(Microsecond)` column intact.
        existing.updated_at = Some(Utc::now().trunc_subsecs(6));
        // `existing.id == id`, so the id-keyed upsert replaces in place —
        // atomic, no delete-then-insert window.
        self.upsert(&existing).await?;
        Ok(Some(existing))
    }

    /// Hard-delete a single fact by id. Returns `true` if a row was
    /// removed, `false` if the id wasn't present.
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let removed = self.delete_one(id).await?;
        Ok(removed > 0)
    }

    /// Bulk-delete by filter. Returns the number of rows removed.
    ///
    /// Safety: refuses to run with an empty filter — use `Filters::default()`
    /// would match every row, which is almost always a bug. Callers who
    /// genuinely want "forget everything" should iterate over all rows and
    /// delete by id, or drop the table directory externally.
    ///
    /// When `origin` is set, we can't push it as a SQL clause (LanceDB
    /// reserved-word bug — see `apply_origin_filter`), so we fall back to
    /// list-then-delete-by-id. The per-row delete cost is fine at our scale.
    pub async fn forget(&self, filters: &Filters) -> Result<usize> {
        // Safety: an `origin`-only filter would render `to_sql()` as `None`
        // (since origin isn't pushed into SQL anymore). Treat that as
        // non-empty to avoid the "refusing to forget with empty filter"
        // guard accidentally rejecting a perfectly valid origin-only request.
        let predicate = filters.to_sql();
        let has_any_filter = predicate.is_some() || filters.origin.is_some();
        if !has_any_filter {
            return Err(anyhow!(
                "refusing to forget with empty filter — provide at least one criterion"
            ));
        }

        if filters.origin.is_some() {
            // Origin filter active → list rows that match, delete each by id.
            // Use a generous limit to cover the whole store; pagination here
            // would just complicate the count math.
            let mut victims = self
                .list(filters, SortOrder::Newest, usize::MAX, 0)
                .await?;
            apply_origin_filter(&mut victims, filters.origin);
            let mut removed = 0;
            for v in victims {
                removed += self.delete_one(&v.id).await? as usize;
            }
            return Ok(removed);
        }

        let predicate =
            predicate.expect("predicate exists when origin is None and filter non-empty");
        self.delete_where(&predicate)
            .await
            .context("deleting memories by filter")
    }

    /// Evict rows whose decay clock has fallen before `cutoff`. Deletes
    /// every row where `COALESCE(updated_at, created_at) < cutoff` — i.e.
    /// an `update` resets a row's age (the touch-resets-clock semantics).
    /// Returns the number of rows removed.
    ///
    /// Used against the episodic store: the consolidation pass calls this
    /// with `cutoff = now − Settings.EPISODIC_TTL` to drop staged rows that
    /// were never promoted into curated facts. There is no TTL column and
    /// no TTL argument — the caller owns the TTL policy and passes only the
    /// resolved cutoff instant.
    pub async fn evict_expired(&self, cutoff: DateTime<Utc>) -> Result<usize> {
        let predicate = format!(
            "COALESCE(updated_at, created_at) < TIMESTAMP '{}'",
            cutoff.format("%Y-%m-%d %H:%M:%S%.6f")
        );
        self.delete_where(&predicate)
            .await
            .context("evicting expired memories")
    }

    /// All `tier='core'` facts — the always-load identity/preference set.
    ///
    /// No vector, no similarity gate, no pagination cap: core is the small
    /// durable set surfaced eagerly on every turn, so it's read whole rather
    /// than retrieved. Filters by `tier = 'core'` in SQL (no LanceDB row
    /// limit — `list`'s over-fetch math can't express "unbounded"), then
    /// sorts newest-first in process to match `list`'s default order.
    pub async fn core_facts(&self) -> Result<Vec<Memory>> {
        let filters = Filters {
            tier: Some(Tier::Core),
            ..Filters::default()
        };
        let mut q = self.table.query();
        if let Some(sql) = filters.to_sql() {
            q = q.only_if(sql);
        }
        let mut facts = self.collect_query(q).await?;
        facts.sort_by(|a, b| b.activity_timestamp().cmp(&a.activity_timestamp()));
        Ok(facts)
    }

    /// Count the rows matching `predicate`, delete them, and return that
    /// pre-delete count as the number removed.
    ///
    /// lancedb 0.29's `Table::delete` yields only a `DeleteResult` (table
    /// version, no deleted-row count), so an exact single-call number
    /// isn't available. Counting the *predicate* — not the whole table —
    /// before the delete replaces two full-table `count_rows(None)` scans
    /// with one scoped scan and narrows the count→delete race window:
    /// under the shared-`MemoryStore` daemon (`Arc<AppState>`), the old
    /// `count()-before − count()-after` could be skewed by any concurrent
    /// insert/delete on an unrelated row, whereas now only a concurrent
    /// change to rows matching `predicate` can affect the reported count.
    /// The delete itself is always complete regardless; only the returned
    /// tally is best-effort.
    async fn delete_where(&self, predicate: &str) -> Result<usize> {
        let removed = self
            .table
            .count_rows(Some(predicate.to_string()))
            .await
            .context("counting rows matching delete predicate")?;
        self.table
            .delete(predicate)
            .await
            .context("executing delete")?;
        Ok(removed)
    }

    /// Delete the single row with the given id. Returns the number removed
    /// (0 or 1 in practice). Private — public API uses `delete` / `update`.
    async fn delete_one(&self, id: &str) -> Result<usize> {
        self.delete_where(&format!("id = '{}'", escape_sql(id)))
            .await
            .with_context(|| format!("deleting memory {id}"))
    }

    async fn collect_query<Q: ExecutableQuery>(&self, q: Q) -> Result<Vec<Memory>> {
        let mut stream = q.execute().await.context("executing facts query")?;
        let mut out: Vec<Memory> = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .context("reading next batch from facts query")?
        {
            if batch.num_rows() == 0 {
                continue;
            }
            out.extend(record_batch_to_memories(&batch)?);
        }
        Ok(out)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::{MemoryType, Origin, Outcome};
    use tempfile::TempDir;

    async fn fresh_store() -> (MemoryStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = MemoryStore::open_semantic(dir.path()).await.unwrap();
        (store, dir)
    }

    fn make_fact(content: &str, t: MemoryType) -> Memory {
        Memory::new(content, t, Origin::User)
    }

    #[tokio::test]
    async fn opens_and_creates_empty_table() {
        let (store, _dir) = fresh_store().await;
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        let dir = TempDir::new().unwrap();
        {
            let store = MemoryStore::open_semantic(dir.path()).await.unwrap();
            store
                .insert(&[make_fact("first", MemoryType::Fact)])
                .await
                .unwrap();
        }
        // Re-opening finds the table and sees the existing row.
        let store = MemoryStore::open_semantic(dir.path()).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn episodic_table_is_separate_from_facts() {
        let dir = TempDir::new().unwrap();
        // Same lancedb dir, two tables in one connection.
        let facts = MemoryStore::open_semantic(dir.path()).await.unwrap();
        let episodic = MemoryStore::open_episodic(dir.path()).await.unwrap();

        let fact = make_fact("curated fact", MemoryType::Fact);
        let fact_id = fact.id.clone();
        facts.insert(&[fact]).await.unwrap();
        episodic
            .insert(&[make_fact("staged experience", MemoryType::Learned)])
            .await
            .unwrap();

        // Each table holds exactly its own row.
        assert_eq!(facts.count().await.unwrap(), 1);
        assert_eq!(episodic.count().await.unwrap(), 1);

        // True isolation: the semantic row is invisible to the episodic store
        // even though they share one LanceDB connection.
        assert!(episodic.get(&fact_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn open_episodic_is_idempotent() {
        let dir = TempDir::new().unwrap();
        {
            let store = MemoryStore::open_episodic(dir.path()).await.unwrap();
            store
                .insert(&[make_fact("staged", MemoryType::Learned)])
                .await
                .unwrap();
        }
        // Re-opening finds the episodic table and sees the existing row.
        let store = MemoryStore::open_episodic(dir.path()).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn insert_returns_row_count() {
        let (store, _dir) = fresh_store().await;
        let facts = vec![
            make_fact("a", MemoryType::Fact),
            make_fact("b", MemoryType::Preference),
        ];
        assert_eq!(store.insert(&facts).await.unwrap(), 2);
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn insert_empty_slice_is_a_noop() {
        let (store, _dir) = fresh_store().await;
        assert_eq!(store.insert(&[]).await.unwrap(), 0);
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn get_finds_inserted_fact() {
        let (store, _dir) = fresh_store().await;
        let mut f = make_fact("find me", MemoryType::Learned);
        f.contexts = vec!["env/macos".into()];
        f.tags = vec!["topic:dates".into()];
        f.outcome = Some(Outcome::Positive);
        let id = f.id.clone();
        store.insert(&[f.clone()]).await.unwrap();

        let got = store.get(&id).await.unwrap().expect("fact missing");
        assert_eq!(got, f);
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_id() {
        let (store, _dir) = fresh_store().await;
        store
            .insert(&[make_fact("x", MemoryType::Fact)])
            .await
            .unwrap();
        let missing = crate::memory::types::short_id();
        assert!(store.get(&missing).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn insert_many_then_count() {
        let (store, _dir) = fresh_store().await;
        let facts: Vec<Memory> = (0..50)
            .map(|i| make_fact(&format!("f{i}"), MemoryType::Fact))
            .collect();
        store.insert(&facts).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 50);
    }

    // ── search + list tests ────────────────────────────────────────────────

    use super::super::schema::VECTOR_DIM;
    use chrono::Duration;

    fn with_vector(mut f: Memory, value: f32) -> Memory {
        f.vector = Some(vec![value; VECTOR_DIM as usize]);
        f
    }

    #[tokio::test]
    async fn filters_to_sql_is_none_for_default() {
        assert!(Filters::default().to_sql().is_none());
    }

    #[tokio::test]
    async fn filters_sql_builds_single_clauses() {
        let f = Filters {
            contexts: vec!["code/linggen".into()],
            types: vec![MemoryType::Fixed],
            origin: Some(Origin::User),
            outcome: Some(Outcome::Positive),
            tier: Some(Tier::Core),
            since: None,
            until: None,
            source_session: None,
        };
        let sql = f.to_sql().unwrap();
        assert!(sql.contains("array_has(contexts, 'code/linggen')"));
        assert!(sql.contains("type = 'fixed'"));
        // origin/from is intentionally NOT in the SQL — applied post-fetch
        // because LanceDB returns 0 rows for `"from" = 'user'` even though
        // the column has matching values.
        assert!(!sql.contains("\"from\""));
        assert!(sql.contains("outcome = 'positive'"));
        // tier IS pushed down — not a SQL keyword, so the plain clause works.
        assert!(sql.contains("tier = 'core'"));
        assert!(sql.contains(" AND "));
    }

    #[tokio::test]
    async fn list_with_empty_filters_returns_all_newest_first() {
        let (store, _dir) = fresh_store().await;

        // list() orders by activity_timestamp (updated_at ?? created_at), so the
        // fixtures have to differ THERE. Stamping only occurred_at leaves all
        // three tied on the real key — successive Utc::now() calls share one
        // microsecond ~86% of the time — and a tied stable sort just returns
        // scan order, which is the store's to choose, not ours to assert.
        let mut old = make_fact("old", MemoryType::Fact);
        old.created_at = Utc::now() - Duration::days(3);
        let mut mid = make_fact("mid", MemoryType::Fact);
        mid.created_at = Utc::now() - Duration::days(1);
        let mut new = make_fact("new", MemoryType::Fact);
        // Back-dated occurred_at on the NEWEST row: list must not reorder on it.
        // That is effective_timestamp's key, deliberately not this one.
        new.occurred_at = Some(Utc::now() - Duration::days(10));

        store.insert(&[old, mid, new]).await.unwrap();

        let results = store
            .list(&Filters::default(), SortOrder::Newest, 10, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].content, "new");
        assert_eq!(results[1].content, "mid");
        assert_eq!(results[2].content, "old");

        let oldest_first = store
            .list(&Filters::default(), SortOrder::Oldest, 10, 0)
            .await
            .unwrap();
        assert_eq!(oldest_first[0].content, "old");
        assert_eq!(oldest_first[2].content, "new");
    }

    #[tokio::test]
    async fn list_returns_global_newest_when_limit_below_total() {
        // Regression: `list` must sort the FULL filtered set before slicing
        // the page window. A DB-side limit-then-sort returns an arbitrary
        // `limit` rows — the bug that made list(Newest, 1) (and the dashboard
        // "latest" badge) pick a random row, and would corrupt page 1 once a
        // store exceeds the page size.
        let (store, _dir) = fresh_store().await;

        // Six rows one hour apart, inserted OLDEST-first so the newest rows
        // are written last — a fetch of the first N would miss them.
        let base = Utc::now() - Duration::days(1);
        let mut facts = Vec::new();
        for i in 0..6 {
            let mut f = make_fact(&format!("row{i}"), MemoryType::Fact);
            f.created_at = base + Duration::hours(i);
            facts.push(f);
        }
        store.insert(&facts).await.unwrap();

        // limit 2 of 6 → the two GLOBAL newest, in order.
        let top2 = store
            .list(&Filters::default(), SortOrder::Newest, 2, 0)
            .await
            .unwrap();
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].content, "row5");
        assert_eq!(top2[1].content, "row4");

        // limit 1 → the single global newest (the count endpoint relies on this).
        let top1 = store
            .list(&Filters::default(), SortOrder::Newest, 1, 0)
            .await
            .unwrap();
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].content, "row5");
    }

    #[tokio::test]
    async fn list_offset_pages_through_results() {
        let (store, _dir) = fresh_store().await;

        let mut a = make_fact("a", MemoryType::Fact);
        a.occurred_at = Some(Utc::now() - Duration::days(5));
        let mut b = make_fact("b", MemoryType::Fact);
        b.occurred_at = Some(Utc::now() - Duration::days(4));
        let mut c = make_fact("c", MemoryType::Fact);
        c.occurred_at = Some(Utc::now() - Duration::days(3));
        let mut d = make_fact("d", MemoryType::Fact);
        d.occurred_at = Some(Utc::now() - Duration::days(2));
        let mut e = make_fact("e", MemoryType::Fact);
        e.occurred_at = Some(Utc::now() - Duration::days(1));

        store.insert(&[a, b, c, d, e]).await.unwrap();

        let page1 = store
            .list(&Filters::default(), SortOrder::Oldest, 2, 0)
            .await
            .unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].content, "a");
        assert_eq!(page1[1].content, "b");

        let page2 = store
            .list(&Filters::default(), SortOrder::Oldest, 2, 2)
            .await
            .unwrap();
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].content, "c");
        assert_eq!(page2[1].content, "d");

        let page3 = store
            .list(&Filters::default(), SortOrder::Oldest, 2, 4)
            .await
            .unwrap();
        assert_eq!(page3.len(), 1);
        assert_eq!(page3[0].content, "e");

        let past_end = store
            .list(&Filters::default(), SortOrder::Oldest, 2, 10)
            .await
            .unwrap();
        assert!(past_end.is_empty());
    }

    #[tokio::test]
    async fn list_filters_by_type_and_context() {
        let (store, _dir) = fresh_store().await;

        let mut a = make_fact("a", MemoryType::Preference);
        a.contexts = vec!["global".into()];
        let mut b = make_fact("b", MemoryType::Fixed);
        b.contexts = vec!["code/linggen".into()];
        let mut c = make_fact("c", MemoryType::Fixed);
        c.contexts = vec!["code/sanji".into()];

        store.insert(&[a, b, c]).await.unwrap();

        let only_fixed_in_linggen = store
            .list(
                &Filters {
                    contexts: vec!["code/linggen".into()],
                    types: vec![MemoryType::Fixed],
                    ..Default::default()
                },
                SortOrder::Newest,
                10,
                0,
            )
            .await
            .unwrap();

        assert_eq!(only_fixed_in_linggen.len(), 1);
        assert_eq!(only_fixed_in_linggen[0].content, "b");
    }

    #[tokio::test]
    async fn list_types_are_or_combined() {
        let (store, _dir) = fresh_store().await;
        let a = make_fact("pref", MemoryType::Preference);
        let b = make_fact("fix", MemoryType::Fixed);
        let c = make_fact("tried", MemoryType::Tried);
        store.insert(&[a, b, c]).await.unwrap();

        let prefs_or_fixes = store
            .list(
                &Filters {
                    types: vec![MemoryType::Preference, MemoryType::Fixed],
                    ..Default::default()
                },
                SortOrder::Newest,
                10,
                0,
            )
            .await
            .unwrap();
        assert_eq!(prefs_or_fixes.len(), 2);
    }

    #[tokio::test]
    async fn list_filters_by_time_range() {
        let (store, _dir) = fresh_store().await;
        let now = Utc::now();

        let mut old = make_fact("old", MemoryType::Fact);
        old.occurred_at = Some(now - Duration::days(10));
        let mut recent = make_fact("recent", MemoryType::Fact);
        recent.occurred_at = Some(now - Duration::hours(2));

        store.insert(&[old, recent]).await.unwrap();

        let since_yesterday = store
            .list(
                &Filters {
                    since: Some(now - Duration::days(1)),
                    ..Default::default()
                },
                SortOrder::Newest,
                10,
                0,
            )
            .await
            .unwrap();
        assert_eq!(since_yesterday.len(), 1);
        assert_eq!(since_yesterday[0].content, "recent");
    }

    #[tokio::test]
    async fn search_returns_nearest_first() {
        let (store, _dir) = fresh_store().await;

        // Three facts in distinct directions (unit basis vectors) — the
        // production shape: unit-normalized, non-collinear embeddings.
        // Collinear constant vectors would all tie at cosine 1.0 and the
        // ordering assertion would be meaningless.
        let mut a = make_fact("a", MemoryType::Fact);
        a.vector = Some(unit_vec_at(0));
        let mut b = make_fact("b", MemoryType::Fact);
        b.vector = Some(unit_vec_at(1));
        let mut c = make_fact("c", MemoryType::Fact);
        c.vector = Some(unit_vec_at(2));
        store
            .insert(&[a.clone(), b.clone(), c.clone()])
            .await
            .unwrap();

        // Query pointing along `a`'s direction → cosine 1.0 with `a`,
        // 0.0 with `b`/`c` → `a` ranks first.
        let query = unit_vec_at(0);
        let results = store.search(&query, &Filters::default(), 3).await.unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].content, "a");
    }

    #[tokio::test]
    async fn search_respects_limit() {
        let (store, _dir) = fresh_store().await;
        // Distinct unit directions — non-zero so cosine is defined for
        // every row (a zero vector has no direction and would be dropped
        // under the Cosine metric).
        let facts: Vec<Memory> = (0..10)
            .map(|i| {
                let mut f = make_fact(&format!("f{i}"), MemoryType::Fact);
                f.vector = Some(unit_vec_at(i));
                f
            })
            .collect();
        store.insert(&facts).await.unwrap();

        let query = unit_vec_at(0);
        let results = store.search(&query, &Filters::default(), 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn search_with_context_filter() {
        let (store, _dir) = fresh_store().await;
        let mut in_ctx = with_vector(make_fact("in-ctx", MemoryType::Fact), 0.3);
        in_ctx.contexts = vec!["music/piano".into()];
        let mut other = with_vector(make_fact("other", MemoryType::Fact), 0.3);
        other.contexts = vec!["code/linggen".into()];
        store.insert(&[in_ctx, other]).await.unwrap();

        let query = vec![0.3; VECTOR_DIM as usize];
        let results = store
            .search(
                &query,
                &Filters {
                    contexts: vec!["music/piano".into()],
                    ..Default::default()
                },
                10,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "in-ctx");
    }

    #[tokio::test]
    async fn search_rejects_wrong_dim() {
        let (store, _dir) = fresh_store().await;
        let err = store
            .search(&[0.0; 100], &Filters::default(), 1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains(&format!("dim is {VECTOR_DIM}")));
    }

    // ── update + delete + forget tests ─────────────────────────────────────

    #[tokio::test]
    async fn delete_removes_row() {
        let (store, _dir) = fresh_store().await;
        let f = make_fact("x", MemoryType::Fact);
        let id = f.id.clone();
        store.insert(&[f]).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        assert!(store.delete(&id).await.unwrap());
        assert_eq!(store.count().await.unwrap(), 0);

        // Second delete is a no-op but not an error.
        assert!(!store.delete(&id).await.unwrap());
    }

    #[tokio::test]
    async fn update_applies_patch() {
        let (store, _dir) = fresh_store().await;
        let mut f = make_fact("original", MemoryType::Fact);
        f.contexts = vec!["a".into()];
        let id = f.id.clone();
        store.insert(&[f]).await.unwrap();

        let patch = MemoryPatch {
            content: Some("edited".into()),
            contexts: Some(vec!["b".into(), "c".into()]),
            outcome: Some(Some(Outcome::Positive)),
            ..Default::default()
        };

        let updated = store.update(&id, &patch).await.unwrap().unwrap();
        assert_eq!(updated.content, "edited");
        assert_eq!(updated.contexts, vec!["b".to_string(), "c".into()]);
        assert_eq!(updated.outcome, Some(Outcome::Positive));

        // Persisted.
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got, updated);
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn update_can_clear_optional_field() {
        let (store, _dir) = fresh_store().await;
        let mut f = make_fact("x", MemoryType::Tried);
        f.outcome = Some(Outcome::Negative);
        let id = f.id.clone();
        store.insert(&[f]).await.unwrap();

        // Some(None) clears to null.
        let patch = MemoryPatch {
            outcome: Some(None),
            ..Default::default()
        };
        let updated = store.update(&id, &patch).await.unwrap().unwrap();
        assert_eq!(updated.outcome, None);
    }

    #[tokio::test]
    async fn update_missing_id_returns_none() {
        let (store, _dir) = fresh_store().await;
        let result = store
            .update(&crate::memory::types::short_id(), &MemoryPatch::default())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn update_stamps_updated_at() {
        let (store, _dir) = fresh_store().await;
        let f = make_fact("before", MemoryType::Fact);
        let id = f.id.clone();
        let created_at = f.created_at;
        assert!(f.updated_at.is_none());
        store.insert(&[f]).await.unwrap();

        let patch = MemoryPatch {
            content: Some("after".into()),
            ..Default::default()
        };
        let updated = store.update(&id, &patch).await.unwrap().unwrap();
        let stamped = updated.updated_at.expect("update must stamp updated_at");
        assert!(stamped >= created_at);

        // Persisted through the store roundtrip, not just the return value.
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.updated_at, Some(stamped));
    }

    #[tokio::test]
    async fn forget_bulk_deletes_matching() {
        let (store, _dir) = fresh_store().await;
        let mut sanji1 = make_fact("s1", MemoryType::Fact);
        sanji1.contexts = vec!["code/sanji".into()];
        let mut sanji2 = make_fact("s2", MemoryType::Fact);
        sanji2.contexts = vec!["code/sanji".into()];
        let mut ling = make_fact("l1", MemoryType::Fact);
        ling.contexts = vec!["code/linggen".into()];
        store.insert(&[sanji1, sanji2, ling]).await.unwrap();

        let removed = store
            .forget(&Filters {
                contexts: vec!["code/sanji".into()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(removed, 2);
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn forget_refuses_empty_filter() {
        let (store, _dir) = fresh_store().await;
        store
            .insert(&[make_fact("x", MemoryType::Fact)])
            .await
            .unwrap();

        let err = store.forget(&Filters::default()).await.unwrap_err();
        assert!(err.to_string().contains("empty filter"));
        assert_eq!(store.count().await.unwrap(), 1);
    }

    // ── evict_expired + core_facts tests ───────────────────────────────────

    #[tokio::test]
    async fn evict_expired_drops_only_rows_older_than_cutoff() {
        let (store, _dir) = fresh_store().await;
        let now = Utc::now();

        // Old: created 10 days ago, never updated → decay clock = created_at.
        let mut old = make_fact("stale", MemoryType::Learned);
        old.created_at = (now - Duration::days(10)).trunc_subsecs(6);
        let old_id = old.id.clone();

        // Fresh: created just now → decay clock = created_at ≈ now.
        let fresh = make_fact("recent", MemoryType::Learned);
        let fresh_id = fresh.id.clone();

        // Touched: created 10 days ago BUT updated 1 hour ago → decay clock
        // = updated_at, which is recent → must survive (touch resets age).
        let mut touched = make_fact("revived", MemoryType::Learned);
        touched.created_at = (now - Duration::days(10)).trunc_subsecs(6);
        touched.updated_at = Some((now - Duration::hours(1)).trunc_subsecs(6));
        let touched_id = touched.id.clone();

        store.insert(&[old, fresh, touched]).await.unwrap();

        // Cutoff one day ago: only `old` is older by COALESCE clock.
        let removed = store.evict_expired(now - Duration::days(1)).await.unwrap();
        assert_eq!(removed, 1);
        assert!(store.get(&old_id).await.unwrap().is_none());
        assert!(store.get(&fresh_id).await.unwrap().is_some());
        assert!(
            store.get(&touched_id).await.unwrap().is_some(),
            "row with recent updated_at must survive even though created_at is old"
        );
    }

    #[tokio::test]
    async fn core_facts_returns_only_core_tier() {
        let (store, _dir) = fresh_store().await;

        let mut core1 = make_fact("identity bit", MemoryType::Fact);
        core1.tier = Tier::Core;
        let mut core2 = make_fact("preference bit", MemoryType::Preference);
        core2.tier = Tier::Core;
        let semantic = make_fact("retrieval pool bit", MemoryType::Learned); // default Semantic

        store.insert(&[core1, core2, semantic]).await.unwrap();

        let core = store.core_facts().await.unwrap();
        assert_eq!(core.len(), 2);
        assert!(core.iter().all(|f| f.tier == Tier::Core));
    }

    // ── insert_with_dedup tests ────────────────────────────────────────────

    /// Unit vector with a single 1.0 at `idx`; cosine similarity with its
    /// twin is 1.0, with a disjoint-index vector is 0.0. Sidesteps flaky
    /// float comparisons against the real embedding distribution.
    fn unit_vec_at(idx: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; VECTOR_DIM as usize];
        v[idx] = 1.0;
        v
    }

    #[tokio::test]
    async fn dedup_adds_when_store_is_empty() {
        let (store, _dir) = fresh_store().await;
        let f = with_vector(make_fact("first", MemoryType::Fact), 1.0);
        let outcome = store.insert_with_dedup(f.clone()).await.unwrap();
        match outcome {
            InsertOutcome::Added(got) => assert_eq!(got.id, f.id),
            InsertOutcome::Merged { .. } => panic!("empty store should never merge"),
        }
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn dedup_merges_exact_content_and_keeps_existing_id() {
        let (store, _dir) = fresh_store().await;

        let mut existing = make_fact("original phrasing", MemoryType::Fact);
        existing.vector = Some(unit_vec_at(0));
        existing.contexts = vec!["code/linggen".into()];
        existing.tags = vec!["topic:setup".into()];
        let existing_id = existing.id.clone();
        store.insert(&[existing.clone()]).await.unwrap();

        // Candidate: byte-identical content (same type), new context + tag.
        let mut candidate = make_fact("original phrasing", MemoryType::Fact);
        candidate.vector = Some(unit_vec_at(0));
        candidate.contexts = vec!["code/linggen".into(), "team/core".into()];
        candidate.tags = vec!["topic:setup".into(), "intent:learn".into()];

        let outcome = store.insert_with_dedup(candidate).await.unwrap();
        let (merged, similarity, previous_id) = match outcome {
            InsertOutcome::Merged {
                fact,
                similarity,
                previous_id,
            } => (fact, similarity, previous_id),
            InsertOutcome::Added(_) => panic!("exact-content match should merge"),
        };

        assert_eq!(previous_id, existing_id);
        assert_eq!(merged.id, existing_id);
        assert_eq!(similarity, 1.0, "exact-content match reports similarity 1.0");
        assert_eq!(merged.content, "original phrasing");
        assert_eq!(
            merged.contexts,
            vec!["code/linggen".to_string(), "team/core".to_string()]
        );
        assert_eq!(
            merged.tags,
            vec!["topic:setup".to_string(), "intent:learn".to_string()]
        );
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn dedup_does_not_merge_contradiction_despite_high_cosine() {
        let (store, _dir) = fresh_store().await;

        let mut existing = make_fact("the cat is male", MemoryType::Fact);
        existing.vector = Some(unit_vec_at(0));
        store.insert(&[existing]).await.unwrap();

        // Same vector index (cosine 1.0) but CONTRADICTORY content. The old
        // cosine gate would have silently merged these and corrupted the
        // fact. Exact-content dedup must keep both rows.
        let mut candidate = make_fact("the cat is female", MemoryType::Fact);
        candidate.vector = Some(unit_vec_at(0));

        let outcome = store.insert_with_dedup(candidate.clone()).await.unwrap();
        match outcome {
            InsertOutcome::Added(got) => assert_eq!(got.id, candidate.id),
            InsertOutcome::Merged { .. } => {
                panic!("a contradiction must NOT merge, regardless of cosine")
            }
        }
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn dedup_does_not_merge_across_types() {
        let (store, _dir) = fresh_store().await;

        let mut existing = make_fact("shared meaning", MemoryType::Fact);
        existing.vector = Some(unit_vec_at(0));
        store.insert(&[existing]).await.unwrap();

        // Same content, different type — must insert, not merge
        // (dedup is type-scoped).
        let mut candidate = make_fact("shared meaning", MemoryType::Preference);
        candidate.vector = Some(unit_vec_at(0));

        let outcome = store.insert_with_dedup(candidate).await.unwrap();
        assert!(matches!(outcome, InsertOutcome::Added(_)));
        assert_eq!(store.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn dedup_without_vector_merges_into_existing_row() {
        let (store, _dir) = fresh_store().await;

        let mut existing = make_fact("anything", MemoryType::Fact);
        existing.vector = Some(unit_vec_at(0));
        let existing_id = existing.id.clone();
        store.insert(&[existing]).await.unwrap();

        // A no-vector candidate dedups like any other (the exact-content
        // lookup is pure SQL) — and the merge keeps the existing row's
        // vector, since equal-length content never replaces it.
        let candidate = make_fact("anything", MemoryType::Fact);
        let outcome = store.insert_with_dedup(candidate).await.unwrap();
        let InsertOutcome::Merged { fact, .. } = outcome else {
            panic!("expected Merged, got {outcome:?}");
        };
        assert_eq!(fact.id, existing_id);
        assert!(fact.vector.is_some());
        assert_eq!(store.count().await.unwrap(), 1);
    }

}
