use anyhow::Result;
use embeddings::EmbeddingModel;
use linggen_core::Chunk;
use linggen_intent::{Intent, IntentClassifier};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use storage::{SourceProfile, UserPreferences, VectorStore};
use tracing::info;

pub mod profile_manager;
pub use profile_manager::ProfileManager;

/// Strategy for constructing the enhanced prompt
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PromptStrategy {
    /// Include full code chunks (default)
    FullCode,
    /// Include file paths and summaries only
    ReferenceOnly,
    /// Focus on high-level architecture
    Architectural,
}

/// Lightweight metadata about a context chunk returned to the frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextChunkMeta {
    /// ID of the source this chunk belongs to
    pub source_id: String,
    /// Logical document identifier (usually file path)
    pub document_id: String,
    /// File path alias for convenience (falls back to document_id)
    pub file_path: String,
}

/// Result of prompt enhancement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedPrompt {
    /// Original user query
    pub original_query: String,

    /// Enhanced prompt ready for AI assistant
    pub enhanced_prompt: String,

    /// Detected intent
    pub intent: Intent,

    /// Retrieved context chunks (raw text)
    pub context_chunks: Vec<String>,

    /// Metadata for each retrieved context chunk (aligned with context_chunks)
    #[serde(default)]
    pub context_metadata: Vec<ContextChunkMeta>,

    /// Applied user preferences
    pub preferences_applied: bool,
}

use linggen_llm::MiniLLM;
use tokio::sync::{Mutex, RwLock};

/// Prompt enhancer - orchestrates all stages
pub struct PromptEnhancer {
    #[allow(dead_code)]
    _intent_classifier: IntentClassifier,
    embedding_model: Arc<RwLock<Option<EmbeddingModel>>>,
    vector_store: Arc<VectorStore>,
}

impl PromptEnhancer {
    pub fn new(
        embedding_model: Arc<RwLock<Option<EmbeddingModel>>>,
        vector_store: Arc<VectorStore>,
        llm: Option<Arc<Mutex<MiniLLM>>>,
    ) -> Self {
        let intent_classifier = IntentClassifier::new(llm);

        Self {
            _intent_classifier: intent_classifier,
            embedding_model,
            vector_store,
        }
    }

    /// Enhance a user prompt through the pipeline
    /// Note: LLM-based intent detection is deprecated and always skipped.
    /// Intent is now provided externally by MCP (Cursor).
    pub async fn enhance(
        &self,
        query: &str,
        preferences: &UserPreferences,
        profile: &SourceProfile,
        strategy: PromptStrategy,
        exclude_source_id: Option<&str>,
    ) -> Result<EnhancedPrompt> {
        // Stage 1: Intent Classification - always use default (intent now comes from MCP)
        info!("Stage 1: Using default intent (AskQuestion). Intent detection is provided by MCP.");
        let intent = linggen_intent::Intent::AskQuestion;

        // Stage 2: Context Retrieval (RAG)
        info!("Stage 2: Retrieving context via RAG...");

        let model_guard = self.embedding_model.read().await;
        let model = model_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Embedding model is initializing"))?;

        let query_embedding = model.embed(query)?;
        let rag_chunks = self
            .vector_store
            .search(query_embedding, Some(query), 5)
            .await?;

        // Optional filtering: exclude chunks from a specific source (useful for "across projects")
        let rag_chunks: Vec<Chunk> = if let Some(excluded) = exclude_source_id {
            rag_chunks
                .into_iter()
                .filter(|c| c.source_id != excluded)
                .collect()
        } else {
            rag_chunks
        };

        // Stage 3: Context Analysis (select top 3)
        info!("Stage 3: Analyzing context...");
        let top_chunks = if !rag_chunks.is_empty() {
            rag_chunks.into_iter().take(3).collect()
        } else {
            Vec::new()
        };

        // Stage 4: User Preferences
        info!("Stage 4: Applying user preferences...");
        let pref_instructions = preferences.to_prompt_instructions();

        // Stage 5: Prompt Enhancement
        info!(
            "Stage 5: Generating enhanced prompt with strategy {:?}...",
            strategy
        );
        let enhanced = self
            .generate_enhanced_prompt(
                query,
                &intent,
                &top_chunks,
                &pref_instructions,
                profile,
                strategy,
            )
            .await?;

        // Build result
        let chunk_contents: Vec<String> = top_chunks.iter().map(|c| c.content.clone()).collect();
        let context_metadata: Vec<ContextChunkMeta> = top_chunks
            .iter()
            .map(|c| {
                // Prefer explicit file_path metadata; fall back to document_id
                let file_path = c
                    .metadata
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| c.document_id.clone());

                ContextChunkMeta {
                    source_id: c.source_id.clone(),
                    document_id: c.document_id.clone(),
                    file_path,
                }
            })
            .collect();

