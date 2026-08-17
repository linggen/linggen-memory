//! Arrow + LanceDB schema for the memory tables.
//!
//! Defines:
//! - [`SEMANTIC_TABLE_NAME`] — the curated long-term (semantic) table.
//! - [`VECTOR_DIM`] — embedding dimension (1024, matches `Qwen3-Embedding-0.6B`).
//! - [`build_schema`] — the 14-field Arrow schema matching `doc/tech-spec.md`.
//! - [`memories_to_record_batch`] — encode `&[Memory]` as a single `RecordBatch`.
//! - [`record_batch_to_memories`] — decode a `RecordBatch` back into `Vec<Memory>`.
//!
//! Changes to any field (rename, type change, nullability flip) must be made
//! in lockstep here and in `types.rs`.

use super::types::{Memory, MemoryType, Origin, Outcome, Tier};
use anyhow::{anyhow, Context, Result};
use arrow_array::{
    builder::{FixedSizeListBuilder, Float32Builder, ListBuilder, StringBuilder},
    Array, FixedSizeListArray, ListArray, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, TimeZone, Utc};
use std::str::FromStr;
use std::sync::Arc;

/// Name of the LanceDB table holding curated long-term memory (semantic
/// memory, in brain terms). Both `tier=core` and `tier=semantic` rows
/// live here.
pub const SEMANTIC_TABLE_NAME: &str = "semantic";

/// Name of the LanceDB table holding staged short-term experience
/// (episodic memory) awaiting consolidation. Same row schema as the
/// semantic table (reuses [`build_schema`]); a separate table so its ANN
/// index stays isolated from the curated semantic index.
pub const EPISODIC_TABLE_NAME: &str = "episodic";

/// Embedding vector dimensionality. Locked by the default model
/// (`Qwen/Qwen3-Embedding-0.6B`, 1024-dim).
pub const VECTOR_DIM: i32 = 1024;

const TZ_UTC: &str = "UTC";

/// Build the Arrow schema for a memory table.
pub fn build_schema() -> Arc<Schema> {
    let utf8_item = Arc::new(Field::new("item", DataType::Utf8, true));
    let float_item = Arc::new(Field::new("item", DataType::Float32, true));

    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(float_item, VECTOR_DIM),
            true,
        ),
        Field::new("contexts", DataType::List(utf8_item.clone()), false),
        Field::new("tags", DataType::List(utf8_item), false),
        Field::new("type", DataType::Utf8, false),
        Field::new("outcome", DataType::Utf8, true),
        Field::new("from", DataType::Utf8, false),
        Field::new("tier", DataType::Utf8, false),
        Field::new("cwd", DataType::Utf8, true),
        Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(TZ_UTC.into())),
            false,
        ),
        Field::new(
            "updated_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(TZ_UTC.into())),
            true,
        ),
        Field::new(
            "occurred_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(TZ_UTC.into())),
            true,
        ),
        Field::new("source_session", DataType::Utf8, true),
        Field::new("host", DataType::Utf8, true),
        // Archive pair (2026-08-17): a merge/digest EXPIRES its losers
        // instead of deleting them. NULL expired_at = live row.
        Field::new(
            "expired_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(TZ_UTC.into())),
            true,
        ),
        Field::new("superseded_by", DataType::Utf8, true),
    ]))
}

