use axum::{extract::State, http::StatusCode, Json};
use linggen_core::{IndexingJob, SourceConfig, SourceType};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use super::index::AppState;
use crate::analytics;

#[derive(Deserialize)]
pub struct AddResourceRequest {
    pub name: String,
    pub resource_type: ResourceType,
    pub path: String, // URL for git/web, file path for local
    #[serde(default)]
    pub include_patterns: Vec<String>, // e.g., ["*.cs", "*.md"]
    #[serde(default)]
    pub exclude_patterns: Vec<String>, // e.g., ["*.meta", "*.asset"]
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    Git,
    Local,
    Web,
    Uploads,
}

impl From<ResourceType> for SourceType {
    fn from(rt: ResourceType) -> Self {
        match rt {
            ResourceType::Git => SourceType::Git,
            ResourceType::Local => SourceType::Local,
            ResourceType::Web => SourceType::Web,
            ResourceType::Uploads => SourceType::Uploads,
        }
    }
}

impl From<SourceType> for ResourceType {
    fn from(st: SourceType) -> Self {
        match st {
            SourceType::Git => ResourceType::Git,
            SourceType::Local => ResourceType::Local,
            SourceType::Web => ResourceType::Web,
            SourceType::Uploads => ResourceType::Uploads,
        }
    }
}

