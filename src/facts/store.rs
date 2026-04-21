//! [`FactsStore`] — LanceDB wrapper around the `facts` table.
//!
//! The store is opened once per `ling-mem` invocation (CLI is one-shot in
//! v0.1). It owns a LanceDB [`Connection`] and a [`Table`] handle for the
//! `facts` table; the table is auto-created on first open if missing.
//!
//! See `doc/tech-spec.md` for the full CLI contract. This module implements
//! just the storage primitives: `open`, `insert`, `get`. Search, list, and
//! mutation ops land in subsequent commits.

use super::schema::{build_schema, facts_to_record_batch, record_batch_to_facts, TABLE_NAME};
use super::types::{Fact, FactType, Origin, Outcome};
use anyhow::{anyhow, Context, Result};
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lancedb::{
    connect,
    query::{ExecutableQuery, QueryBase},
    Connection, Table,
};
use std::path::Path;
use uuid::Uuid;

/// Filter criteria shared by [`FactsStore::search`] and [`FactsStore::list`].
///
/// All filter fields combine with AND. Within `contexts`, every entry must
/// appear in the fact's `contexts` array (AND semantics). Within `types`,
/// any one entry matches (OR). An empty filter matches every row.
#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub contexts: Vec<String>,
    pub types: Vec<FactType>,
    pub origin: Option<Origin>,
    pub outcome: Option<Outcome>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
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

        if let Some(o) = self.origin {
            // `from` is a SQL keyword — double-quote the column name.
            clauses.push(format!("\"from\" = '{}'", o.as_str()));
        }

        if let Some(o) = self.outcome {
            clauses.push(format!("outcome = '{}'", o.as_str()));
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

        if clauses.is_empty() {
            None
        } else {
            Some(clauses.join(" AND "))
        }
    }
}

/// Sort order for [`FactsStore::list`]. Search is ordered by similarity and
/// doesn't use this.
#[derive(Debug, Clone, Copy, Default)]
pub enum SortOrder {
    /// Newest first (by `effective_timestamp`).
    #[default]
    Newest,
    /// Oldest first.
    Oldest,
}

/// Per-field changes for [`FactsStore::update`].
///
/// Each field carries **three** states: `None` means "leave unchanged";
/// `Some(value)` applies the value. For nullable schema fields (`outcome`,
/// `cwd`, `occurred_at`, `source_session`, `vector`) the inner value is
/// itself an `Option` — `Some(Some(x))` sets, `Some(None)` clears to null.
#[derive(Debug, Clone, Default)]
pub struct FactPatch {
    pub content: Option<String>,
    pub contexts: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub r#type: Option<FactType>,
    pub origin: Option<Origin>,
    pub outcome: Option<Option<Outcome>>,
    pub cwd: Option<Option<String>>,
    pub occurred_at: Option<Option<DateTime<Utc>>>,
    pub source_session: Option<Option<String>>,
    pub vector: Option<Option<Vec<f32>>>,
}

impl FactPatch {
    /// Apply this patch in place to a fact. Used internally by
    /// [`FactsStore::update`]; exposed for tests.
    pub fn apply(&self, f: &mut Fact) {
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
        if let Some(v) = &self.vector {
            f.vector = v.clone();
        }
    }
}

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

/// The LanceDB-backed memory store.
pub struct FactsStore {
    _conn: Connection,
    table: Table,
}

impl FactsStore {
    /// Open (or auto-create) the facts store rooted at `data_dir`.
    ///
    /// The actual LanceDB directory is `data_dir/memory/facts.lancedb/` —
    /// appending `memory/facts.lancedb/` to the Linggen-per-user data dir.
    /// If the dir doesn't exist, both the directories and an empty `facts`
    /// table are created.
    pub async fn open(data_dir: &Path) -> Result<Self> {
        let lancedb_dir = data_dir.join("memory").join("facts.lancedb");
        tokio::fs::create_dir_all(&lancedb_dir)
            .await
            .with_context(|| {
                format!("creating memory dir at {}", lancedb_dir.display())
            })?;

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

        let table = if names.iter().any(|n| n == TABLE_NAME) {
            conn.open_table(TABLE_NAME)
                .execute()
                .await
                .with_context(|| format!("opening `{TABLE_NAME}` table"))?
        } else {
            let schema = build_schema();
            // lancedb 0.27 needs a boxed RecordBatchReader (Scannable is
            // implemented for that shape, not for bare iterators).
            let empty: Box<dyn RecordBatchReader + Send> = Box::new(
                RecordBatchIterator::new(
                    std::iter::empty::<arrow::error::Result<RecordBatch>>(),
                    schema,
                ),
            );
            conn.create_table(TABLE_NAME, empty)
                .execute()
                .await
                .with_context(|| format!("creating `{TABLE_NAME}` table"))?
        };

        Ok(Self { _conn: conn, table })
    }