/// Encode a slice of facts as a single Arrow `RecordBatch` matching
/// [`build_schema`]. Preserves row order; `None`-valued optional fields
/// become nulls in the corresponding columns.
pub fn memories_to_record_batch(facts: &[Memory]) -> Result<RecordBatch> {
    let schema = build_schema();

    let ids = StringArray::from_iter_values(facts.iter().map(|f| f.id.as_str()));
    let contents = StringArray::from_iter_values(facts.iter().map(|f| f.content.clone()));
    let types = StringArray::from_iter_values(facts.iter().map(|f| f.r#type.as_str()));
    let froms = StringArray::from_iter_values(facts.iter().map(|f| f.origin.as_str()));
    let tiers = StringArray::from_iter_values(facts.iter().map(|f| f.tier.as_str()));

    let outcomes = StringArray::from_iter(facts.iter().map(|f| f.outcome.map(|o| o.as_str())));
    let cwds = StringArray::from_iter(facts.iter().map(|f| f.cwd.clone()));
    let source_sessions = StringArray::from_iter(facts.iter().map(|f| f.source_session.clone()));
    let hosts = StringArray::from_iter(facts.iter().map(|f| f.host.clone()));

    let vectors = build_vector_column(facts)?;
    let contexts = build_string_list_column(facts.iter().map(|f| &f.contexts));
    let tags = build_string_list_column(facts.iter().map(|f| &f.tags));

    let created_at = TimestampMicrosecondArray::from_iter_values(
        facts.iter().map(|f| f.created_at.timestamp_micros()),
    )
    .with_timezone(TZ_UTC);

    let updated_at = TimestampMicrosecondArray::from_iter(
        facts
            .iter()
            .map(|f| f.updated_at.map(|t| t.timestamp_micros())),
    )
    .with_timezone(TZ_UTC);

    let occurred_at = TimestampMicrosecondArray::from_iter(
        facts
            .iter()
            .map(|f| f.occurred_at.map(|t| t.timestamp_micros())),
    )
    .with_timezone(TZ_UTC);

    let expired_at = TimestampMicrosecondArray::from_iter(
        facts
            .iter()
            .map(|f| f.expired_at.map(|t| t.timestamp_micros())),
    )
    .with_timezone(TZ_UTC);
    let superseded_bys = StringArray::from_iter(facts.iter().map(|f| f.superseded_by.clone()));

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(ids),
            Arc::new(contents),
            Arc::new(vectors),
            Arc::new(contexts),
            Arc::new(tags),
            Arc::new(types),
            Arc::new(outcomes),
            Arc::new(froms),
            Arc::new(tiers),
            Arc::new(cwds),
            Arc::new(created_at),
            Arc::new(updated_at),
            Arc::new(occurred_at),
            Arc::new(source_sessions),
            Arc::new(hosts),
            Arc::new(expired_at),
            Arc::new(superseded_bys),
        ],
    )
    .context("building facts RecordBatch")
}

/// Decode a `RecordBatch` produced against [`build_schema`] back into facts.
/// Returns an error if any required column is missing or has the wrong type.
pub fn record_batch_to_memories(batch: &RecordBatch) -> Result<Vec<Memory>> {
    let n = batch.num_rows();
    let ids = col_utf8(batch, "id")?;
    let contents = col_utf8(batch, "content")?;
    let types = col_utf8(batch, "type")?;
    let froms = col_utf8(batch, "from")?;
    let tiers = col_utf8(batch, "tier")?;
    let outcomes = col_utf8_opt(batch, "outcome")?;
    let cwds = col_utf8_opt(batch, "cwd")?;
    let source_sessions = col_utf8_opt(batch, "source_session")?;
    // `host` was added after the initial schema. Older tables on disk
    // may not carry the column yet — treat its absence as "all rows
    // have host=null" rather than failing the whole batch decode.
    let hosts = col_utf8_opt_missing_ok(batch, "host");
    let contexts = col_string_list(batch, "contexts")?;
    let tags = col_string_list(batch, "tags")?;
    let vectors = col_vector(batch, "vector")?;
    let created_at = col_timestamp(batch, "created_at")?;
    let updated_at = col_timestamp_opt(batch, "updated_at")?;
    let occurred_at = col_timestamp_opt(batch, "occurred_at")?;
    // Archive pair — added 2026-08-17; absent on older on-disk tables
    // until `ensure_late_schema_additions` runs, so decode is lenient.
    let expired_at = col_timestamp_opt_missing_ok(batch, "expired_at");
    let superseded_bys = col_utf8_opt_missing_ok(batch, "superseded_by");

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let id = ids.value(i).to_string();
        let type_str = types.value(i);
        let r#type = MemoryType::from_str(type_str)
            .with_context(|| format!("unknown type `{type_str}` at row {i}"))?;
        let from_str = froms.value(i);
        let origin = Origin::from_str(from_str)
            .with_context(|| format!("unknown from `{from_str}` at row {i}"))?;
        let tier_str = tiers.value(i);
        let tier = Tier::from_str(tier_str)
            .with_context(|| format!("unknown tier `{tier_str}` at row {i}"))?;
        let outcome = match outcomes.get(i).copied().flatten() {
            Some(s) => Some(
                Outcome::from_str(s)
                    .with_context(|| format!("unknown outcome `{s}` at row {i}"))?,
            ),
            None => None,
        };

        out.push(Memory {
            id,
            content: contents.value(i).to_string(),
            vector: vectors.get(i).cloned().flatten(),
            contexts: contexts[i].clone(),
            tags: tags[i].clone(),
            r#type,
            tier,
            outcome,
            origin,
            cwd: cwds.get(i).copied().flatten().map(str::to_string),
            created_at: created_at[i],
            updated_at: updated_at.get(i).copied().flatten(),
            occurred_at: occurred_at.get(i).copied().flatten(),
            source_session: source_sessions
                .get(i)
                .copied()
                .flatten()
                .map(str::to_string),
            host: hosts.get(i).cloned().flatten(),
            expired_at: expired_at.get(i).copied().flatten(),
            superseded_by: superseded_bys.get(i).cloned().flatten(),
        });
    }
    Ok(out)
}

