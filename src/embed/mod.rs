//! Text → 1024-dim embedding via `fastembed`'s Qwen3 backend (candle + Qwen3-Embedding-0.6B).
//!
//! The model matches the vector dimension locked into the memory schema
//! ([`crate::memory::VECTOR_DIM`] = 1024).
//!
//! ## Why Qwen3-Embedding-0.6B
//!
//! - Multilingual (100+ langs incl. Chinese) — MiniLM-L6-v2 was English-only
//!   and embedded Chinese rows as noise. Bilingual users (CN + EN) couldn't
//!   reliably retrieve their own facts.
//! - 1024-dim output (MRL-truncatable to 32–1024) — more semantic resolution
//!   than 384-dim MiniLM, at the cost of 2.7× larger vectors in the store.
//! - Instruction-aware: queries are prefixed with `"query: "`, stored
//!   passages with `"passage: "`. Skipping the prefixes costs ~1–5 pp recall.
//!
//! ## First-run cost
//!
//! The Qwen3-Embedding-0.6B weights (~1.2 GB BF16) are downloaded from
//! HuggingFace Hub on first use and cached under the user's HF cache dir.
//! Subsequent invocations load from cache. macOS Metal / Linux CUDA / CPU
//! are auto-selected at runtime via [`best_device`].
//!
//! ## Daemon lifecycle
//!
//! The daemon (`ling-mem serve`) constructs one [`Embedder`] at startup and
//! shares it across requests — model load (~1–2 s) happens once.

use std::sync::Arc;

use anyhow::{Context, Result};
use candle_core::{DType, Device};
use fastembed::Qwen3TextEmbedding;

use crate::memory::VECTOR_DIM;

pub const MODEL_REPO: &str = "Qwen/Qwen3-Embedding-0.6B";
const MAX_SEQ_LEN: usize = 512;
const QUERY_PREFIX: &str = "query: ";
const PASSAGE_PREFIX: &str = "passage: ";

/// Hard cap on the byte length of any single text handed to the encoder.
/// fastembed already truncates to [`MAX_SEQ_LEN`] *tokens*, but that cap
/// is applied *after* the full string is tokenized — a multi-MB blob
/// (a giant `Memory_add` content, a runaway query) still forces a huge
/// tokenizer allocation before truncation. 16 KiB sits comfortably above
/// a 512-token window in any language (incl. multi-byte CJK) while making
/// the worst-case per-call cost deterministic. Over-long input is
/// truncated, not rejected: the full content is still stored verbatim;
/// only the retrieval vector loses the tail (already lossy past 512 tok).
const MAX_EMBED_INPUT_BYTES: usize = 16 * 1024;

/// Max texts encoded in one candle forward pass. Larger batches are
/// processed in chunks so peak attention-tensor memory stays bounded
/// regardless of caller batch size (`ling-mem add --stdin`, reindex).
const MAX_EMBED_BATCH: usize = 32;

pub struct Embedder {
    inner: Qwen3TextEmbedding,
    /// Serializes candle forward passes. The daemon shares one [`Embedder`]
    /// across a multi-thread Tokio runtime; without this, N concurrent
    /// `/api/memory/*` calls ran N simultaneous forward passes on the same
    /// Metal device, and candle's per-shape Metal buffer pool grew without
    /// bound for the daemon's whole life (the 109 GB OOM). One inference
    /// at a time keeps resident memory flat.
    gate: tokio::sync::Mutex<()>,
}

impl Embedder {
    /// Construct with the default model (Qwen3-Embedding-0.6B) on the best
    /// available device (Metal on macOS, CUDA where compiled in, else CPU).
    ///
    /// Weights load as F16 (not F32): the checkpoint ships BF16, F16 halves
    /// the resident model footprint (~2.4 GB → ~1.2 GB), and the L2-
    /// normalized cosine outputs differ from F32 by ~1e-3 — far inside the
    /// 0.03 dedup-threshold margin, so no reindex is needed.
    pub fn new() -> Result<Self> {
        let device = best_device()?;
        tracing::info!(
            "embedder: model={MODEL_REPO} device={device:?} dtype=F16 max_seq_len={MAX_SEQ_LEN}"
        );
        let inner = Qwen3TextEmbedding::from_hf(MODEL_REPO, &device, DType::F16, MAX_SEQ_LEN)
            .context("initializing Qwen3 text embedder")?;
        Ok(Self {
            inner,
            gate: tokio::sync::Mutex::new(()),
        })
    }

    /// Embed one stored passage, serialized and run on the blocking pool.
    /// HTTP handlers must use this (never the sync [`Self::embed_one`])
    /// so concurrent requests cannot stack forward passes.
    pub async fn embed_passage(self: Arc<Self>, text: String) -> Result<Vec<f32>> {
        let _guard = self.gate.lock().await;
        let me = Arc::clone(&self);
        tokio::task::spawn_blocking(move || me.embed_one(&text))
            .await
            .context("embed worker join failed")?
    }

    /// Embed many stored passages in one serialized, off-thread call.
    /// HTTP handlers use this for bulk inserts (`/api/memory/add_batch`) so
    /// a whole `add --stdin` import is one gate acquisition + one forward-
    /// pass sequence instead of N independent `embed_passage` calls.
    /// Order is preserved; internally chunked to [`MAX_EMBED_BATCH`].
    pub async fn embed_passages(self: Arc<Self>, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let _guard = self.gate.lock().await;
        let me = Arc::clone(&self);
        tokio::task::spawn_blocking(move || me.embed_many(&texts))
            .await
            .context("embed worker join failed")?
    }