    /// Insert one or more facts. Returns the number of rows written.
    ///
    /// Treats the store as append-only — existing IDs are not detected or
    /// deduplicated here. Callers who need upsert semantics should use
    /// `update()` (next commit) or check with `get()` first.
    pub async fn insert(&self, facts: &[Fact]) -> Result<usize> {
        if facts.is_empty() {
            return Ok(0);
        }

        let batch = facts_to_record_batch(facts)?;
        let schema = batch.schema();
        let batches: Box<dyn RecordBatchReader + Send> = Box::new(
            RecordBatchIterator::new(std::iter::once(Ok(batch)), schema),
        );

        self.table
            .add(batches)
            .execute()
            .await
            .context("adding batch to facts table")?;

        Ok(facts.len())
    }

    /// Fetch one fact by id. Returns `None` if the id is not present.
    pub async fn get(&self, id: Uuid) -> Result<Option<Fact>> {
        let id_str = id.to_string();
        // LanceDB SQL filter — id is Utf8, so we compare against a literal.
        let filter = format!("id = '{id_str}'");

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
            let facts = record_batch_to_facts(&batch)?;
            if let Some(fact) = facts.into_iter().next() {
                return Ok(Some(fact));
            }
        }
        Ok(None)
    }

    /// Total row count in the facts table. Useful for tests and dashboards.
    pub async fn count(&self) -> Result<usize> {
        self.table
            .count_rows(None)
            .await
            .context("counting rows in facts table")
    }

    /// Nearest-neighbor search over `vector`, constrained by `filters`.
    ///
    /// Returns up to `limit` facts sorted by vector similarity (closest
    /// first). Rows with null vectors are never returned — they wouldn't
    /// have a similarity score.
    pub async fn search(
        &self,
        query_vec: &[f32],
        filters: &Filters,
        limit: usize,
    ) -> Result<Vec<Fact>> {
        if query_vec.len() != super::schema::VECTOR_DIM as usize {
            return Err(anyhow!(
                "query vector has len {} but schema dim is {}",
                query_vec.len(),
                super::schema::VECTOR_DIM
            ));
        }

        let mut q = self
            .table
            .vector_search(query_vec.to_vec())
            .context("starting vector search")?
            .limit(limit);

        if let Some(sql) = filters.to_sql() {
            q = q.only_if(sql);
        }

        self.collect_query(q).await
    }

    /// Non-semantic browse. Returns up to `limit` facts matching `filters`,
    /// sorted by `effective_timestamp` according to `order`.
    ///
    /// Sorting is done in-process after the batch returns — LanceDB's query
    /// builder doesn't expose an ORDER BY equivalent for list-style queries
    /// without a vector. For v0.1 row counts this is fine; revisit if files
    /// grow past ~100k rows.
    pub async fn list(
        &self,
        filters: &Filters,
        order: SortOrder,
        limit: usize,
    ) -> Result<Vec<Fact>> {
        // Fetch up to `limit` rows matching the filter. We pull more than
        // `limit` only when LanceDB might return unsorted rows and we need
        // to top-N-sort; for v0.1 we trust the filter to narrow enough.
        let mut q = self.table.query().limit(limit);
        if let Some(sql) = filters.to_sql() {
            q = q.only_if(sql);
        }

        let mut facts = self.collect_query(q).await?;

        facts.sort_by(|a, b| match order {
            SortOrder::Newest => b.effective_timestamp().cmp(&a.effective_timestamp()),
            SortOrder::Oldest => a.effective_timestamp().cmp(&b.effective_timestamp()),
        });
        Ok(facts)
    }

    /// Update a single fact by id. Applies `patch` on top of the existing
    /// row, then writes the result back. Returns the post-patch fact, or
    /// `None` if the id doesn't exist.
    ///
    /// v0.1 uses read-modify-write (delete + insert). LanceDB supports
    /// column-level updates for simple types, but our nested schema (lists,
    /// FixedSizeList vectors, timezone-tagged timestamps) is cleaner to
    /// round-trip through [`Fact`] than to express as SQL column setters.
    pub async fn update(&self, id: Uuid, patch: &FactPatch) -> Result<Option<Fact>> {
        let Some(mut existing) = self.get(id).await? else {
            return Ok(None);
        };
        patch.apply(&mut existing);
        self.delete_one(id).await?;
        self.insert(std::slice::from_ref(&existing)).await?;
        Ok(Some(existing))
    }

    /// Hard-delete a single fact by id. Returns `true` if a row was
    /// removed, `false` if the id wasn't present.
    pub async fn delete(&self, id: Uuid) -> Result<bool> {
        let removed = self.delete_one(id).await?;
        Ok(removed > 0)
    }

    /// Bulk-delete by filter. Returns the number of rows removed.
    ///
    /// Safety: refuses to run with an empty filter — use `Filters::default()`
    /// would match every row, which is almost always a bug. Callers who
    /// genuinely want "forget everything" should iterate over all rows and
    /// delete by id, or drop the table directory externally.
    pub async fn forget(&self, filters: &Filters) -> Result<usize> {
        let Some(predicate) = filters.to_sql() else {
            return Err(anyhow!(
                "refusing to forget with empty filter — provide at least one criterion"
            ));
        };

        let before = self.count().await?;
        self.table
            .delete(&predicate)
            .await
            .context("deleting rows by filter")?;
        let after = self.count().await?;
        Ok(before.saturating_sub(after))
    }

    /// Delete the single row with the given id. Returns the number removed
    /// (0 or 1 in practice). Private — public API uses `delete` / `update`.
    async fn delete_one(&self, id: Uuid) -> Result<usize> {
        let before = self.count().await?;
        self.table
            .delete(&format!("id = '{}'", id))
            .await
            .with_context(|| format!("deleting fact {id}"))?;
        let after = self.count().await?;
        Ok(before.saturating_sub(after))
    }

    async fn collect_query<Q: ExecutableQuery>(&self, q: Q) -> Result<Vec<Fact>> {
        let mut stream = q.execute().await.context("executing facts query")?;
        let mut out: Vec<Fact> = Vec::new();
        while let Some(batch) = stream
            .try_next()
            .await
            .context("reading next batch from facts query")?
        {
            if batch.num_rows() == 0 {
                continue;
            }
            out.extend(record_batch_to_facts(&batch)?);
        }
        Ok(out)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::types::{FactType, Origin, Outcome};
    use tempfile::TempDir;

    async fn fresh_store() -> (FactsStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = FactsStore::open(dir.path()).await.unwrap();
        (store, dir)
    }

    fn make_fact(content: &str, t: FactType) -> Fact {
        Fact::new(content, t, Origin::User)
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
            let store = FactsStore::open(dir.path()).await.unwrap();
            store
                .insert(&[make_fact("first", FactType::Fact)])
                .await
                .unwrap();
        }
        // Re-opening finds the table and sees the existing row.
        let store = FactsStore::open(dir.path()).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn insert_returns_row_count() {
        let (store, _dir) = fresh_store().await;
        let facts = vec![
            make_fact("a", FactType::Fact),
            make_fact("b", FactType::Preference),
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
        let mut f = make_fact("find me", FactType::Learned);
        f.contexts = vec!["env/macos".into()];
        f.tags = vec!["topic:dates".into()];
        f.outcome = Some(Outcome::Positive);
        let id = f.id;
        store.insert(&[f.clone()]).await.unwrap();

        let got = store.get(id).await.unwrap().expect("fact missing");
        assert_eq!(got, f);
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_id() {
        let (store, _dir) = fresh_store().await;
        store
            .insert(&[make_fact("x", FactType::Fact)])
            .await
            .unwrap();
        let missing = Uuid::new_v4();
        assert!(store.get(missing).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn insert_many_then_count() {
        let (store, _dir) = fresh_store().await;
        let facts: Vec<Fact> = (0..50)
            .map(|i| make_fact(&format!("f{i}"), FactType::Fact))
            .collect();
        store.insert(&facts).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 50);
    }

    // ── search + list tests ────────────────────────────────────────────────

    use super::super::schema::VECTOR_DIM;
    use chrono::Duration;

    fn with_vector(mut f: Fact, value: f32) -> Fact {
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
            types: vec![FactType::Fixed],
            origin: Some(Origin::User),
            outcome: Some(Outcome::Positive),
            since: None,
            until: None,
        };
        let sql = f.to_sql().unwrap();
        assert!(sql.contains("array_has(contexts, 'code/linggen')"));
        assert!(sql.contains("type = 'fixed'"));
        assert!(sql.contains("\"from\" = 'user'"));
        assert!(sql.contains("outcome = 'positive'"));
        assert!(sql.contains(" AND "));
    }

    #[tokio::test]
    async fn list_with_empty_filters_returns_all_newest_first() {
        let (store, _dir) = fresh_store().await;

        let mut old = make_fact("old", FactType::Fact);
        old.occurred_at = Some(Utc::now() - Duration::days(3));
        let mut mid = make_fact("mid", FactType::Fact);
        mid.occurred_at = Some(Utc::now() - Duration::days(1));
        let new = make_fact("new", FactType::Fact); // no occurred_at, uses created_at ≈ now

        store.insert(&[old, mid, new]).await.unwrap();

        let results = store
            .list(&Filters::default(), SortOrder::Newest, 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].content, "new");
        assert_eq!(results[1].content, "mid");
        assert_eq!(results[2].content, "old");

        let oldest_first = store
            .list(&Filters::default(), SortOrder::Oldest, 10)
            .await
            .unwrap();
        assert_eq!(oldest_first[0].content, "old");
        assert_eq!(oldest_first[2].content, "new");
    }

    #[tokio::test]
    async fn list_filters_by_type_and_context() {
        let (store, _dir) = fresh_store().await;

        let mut a = make_fact("a", FactType::Preference);
        a.contexts = vec!["global".into()];
        let mut b = make_fact("b", FactType::Fixed);
        b.contexts = vec!["code/linggen".into()];
        let mut c = make_fact("c", FactType::Fixed);
        c.contexts = vec!["code/sanji".into()];

        store.insert(&[a, b, c]).await.unwrap();

        let only_fixed_in_linggen = store
            .list(
                &Filters {
                    contexts: vec!["code/linggen".into()],
                    types: vec![FactType::Fixed],
                    ..Default::default()
                },
                SortOrder::Newest,
                10,
            )
            .await
            .unwrap();

        assert_eq!(only_fixed_in_linggen.len(), 1);
        assert_eq!(only_fixed_in_linggen[0].content, "b");
    }

    #[tokio::test]
    async fn list_types_are_or_combined() {
        let (store, _dir) = fresh_store().await;
        let a = make_fact("pref", FactType::Preference);
        let b = make_fact("fix", FactType::Fixed);
        let c = make_fact("tried", FactType::Tried);
        store.insert(&[a, b, c]).await.unwrap();

        let prefs_or_fixes = store
            .list(
                &Filters {
                    types: vec![FactType::Preference, FactType::Fixed],
                    ..Default::default()
                },
                SortOrder::Newest,
                10,
            )
            .await
            .unwrap();
        assert_eq!(prefs_or_fixes.len(), 2);
    }

    #[tokio::test]
    async fn list_filters_by_time_range() {
        let (store, _dir) = fresh_store().await;
        let now = Utc::now();

        let mut old = make_fact("old", FactType::Fact);
        old.occurred_at = Some(now - Duration::days(10));
        let mut recent = make_fact("recent", FactType::Fact);
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
            )
            .await
            .unwrap();
        assert_eq!(since_yesterday.len(), 1);
        assert_eq!(since_yesterday[0].content, "recent");
    }

    #[tokio::test]
    async fn search_returns_nearest_first() {
        let (store, _dir) = fresh_store().await;

        // Three facts, each with a distinct unit-ish vector.
        let a = with_vector(make_fact("a", FactType::Fact), 0.1);
        let b = with_vector(make_fact("b", FactType::Fact), 0.5);
        let c = with_vector(make_fact("c", FactType::Fact), 0.9);
        store.insert(&[a.clone(), b.clone(), c.clone()]).await.unwrap();

        // Query close to `a`'s vector.
        let query = vec![0.11; VECTOR_DIM as usize];
        let results = store
            .search(&query, &Filters::default(), 3)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].content, "a");
    }

    #[tokio::test]
    async fn search_respects_limit() {
        let (store, _dir) = fresh_store().await;
        let facts: Vec<Fact> = (0..10)
            .map(|i| with_vector(make_fact(&format!("f{i}"), FactType::Fact), i as f32 * 0.1))
            .collect();
        store.insert(&facts).await.unwrap();

        let query = vec![0.0; VECTOR_DIM as usize];
        let results = store
            .search(&query, &Filters::default(), 3)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn search_with_context_filter() {
        let (store, _dir) = fresh_store().await;
        let mut in_ctx = with_vector(make_fact("in-ctx", FactType::Fact), 0.3);
        in_ctx.contexts = vec!["music/piano".into()];
        let mut other = with_vector(make_fact("other", FactType::Fact), 0.3);
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
        assert!(err.to_string().contains("dim is 384"));
    }

    // ── update + delete + forget tests ─────────────────────────────────────

    #[tokio::test]
    async fn delete_removes_row() {
        let (store, _dir) = fresh_store().await;
        let f = make_fact("x", FactType::Fact);
        let id = f.id;
        store.insert(&[f]).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        assert!(store.delete(id).await.unwrap());
        assert_eq!(store.count().await.unwrap(), 0);

        // Second delete is a no-op but not an error.
        assert!(!store.delete(id).await.unwrap());
    }

    #[tokio::test]
    async fn update_applies_patch() {
        let (store, _dir) = fresh_store().await;
        let mut f = make_fact("original", FactType::Fact);
        f.contexts = vec!["a".into()];
        let id = f.id;
        store.insert(&[f]).await.unwrap();

        let patch = FactPatch {
            content: Some("edited".into()),
            contexts: Some(vec!["b".into(), "c".into()]),
            outcome: Some(Some(Outcome::Positive)),
            ..Default::default()
        };

        let updated = store.update(id, &patch).await.unwrap().unwrap();
        assert_eq!(updated.content, "edited");
        assert_eq!(updated.contexts, vec!["b".to_string(), "c".into()]);
        assert_eq!(updated.outcome, Some(Outcome::Positive));

        // Persisted.
        let got = store.get(id).await.unwrap().unwrap();
        assert_eq!(got, updated);
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn update_can_clear_optional_field() {
        let (store, _dir) = fresh_store().await;
        let mut f = make_fact("x", FactType::Tried);
        f.outcome = Some(Outcome::Negative);
        let id = f.id;
        store.insert(&[f]).await.unwrap();

        // Some(None) clears to null.
        let patch = FactPatch {
            outcome: Some(None),
            ..Default::default()
        };
        let updated = store.update(id, &patch).await.unwrap().unwrap();
        assert_eq!(updated.outcome, None);
    }

    #[tokio::test]
    async fn update_missing_id_returns_none() {
        let (store, _dir) = fresh_store().await;
        let result = store
            .update(Uuid::new_v4(), &FactPatch::default())
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn forget_bulk_deletes_matching() {
        let (store, _dir) = fresh_store().await;
        let mut sanji1 = make_fact("s1", FactType::Fact);
        sanji1.contexts = vec!["code/sanji".into()];
        let mut sanji2 = make_fact("s2", FactType::Fact);
        sanji2.contexts = vec!["code/sanji".into()];
        let mut ling = make_fact("l1", FactType::Fact);
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
            .insert(&[make_fact("x", FactType::Fact)])
            .await
            .unwrap();

        let err = store
            .forget(&Filters::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("empty filter"));
        assert_eq!(store.count().await.unwrap(), 1);
    }
}