// ── column builders ─────────────────────────────────────────────────────────

fn build_string_list_column<'a, I>(iter: I) -> ListArray
where
    I: IntoIterator<Item = &'a Vec<String>>,
{
    let mut builder = ListBuilder::new(StringBuilder::new());
    for row in iter {
        for s in row {
            builder.values().append_value(s);
        }
        builder.append(true);
    }
    builder.finish()
}

fn build_vector_column(facts: &[Memory]) -> Result<FixedSizeListArray> {
    let mut builder = FixedSizeListBuilder::new(Float32Builder::new(), VECTOR_DIM);
    for fact in facts {
        match &fact.vector {
            Some(v) => {
                if v.len() != VECTOR_DIM as usize {
                    return Err(anyhow!(
                        "fact {} has vector len {} but schema expects {}",
                        fact.id,
                        v.len(),
                        VECTOR_DIM
                    ));
                }
                for x in v {
                    builder.values().append_value(*x);
                }
                builder.append(true);
            }
            None => {
                for _ in 0..VECTOR_DIM {
                    builder.values().append_null();
                }
                builder.append(false);
            }
        }
    }
    Ok(builder.finish())
}

// ── column extractors ───────────────────────────────────────────────────────

fn col_utf8<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing column `{name}`"))?
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("column `{name}` is not Utf8"))
}

/// Like [`col_utf8_opt`] but returns all-None when the column is absent
/// entirely — used for fields added after the initial schema (e.g.
/// `host`) so legacy on-disk tables still decode cleanly.
fn col_utf8_opt_missing_ok(batch: &RecordBatch, name: &str) -> Vec<Option<String>> {
    let Some(col) = batch.column_by_name(name) else {
        return vec![None; batch.num_rows()];
    };
    let Some(arr) = col.as_any().downcast_ref::<StringArray>() else {
        return vec![None; batch.num_rows()];
    };
    (0..arr.len())
        .map(|i| {
            if arr.is_null(i) {
                None
            } else {
                Some(arr.value(i).to_string())
            }
        })
        .collect()
}

/// Like [`col_timestamp_opt`] but returns all-None when the column is
/// absent entirely — same legacy-table leniency as
/// [`col_utf8_opt_missing_ok`], for timestamp fields added after the
/// initial schema (e.g. `expired_at`).
fn col_timestamp_opt_missing_ok(batch: &RecordBatch, name: &str) -> Vec<Option<DateTime<Utc>>> {
    if batch.column_by_name(name).is_none() {
        return vec![None; batch.num_rows()];
    }
    col_timestamp_opt(batch, name).unwrap_or_else(|_| vec![None; batch.num_rows()])
}