        Ok(EnhancedPrompt {
            original_query: query.to_string(),
            enhanced_prompt: enhanced,
            intent,
            context_chunks: chunk_contents,
            context_metadata,
            preferences_applied: true,
        })
    }

    /// Generate the final enhanced prompt using a template-based approach
    async fn generate_enhanced_prompt(
        &self,
        query: &str,
        intent: &Intent,
        context: &[Chunk],
        preferences: &str,
        profile: &SourceProfile,
        strategy: PromptStrategy,
    ) -> Result<String> {
        // Format context based on strategy
        let context_text = if context.is_empty() {
            "No additional context available.".to_string()
        } else {
            match strategy {
                PromptStrategy::FullCode => context
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let file_info = if let Some(path) = c.metadata.get("file_path") {
                            format!("\nFile: {}", path.as_str().unwrap_or("Unknown"))
                        } else {
                            String::new()
                        };
                        format!(
                            "--- Context {} ---{}{}\n------------------",
                            i + 1,
                            file_info,
                            c.content
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                PromptStrategy::ReferenceOnly => context
                    .iter()
                    .map(|c| {
                        // Prefer explicit file_path metadata; fall back to document_id
                        let path = c
                            .metadata
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| c.document_id.clone());
                        format!("- {}", path)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                PromptStrategy::Architectural => {
                    // For architectural, we list files/modules at a high level
                    context
                        .iter()
                        .map(|c| {
                            let path = c
                                .metadata
                                .get("file_path")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| c.document_id.clone());
                            format!("- {}", path)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }
        };

        // Construct structured prompt
        let profile_text = format!(
            "SOURCE PROFILE:\nName: {}\nDescription: {}",
            profile.profile_name, profile.description,
        );

        let memory_anchors_text = "MEMORY ANCHORS:\nLinggen supports memory anchors in code. \
        - To read a memory: If you see comments like '//linggen memory: <ID>' (e.g., //linggen memory: 4f7a905824), use the 'memory_fetch_by_meta' tool with key='id' and value='<ID>'. \
        - To create/update a memory: Simply edit or create a markdown file in the '.linggen/memory/' directory. Ensure it has a YAML frontmatter with a unique 10-character 'id', a 'title', and optional 'tags'. Linggen will automatically index these changes.";

        let enhanced = format!(
            "{}\n\n{}\n\nCONTEXT:\n{}\n\nUSER PREFERENCES:\n{}\n\nTASK: {:?}\n\nQUERY:\n{}",
            profile_text, memory_anchors_text, context_text, preferences, intent, query
        );

        Ok(enhanced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_strategy_serde() {
        let strategies = [
            (PromptStrategy::FullCode, "\"full_code\""),
            (PromptStrategy::ReferenceOnly, "\"reference_only\""),
            (PromptStrategy::Architectural, "\"architectural\""),
        ];
        for (strategy, expected) in &strategies {
            let json = serde_json::to_string(strategy).unwrap();
            assert_eq!(&json, expected);
            let parsed: PromptStrategy = serde_json::from_str(expected).unwrap();
            assert_eq!(&parsed, strategy);
        }
    }

    #[test]
    fn test_enhanced_prompt_serde() {
        let ep = EnhancedPrompt {
            original_query: "What is this?".into(),
            enhanced_prompt: "Enhanced...".into(),
            intent: Intent::AskQuestion,
            context_chunks: vec!["chunk1".into()],
            context_metadata: vec![ContextChunkMeta {
                source_id: "s1".into(),
                document_id: "d1".into(),
                file_path: "src/main.rs".into(),
            }],
            preferences_applied: true,
        };
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: EnhancedPrompt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.original_query, "What is this?");
        assert!(parsed.preferences_applied);
        assert_eq!(parsed.context_metadata.len(), 1);
    }

    #[test]
    fn test_context_chunk_meta_serde() {
        let meta = ContextChunkMeta {
            source_id: "src-1".into(),
            document_id: "doc-1".into(),
            file_path: "path/to/file.rs".into(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: ContextChunkMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.source_id, "src-1");
        assert_eq!(parsed.file_path, "path/to/file.rs");
    }
}