#[derive(Serialize)]
pub struct AddResourceResponse {
    pub id: String,
    pub name: String,
    pub resource_type: ResourceType,
    pub path: String,
    pub enabled: bool,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

#[derive(Serialize)]
pub struct ListResourcesResponse {
    pub resources: Vec<ResourceInfo>,
}

use storage::SourceStats;

#[derive(Serialize)]
pub struct ResourceInfo {
    pub id: String,
    pub name: String,
    pub resource_type: ResourceType,
    pub path: String,
    pub enabled: bool,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub latest_job: Option<IndexingJob>,
    pub stats: Option<SourceStats>,
    pub last_upload_time: Option<String>,
}

#[derive(Deserialize)]
pub struct RemoveResourceRequest {
    pub id: String,
}

#[derive(Serialize)]
pub struct RemoveResourceResponse {
    pub success: bool,
    pub id: String,
}

#[derive(Deserialize)]
pub struct RenameResourceRequest {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct RenameResourceResponse {
    pub success: bool,
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct UpdateResourcePatternsRequest {
    pub id: String,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

#[derive(Serialize)]
pub struct UpdateResourcePatternsResponse {
    pub success: bool,
    pub id: String,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

pub async fn add_resource(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddResourceRequest>,
) -> Result<Json<AddResourceResponse>, (StatusCode, String)> {
    let source_id = Uuid::new_v4().to_string();

    // For Uploads type, no physical path is needed - files are uploaded directly via HTTP
    let path = if req.resource_type == ResourceType::Uploads {
        String::new() // No path for uploads - files stored directly in LanceDB
    } else {
        req.path.clone()
    };

    let source = SourceConfig {
        id: source_id,
        name: req.name.clone(),
        source_type: req.resource_type.into(),
        path,
        enabled: true,
        include_patterns: req.include_patterns.clone(),
        exclude_patterns: req.exclude_patterns.clone(),
        chunk_count: None,
        file_count: None,
        total_size_bytes: None,
        file_sizes: std::collections::HashMap::new(),
        last_upload_time: None,
    };

    state
        .metadata_store
        .add_source(&source)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Start watcher for the new source if it has a .linggen folder
    if source.source_type == linggen_core::SourceType::Local {
        let linggen_path = std::path::PathBuf::from(&source.path).join(".linggen");
        if linggen_path.exists() {
            let internal_index_store = state.internal_index_store.clone();
            let embedding_model = state.embedding_model.clone();
            let chunker = state.chunker.clone();
            let broadcast_tx = state.broadcast_tx.clone();
            let source_id = source.id.clone();

            tokio::spawn(async move {
                if let Err(e) = crate::internal_indexer::start_internal_watcher(
                    internal_index_store,
                    embedding_model,
                    chunker,
                    broadcast_tx,
                    source_id,
                    linggen_path,
                )
                .await
                {
                    tracing::warn!("Failed to start watcher for new source: {}", e);
                }
            });
        }
    }

    // Track source_added analytics event (non-blocking)
    let source_type_for_analytics = source.source_type.clone();
    tokio::spawn(async move {
        analytics::track_source_added(source_type_for_analytics, None).await;
    });

    Ok(Json(AddResourceResponse {
        id: source.id,
        name: source.name,
        resource_type: source.source_type.into(),
        path: source.path,
        enabled: source.enabled,
        include_patterns: source.include_patterns,
        exclude_patterns: source.exclude_patterns,
    }))
}

pub async fn list_resources(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ListResourcesResponse>, (StatusCode, String)> {
    let sources = state
        .metadata_store
        .get_sources()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut resources = Vec::new();

    for s in sources {
        let latest_job = state
            .metadata_store
            .get_latest_job_for_source(&s.id)
            .unwrap_or(None); // Log error in real app

        // Use cached stats from SourceConfig
        let stats = if s.chunk_count.is_some() && s.total_size_bytes.is_some() {
            Some(SourceStats {
                chunk_count: s.chunk_count.unwrap_or(0),
                file_count: s.file_count.unwrap_or(0),
                total_size_bytes: s.total_size_bytes.unwrap_or(0),
            })
        } else {
            None
        };

        resources.push(ResourceInfo {
            id: s.id,
            name: s.name,
            resource_type: s.source_type.into(),
            path: s.path,
            enabled: s.enabled,
            include_patterns: s.include_patterns,
            exclude_patterns: s.exclude_patterns,
            latest_job,
            stats,
            last_upload_time: s.last_upload_time,
        });
    }

    Ok(Json(ListResourcesResponse { resources }))
}

pub async fn remove_resource(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RemoveResourceRequest>,
) -> Result<Json<RemoveResourceResponse>, (StatusCode, String)> {
    tracing::info!("Removing source {} and all associated data", req.id);

    // Get the source before deleting (checking if it exists)
    let _source = state
        .metadata_store
        .get_source(&req.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete from metadata store (redb)
    state
        .metadata_store
        .remove_source(&req.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete all chunks from vector store (LanceDB) - hard delete
    state
        .vector_store
        .delete_by_source(&req.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete file index metadata for this source
    if let Err(e) = state.metadata_store.remove_all_file_index_infos(&req.id) {
        tracing::warn!(
            "Failed to remove file index metadata for source {}: {}",
            req.id,
            e
        );
    }

    tracing::info!(
        "Successfully removed source {} and all associated data",
        req.id
    );

    Ok(Json(RemoveResourceResponse {
        success: true,
        id: req.id,
    }))
}

pub async fn rename_resource(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameResourceRequest>,
) -> Result<Json<RenameResourceResponse>, (StatusCode, String)> {
    // Get the existing source
    let mut source = state
        .metadata_store
        .get_source(&req.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Source not found: {}", req.id),
            )
        })?;

    // Update the name
    source.name = req.name.clone();

    // Save the updated source
    state
        .metadata_store
        .update_source(&source)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(RenameResourceResponse {
        success: true,
        id: req.id,
        name: req.name,
    }))
}

pub async fn update_resource_patterns(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateResourcePatternsRequest>,
) -> Result<Json<UpdateResourcePatternsResponse>, (StatusCode, String)> {
    // Get the existing source
    let mut source = state
        .metadata_store
        .get_source(&req.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Source not found: {}", req.id),
            )
        })?;

    // Update the patterns
    source.include_patterns = req.include_patterns.clone();
    source.exclude_patterns = req.exclude_patterns.clone();

    // Save the updated source
    state
        .metadata_store
        .update_source(&source)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(UpdateResourcePatternsResponse {
        success: true,
        id: req.id,
        include_patterns: req.include_patterns,
        exclude_patterns: req.exclude_patterns,
    }))
}
