use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SourceType {
    Git,
    Local,
    Web,
    Uploads,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub id: String,
    pub name: String,
    pub source_type: SourceType,
    pub path: String, // URL or file path
    pub enabled: bool,
    // File pattern filters (glob patterns like "*.cs", "*.md")
    #[serde(default)]
    pub include_patterns: Vec<String>,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
    // Cached stats from last successful indexing
    pub chunk_count: Option<usize>,
    pub file_count: Option<usize>,
    pub total_size_bytes: Option<usize>,
    // Track individual file sizes for uploads (filename -> size in bytes)
    #[serde(default)]
    pub file_sizes: std::collections::HashMap<String, usize>,
    // Last upload time for uploads sources (ISO 8601 timestamp)
    pub last_upload_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub source_type: SourceType,
    pub source_url: String,
    pub content: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    /// Unique identifier for this chunk (UUID for the chunk row itself)
    pub id: Uuid,
    /// ID of the source this chunk belongs to (matches `SourceConfig.id`, e.g. a repo or local folder)
    pub source_id: String,
    /// Logical document identifier within a source (e.g. file path or URL), shared by all chunks from the same file
    pub document_id: String,
    /// Raw text content of this chunk
    pub content: String,
    /// Optional embedding vector for this chunk (e.g. 384‑dim sentence transformer output)
    pub embedding: Option<Vec<f32>>,
    /// Arbitrary JSON metadata for this chunk (e.g. `file_path`, language, tags)
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingJob {
    pub id: String,
    pub source_id: String,
    pub source_name: String,
    pub source_type: SourceType,
    pub status: JobStatus,
    pub started_at: String, // ISO 8601 timestamp
    pub finished_at: Option<String>,
    pub files_indexed: Option<usize>,
    pub chunks_created: Option<usize>,
    pub total_files: Option<usize>, // Total number of files to index
    pub total_size_bytes: Option<usize>, // Total size in bytes
    pub error: Option<String>,
}

/// Per-file index metadata for incremental updates.
/// Tracks when each file was last indexed so we can detect changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileIndexInfo {
    /// The document_id (typically the relative file path within the source)
    pub document_id: String,
    /// Filesystem modification time at last indexing (Unix timestamp in seconds)
    pub last_indexed_mtime: i64,
    /// Number of chunks created for this file
    pub chunk_count: usize,
    /// File size in bytes at last indexing
    pub file_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_type_serde_roundtrip() {
        let types = [SourceType::Git, SourceType::Local, SourceType::Web, SourceType::Uploads];
        for st in &types {
            let json = serde_json::to_string(st).unwrap();
            let parsed: SourceType = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, st);
        }
    }

    #[test]
    fn test_job_status_serde_roundtrip() {
        let statuses = [
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let parsed: JobStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, status);
        }
    }

    #[test]
    fn test_chunk_serde_roundtrip() {
        let chunk = Chunk {
            id: Uuid::new_v4(),
            source_id: "src-1".into(),
            document_id: "doc-1".into(),
            content: "fn main() {}".into(),
            embedding: Some(vec![0.1, 0.2, 0.3]),
            metadata: serde_json::json!({"file_path": "src/main.rs", "language": "rust"}),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        let parsed: Chunk = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, chunk.id);
        assert_eq!(parsed.source_id, "src-1");
        assert_eq!(parsed.document_id, "doc-1");
        assert_eq!(parsed.content, "fn main() {}");
        assert_eq!(parsed.embedding, Some(vec![0.1, 0.2, 0.3]));
        assert_eq!(
            parsed.metadata.get("file_path").unwrap().as_str().unwrap(),
            "src/main.rs"
        );
    }

    #[test]
    fn test_chunk_without_embedding() {
        let chunk = Chunk {
            id: Uuid::new_v4(),
            source_id: "src-1".into(),
            document_id: "doc-1".into(),
            content: "hello".into(),
            embedding: None,
            metadata: serde_json::json!({}),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        let parsed: Chunk = serde_json::from_str(&json).unwrap();
        assert!(parsed.embedding.is_none());
    }

    #[test]
    fn test_document_serde_roundtrip() {
        let doc = Document {
            id: "doc-1".into(),
            source_type: SourceType::Git,
            source_url: "https://github.com/foo/bar".into(),
            content: "file content".into(),
            metadata: serde_json::json!({"branch": "main"}),
        };
        let json = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "doc-1");
        assert_eq!(parsed.source_type, SourceType::Git);
    }

    #[test]
    fn test_source_config_serde_roundtrip() {
        let cfg = SourceConfig {
            id: "src-1".into(),
            name: "My Repo".into(),
            source_type: SourceType::Local,
            path: "/tmp/repo".into(),
            enabled: true,
            include_patterns: vec!["*.rs".into()],
            exclude_patterns: vec!["target/*".into()],
            chunk_count: Some(100),
            file_count: Some(10),
            total_size_bytes: Some(50000),
            file_sizes: std::collections::HashMap::new(),
            last_upload_time: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: SourceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "src-1");
        assert_eq!(parsed.name, "My Repo");
        assert!(parsed.enabled);
        assert_eq!(parsed.include_patterns, vec!["*.rs"]);
        assert_eq!(parsed.chunk_count, Some(100));
    }

    #[test]
    fn test_source_config_defaults() {
        // Test that optional fields default correctly
        let json = r#"{
            "id": "s1",
            "name": "Test",
            "source_type": "Local",
            "path": "/tmp",
            "enabled": true,
            "chunk_count": null,
            "file_count": null,
            "total_size_bytes": null,
            "last_upload_time": null
        }"#;
        let parsed: SourceConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.include_patterns.is_empty());
        assert!(parsed.exclude_patterns.is_empty());
        assert!(parsed.file_sizes.is_empty());
    }

    #[test]
    fn test_indexing_job_serde() {
        let job = IndexingJob {
            id: "job-1".into(),
            source_id: "src-1".into(),
            source_name: "My Source".into(),
            source_type: SourceType::Web,
            status: JobStatus::Running,
            started_at: "2024-01-01T00:00:00Z".into(),
            finished_at: None,
            files_indexed: Some(5),
            chunks_created: Some(20),
            total_files: Some(10),
            total_size_bytes: Some(1024),
            error: None,
        };
        let json = serde_json::to_string(&job).unwrap();
        let parsed: IndexingJob = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, JobStatus::Running);
        assert_eq!(parsed.files_indexed, Some(5));
    }

    #[test]
    fn test_file_index_info_serde() {
        let info = FileIndexInfo {
            document_id: "src/main.rs".into(),
            last_indexed_mtime: 1700000000,
            chunk_count: 3,
            file_size: 1024,
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: FileIndexInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.document_id, "src/main.rs");
        assert_eq!(parsed.last_indexed_mtime, 1700000000);
        assert_eq!(parsed.chunk_count, 3);
    }
}
