//! Text → 384-dim embedding via `fastembed` (ONNX Runtime + MiniLM-L6-v2).
//!
//! The model matches the vector dimension locked into the `facts` schema
//! ([`crate::facts::VECTOR_DIM`] = 384). A future model swap to
//! `bge-small-en-v1.5` (also 384-dim) needs no schema change — just change
//! the `Model` default here.
//!
//! ## First-run cost
//!
//! `fastembed` downloads the ONNX model from HuggingFace Hub on first use
//! (~23 MB for MiniLM-L6-v2), caching under `$FASTEMBED_CACHE_PATH` or a
//! platform-standard cache dir. Subsequent invocations load from cache.
//!
//! ## CLI lifecycle
//!
//! `ling-mem` is a one-shot CLI in v0.1, so every invocation that needs
//! embeddings pays the ONNX session load cost (~100–300 ms). Daemon mode
//! (Phase 8) will keep a single [`Embedder`] alive across requests.

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::facts::VECTOR_DIM;

/// Lazily-constructed text embedder. Wraps `fastembed`'s [`TextEmbedding`]
/// behind an owning interface so CLI callers don't leak ONNX types.
pub struct Embedder {
    inner: TextEmbedding,
}

impl Embedder {
    /// Construct with the default model (MiniLM-L6-v2).
    pub fn new() -> Result<Self> {
        Self::with_model(EmbeddingModel::AllMiniLML6V2)
    }

    /// Construct with an explicit model. Useful for v0.2's upgrade to
    /// `BGESmallENV15` (same 384-dim, better quality).
    pub fn with_model(model: EmbeddingModel) -> Result<Self> {
        let inner = TextEmbedding::try_new(InitOptions::new(model))
            .context("initializing fastembed text embedder")?;
        Ok(Self { inner })
    }

    /// Embed a single text into a 384-dim unit vector.
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed_many(&[text.to_string()])?;
        out.pop()
            .context("embedder returned no vector for single input")
    }

    /// Embed a batch of texts. Order is preserved.
    pub fn embed_many(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let vectors = self
            .inner
            .embed(texts.to_vec(), None)
            .context("running embedding model")?;

        // Defensive check — fastembed could in principle change the default
        // model to something non-384-dim. Our schema doesn't forgive that.
        for (i, v) in vectors.iter().enumerate() {
            if v.len() != VECTOR_DIM as usize {
                return Err(anyhow::anyhow!(
                    "embedding {} has dim {}, expected {}",
                    i,
                    v.len(),
                    VECTOR_DIM
                ));
            }
        }
        Ok(vectors)
    }
}
