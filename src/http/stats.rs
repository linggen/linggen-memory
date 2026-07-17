//! `POST /api/memory/stats` — one-call store-state summary.
//!
//! The single source every surface renders from: the console header,
//! the memory app's dashboard, and the Linggen engine's mission-report
//! footer. Purely mechanical — counts are LanceDB metadata reads, disk
//! usage is a directory walk, the rest is config/constants.

use super::envelope::{ok, ApiError};
use super::state::SharedState;
use axum::extract::State;
use axum::response::Response;
use axum::Router;
use serde_json::json;
use std::path::Path;

use crate::memory::store::Filters;
use crate::memory::types::Tier;

pub fn router() -> Router<SharedState> {
    use axum::routing::post;
    Router::new().route("/api/memory/stats", post(stats))
}

async fn stats(State(state): State<SharedState>) -> Result<Response, ApiError> {
    let core = state
        .store
        .count_filtered(&Filters {
            tier: Some(Tier::Core),
            ..Filters::default()
        })
        .await
        .map_err(ApiError::internal)?;
    let semantic = state
        .store
        .count_filtered(&Filters {
            tier: Some(Tier::Semantic),
            ..Filters::default()
        })
        .await
        .map_err(ApiError::internal)?;
    let episodic = state.episodic.count().await.map_err(ApiError::internal)?;

    // Disk footprint. Both tables live inside the single LanceDB
    // database dir (`memory.lancedb/<table>.lance/`). Lance is
    // versioned append-only, so table dirs include historical versions
    // until compaction — this is honest "bytes on disk", not live-data
    // size. Total covers the whole memory dir (db + sidecars).
    let mem_dir = state.data_dir.join("memory");
    let db_dir = mem_dir.join("memory.lancedb");
    let semantic_bytes = dir_size(&db_dir.join("semantic.lance"));
    let episodic_bytes = dir_size(&db_dir.join("episodic.lance"));
    let total_bytes = dir_size(&mem_dir);

    // Last completed remember pass across all days — "last dream".
    let days = super::days::load(&state.data_dir).await;
    let last_remembered_at = days
        .days
        .values()
        .filter_map(|r| r.remembered_at)
        .max()
        .map(|t| t.to_rfc3339());
    let remembered_days = days
        .days
        .values()
        .filter(|r| r.remembered_at.is_some())
        .count();

    let config = super::config::load(&state.data_dir).await;
    let open_issues = super::issues::open_count(&state.data_dir).await;

    Ok(ok(json!({
        "total": core + semantic + episodic,
        "per_tier": { "core": core, "semantic": semantic, "episodic": episodic },
        "disk_bytes": {
            "semantic": semantic_bytes,
            "episodic": episodic_bytes,
            "total": total_bytes,
        },
        "remembered_days": remembered_days,
        "last_remembered_at": last_remembered_at,
        "open_issues": open_issues,
        "ttl_days": config.episodic_ttl_days,
        "schema_version": crate::memory::schema_version::STORE_SCHEMA_VERSION,
        "embedding_model": crate::embed::MODEL_REPO,
    })))
}

/// Recursive on-disk size of a directory tree; 0 for a missing path.
/// Sync std::fs is fine here — the store dir is a few hundred files.
fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            let p = e.path();
            match e.metadata() {
                Ok(m) if m.is_dir() => dir_size(&p),
                Ok(m) => m.len(),
                Err(_) => 0,
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_size_missing_path_is_zero() {
        assert_eq!(dir_size(Path::new("/nonexistent/path/xyz")), 0);
    }

    #[test]
    fn dir_size_counts_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"12345").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b"), b"123").unwrap();
        assert_eq!(dir_size(dir.path()), 8);
    }
}