/// Utf8 column as a Vec mapping row → Option<&str>. Cheap: borrows from the
/// underlying array, one Option per row.
fn col_utf8_opt<'a>(batch: &'a RecordBatch, name: &str) -> Result<Vec<Option<&'a str>>> {
    let arr = col_utf8(batch, name)?;
    Ok((0..arr.len())
        .map(|i| {
            if arr.is_null(i) {
                None
            } else {
                Some(arr.value(i))
            }
        })
        .collect())
}

fn col_string_list(batch: &RecordBatch, name: &str) -> Result<Vec<Vec<String>>> {
    let arr = batch
        .column_by_name(name)
        .with_context(|| format!("missing column `{name}`"))?
        .as_any()
        .downcast_ref::<ListArray>()
        .with_context(|| format!("column `{name}` is not List<Utf8>"))?;

    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        let row = arr.value(i);
        let strs = row
            .as_any()
            .downcast_ref::<StringArray>()
            .context("list item is not Utf8")?;
        out.push(
            (0..strs.len())
                .filter(|&j| !strs.is_null(j))
                .map(|j| strs.value(j).to_string())
                .collect(),
        );
    }
    Ok(out)
}

fn col_vector(batch: &RecordBatch, name: &str) -> Result<Vec<Option<Vec<f32>>>> {
    let arr = batch
        .column_by_name(name)
        .with_context(|| format!("missing column `{name}`"))?
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .with_context(|| format!("column `{name}` is not FixedSizeList<Float32>"))?;

    let mut out = Vec::with_capacity(arr.len());
    for i in 0..arr.len() {
        if arr.is_null(i) {
            out.push(None);
            continue;
        }
        let row = arr.value(i);
        let floats = row
            .as_any()
            .downcast_ref::<arrow_array::Float32Array>()
            .context("vector row is not Float32")?;
        let vec: Vec<f32> = (0..floats.len())
            .map(|j| {
                if floats.is_null(j) {
                    0.0
                } else {
                    floats.value(j)
                }
            })
            .collect();
        out.push(Some(vec));
    }
    Ok(out)
}

fn col_timestamp(batch: &RecordBatch, name: &str) -> Result<Vec<DateTime<Utc>>> {
    let arr = batch
        .column_by_name(name)
        .with_context(|| format!("missing column `{name}`"))?
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .with_context(|| format!("column `{name}` is not Timestamp(us)"))?;

    (0..arr.len())
        .map(|i| {
            Utc.timestamp_micros(arr.value(i))
                .single()
                .with_context(|| format!("row {i} of `{name}` is an ambiguous/invalid timestamp"))
        })
        .collect()
}

