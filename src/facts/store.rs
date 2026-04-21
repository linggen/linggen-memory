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
use super::types::Fact;
use anyhow::{anyhow, Context, Result};
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader};
use futures::TryStreamExt;
use lancedb::{
    connect,
    query::{ExecutableQuery, QueryBase},
    Connection, Table,
};
use std::path::Path;
use uuid::Uuid;

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
}