    /// Embed one search query, serialized and run on the blocking pool.
    /// HTTP handlers must use this (never the sync [`Self::embed_query`]).
    pub async fn embed_query_serialized(self: Arc<Self>, text: String) -> Result<Vec<f32>> {
        let _guard = self.gate.lock().await;
        let me = Arc::clone(&self);
        tokio::task::spawn_blocking(move || me.embed_query(&text))
            .await
            .context("embed worker join failed")?
    }

    /// Embed a passage (stored memory content) into a 1024-dim unit vector.
    /// Prefixed with `"passage: "` per Qwen3's retrieval convention.
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed_with_prefix(&[text.to_string()], PASSAGE_PREFIX)?;
        out.pop()
            .context("embedder returned no vector for single input")
    }

    /// Embed a batch of passages. Order is preserved.
    pub fn embed_many(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_with_prefix(texts, PASSAGE_PREFIX)
    }

    /// Embed a search query into a 1024-dim unit vector. Prefixed with
    /// `"query: "` per Qwen3's retrieval convention. Use this for the
    /// query side of a similarity search; use [`embed_one`] for stored
    /// content.
    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed_with_prefix(&[text.to_string()], QUERY_PREFIX)?;
        out.pop()
            .context("embedder returned no vector for single query")
    }

    fn embed_with_prefix(&self, texts: &[String], prefix: &str) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let total_bytes: usize = texts.iter().map(String::len).sum();
        let truncated = texts.iter().any(|t| t.len() > MAX_EMBED_INPUT_BYTES);
        tracing::info!(
            "embed: batch={} total_bytes={} max_text_bytes={} truncated={} prefix={:?}",
            texts.len(),
            total_bytes,
            texts.iter().map(String::len).max().unwrap_or(0),
            truncated,
            prefix.trim()
        );

        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| format!("{prefix}{}", clamp_bytes(t, MAX_EMBED_INPUT_BYTES)))
            .collect();

        // Chunk so peak attention-tensor memory is bounded by MAX_EMBED_BATCH
        // even when a caller passes an arbitrarily large batch.
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(prefixed.len());
        for chunk in prefixed.chunks(MAX_EMBED_BATCH) {
            let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
            let part = self
                .inner
                .embed(&refs)
                .context("running Qwen3 embedding model")?;
            vectors.extend(part);
        }

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

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
    }

    /// Empirical probe for the dedup threshold (Phase 1a, sub-step 4).
    /// Ignored by default — needs the ~1.2 GB Qwen3-Embedding-0.6B weights.
    /// Run with: `cargo test --lib embed::tests::dedup_threshold_probe
    /// -- --ignored --nocapture` and read the printed sims off stderr.
    #[test]
    #[ignore = "downloads/loads the 1.2GB Qwen3 model; run manually to retune DEDUP_SIMILARITY_THRESHOLD"]
    fn dedup_threshold_probe() {
        let e = Embedder::new().expect("embedder");

        // Restatement pairs: same fact, reworded — these SHOULD merge.
        let restatements = [
            (
                "the user prefers concise replies without preamble",
                "user wants short answers and no fluff at the start",
            ),
            (
                "webrtc data channel closes after 30 seconds of inactivity",
                "the WebRTC DC drops the connection once it's been idle for half a minute",
            ),
            (
                "the user's cat is named Xiaoman",
                "user has a cat called Xiaoman",
            ),
        ];

        // Related-but-distinct pairs: same topic, different fact — these
        // must NOT merge (merging would lose information).
        let related = [
            (
                "the user prefers concise replies without preamble",
                "the user prefers dark mode in the editor",
            ),
            (
                "webrtc data channel closes after 30 seconds of inactivity",
                "webrtc uses STUN servers for NAT traversal",
            ),
            (
                "the user's cat is named Xiaoman",
                "the user has a dog named Rex",
            ),
        ];

        eprintln!("\n=== RESTATEMENT (should merge — want high sim) ===");
        let mut restate_sims = Vec::new();
        for (a, b) in restatements {
            let va = e.embed_one(a).unwrap();
            let vb = e.embed_one(b).unwrap();
            let s = cosine(&va, &vb);
            restate_sims.push(s);
            eprintln!("{s:.4}  | {a:?} <> {b:?}");
        }

        eprintln!("\n=== RELATED-BUT-DISTINCT (should NOT merge — want lower sim) ===");
        let mut related_sims = Vec::new();
        for (a, b) in related {
            let va = e.embed_one(a).unwrap();
            let vb = e.embed_one(b).unwrap();
            let s = cosine(&va, &vb);
            related_sims.push(s);
            eprintln!("{s:.4}  | {a:?} <> {b:?}");
        }

        let min_restate = restate_sims.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_related = related_sims
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max);
        eprintln!("\nmin restatement sim = {min_restate:.4}");
        eprintln!("max related sim     = {max_related:.4}");
        eprintln!(
            "→ a clean threshold sits between {max_related:.4} and {min_restate:.4} \
             (e.g. {:.2})\n",
            (max_related + min_restate) / 2.0
        );
    }
}

/// Truncate `s` to at most `max` bytes, snapping back to the nearest
/// UTF-8 char boundary so we never split a multi-byte codepoint (slicing
/// mid-codepoint would panic). Returns a borrow when no cut is needed.
fn clamp_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Pick the best available compute device for candle: Metal on macOS,
/// otherwise CPU. Falls back silently if Metal fails to initialize
/// (e.g. inside a sandboxed environment).
fn best_device() -> Result<Device> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(d) = Device::new_metal(0) {
            return Ok(d);
        }
    }
    Ok(Device::Cpu)
}