fn col_timestamp_opt(batch: &RecordBatch, name: &str) -> Result<Vec<Option<DateTime<Utc>>>> {
    let arr = batch
        .column_by_name(name)
        .with_context(|| format!("missing column `{name}`"))?
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .with_context(|| format!("column `{name}` is not Timestamp(us)"))?;

    (0..arr.len())
        .map(|i| {
            if arr.is_null(i) {
                Ok(None)
            } else {
                Utc.timestamp_micros(arr.value(i))
                    .single()
                    .map(Some)
                    .with_context(|| {
                        format!("row {i} of `{name}` is an ambiguous/invalid timestamp")
                    })
            }
        })
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn sample_facts() -> Vec<Memory> {
        let mut f1 = Memory::new(
            "user prefers concise replies",
            MemoryType::Preference,
            Origin::User,
        );
        f1.contexts = vec!["global".into()];
        f1.tags = vec!["intent:style".into()];

        let mut f2 = Memory::new(
            "webrtc dc closes after 30s idle",
            MemoryType::Fixed,
            Origin::Agent,
        );
        f2.contexts = vec!["code/linggen".into(), "webrtc".into()];
        f2.tags = vec!["topic:networking".into()];
        f2.outcome = Some(Outcome::Positive);
        f2.vector = Some(vec![0.1; VECTOR_DIM as usize]);
        f2.cwd = Some("/home/u/workspace/linggen".into());
        f2.updated_at = Some(f2.created_at + Duration::minutes(20));
        f2.occurred_at = Some(f2.created_at - Duration::hours(3));
        f2.source_session = Some("sess-abc".into());

        // f3 leaves updated_at None — proves the null path round-trips too.
        let f3 = Memory::new("", MemoryType::Learned, Origin::Derived);

        vec![f1, f2, f3]
    }

    #[test]
    fn schema_has_seventeen_fields() {
        let schema = build_schema();
        assert_eq!(schema.fields().len(), 17);
    }

    #[test]
    fn schema_field_names_match_spec() {
        let schema = build_schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            vec![
                "id",
                "content",
                "vector",
                "contexts",
                "tags",
                "type",
                "outcome",
                "from",
                "tier",
                "cwd",
                "created_at",
                "updated_at",
                "occurred_at",
                "source_session",
                "host",
                "expired_at",
                "superseded_by",
            ]
        );
    }

    #[test]
    fn facts_roundtrip_through_record_batch() {
        let facts = sample_facts();
        let batch = memories_to_record_batch(&facts).unwrap();
        assert_eq!(batch.num_rows(), facts.len());

        let decoded = record_batch_to_memories(&batch).unwrap();
        assert_eq!(decoded, facts);
    }

    #[test]
    fn empty_vec_roundtrips() {
        let batch = memories_to_record_batch(&[]).unwrap();
        assert_eq!(batch.num_rows(), 0);
        let decoded = record_batch_to_memories(&batch).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn wrong_vector_dim_is_rejected() {
        let mut f = Memory::new("x", MemoryType::Fact, Origin::Derived);
        f.vector = Some(vec![0.0; 128]);
        let err = memories_to_record_batch(&[f]).unwrap_err();
        assert!(err.to_string().contains(&format!("expects {VECTOR_DIM}")));
    }

    #[test]
    fn nulls_omit_optional_fields() {
        let facts = sample_facts();
        let batch = memories_to_record_batch(&facts).unwrap();

        // f1 and f3 have no outcome; f2 does.
        let outcomes = batch
            .column_by_name("outcome")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert!(outcomes.is_null(0));
        assert!(!outcomes.is_null(1));
        assert!(outcomes.is_null(2));

        // f2 carries updated_at; f1 and f3 leave it null.
        let updated_at = batch
            .column_by_name("updated_at")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        assert!(updated_at.is_null(0));
        assert!(!updated_at.is_null(1));
        assert!(updated_at.is_null(2));

        // f2 has a vector; f1 and f3 don't.
        let vectors = batch
            .column_by_name("vector")
            .unwrap()
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .unwrap();
        assert!(vectors.is_null(0));
        assert!(!vectors.is_null(1));
        assert!(vectors.is_null(2));
    }

    #[test]
    fn list_columns_never_null() {
        // Even empty contexts / tags produce a present-but-empty list,
        // not a null. This keeps list-contains filters simple.
        let f = Memory::new("x", MemoryType::Fact, Origin::Derived);
        let batch = memories_to_record_batch(&[f]).unwrap();
        let contexts = batch
            .column_by_name("contexts")
            .unwrap()
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        assert!(!contexts.is_null(0));
        assert_eq!(contexts.value(0).len(), 0);
    }

    #[test]
    fn record_batch_reports_missing_column() {
        // Build a batch missing the `tags` column to ensure the decoder
        // surfaces a clear error.
        let mut facts = sample_facts();
        facts.truncate(1);
        let batch = memories_to_record_batch(&facts).unwrap();

        // Drop a column by re-projecting.
        let schema = batch.schema();
        let keep: Vec<usize> = (0..schema.fields().len())
            .filter(|&i| schema.field(i).name() != "tags")
            .collect();
        let pruned = batch.project(&keep).unwrap();

        let err = record_batch_to_memories(&pruned).unwrap_err();
        assert!(err.to_string().contains("tags"));
    }
}
