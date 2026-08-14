//! ModelScope fallback download for the embedding model.
//!
//! fastembed fetches Qwen3-Embedding-0.6B through `hf_hub`'s
//! `ApiBuilder::new()`, which hardcodes both the `https://huggingface.co`
//! endpoint and the default cache (`~/.cache/huggingface/hub`) — neither
//! `HF_ENDPOINT` nor `HF_HOME` reaches that path. huggingface.co is blocked
//! in mainland China, so on a CN machine the first-ever embedder init fails
//! and memory is dead on arrival.
//!
//! The fallback: when the HF load fails, fetch the same three files from
//! ModelScope (modelscope.cn — Qwen's official China distribution, also
//! reachable elsewhere), verify each against a SHA-256 pinned HERE, lay them
//! into the hf-hub cache layout, and retry. `hf_hub`'s `ApiRepo::get` is
//! cache-first — with the snapshot present it does no network at all — so
//! the retry loads offline and fastembed needs no changes.
//!
//! Trust model: the mirror is untrusted-but-verified, the same discipline as
//! install-bin.sh — pinned hashes decide, not the host. The pins were taken
//! from the HuggingFace copy and confirmed byte-identical on ModelScope
//! (2026-08-14). A model update changes hashes and lands here as a code
//! change, never silently.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::hash::Sha256;

const MS_BASE: &str = "https://modelscope.cn/models/Qwen/Qwen3-Embedding-0.6B/resolve/master";

/// Snapshot directory name in the hf-hub cache. Any string works — hf-hub
/// just resolves `refs/main` to a snapshot dir — but naming the source makes
/// `ls ~/.cache/huggingface/hub` self-explanatory.
const FALLBACK_REVISION: &str = "modelscope-fallback";

/// The exact files `Qwen3TextEmbedding::from_hf` asks for, with pinned
/// SHA-256 + size. Everything the loader needs and nothing more.
const FILES: &[(&str, &str, u64)] = &[
    (
        "config.json",
        "b5bf1f51fc45be473a54718cef92448d90a1be001bf9b9a44b8c7f10a19feaa9",
        727,
    ),
    (
        "tokenizer.json",
        "def76fb086971c7867b829c23a26261e38d9d74e02139253b38aeb9df8b4b50a",
        11_423_705,
    ),
    (
        "model.safetensors",
        "0437e45c94563b09e13cb7a64478fc406947a93cb34a7e05870fc8dcd48e23fd",
        1_191_586_416,
    ),
];

/// Populate the default hf-hub cache with the embedding model from
/// ModelScope. Idempotent: files already present with matching hashes are
/// kept. Called only after the normal HuggingFace load has failed.
pub fn ensure_model_cached() -> Result<()> {
    // Mirror hf_hub's Cache::default() — fastembed uses ApiBuilder::new(),
    // so this fixed path is the one place the loader will look.
    let root = dirs::home_dir()
        .ok_or_else(|| anyhow!("no home directory"))?
        .join(".cache")
        .join("huggingface")
        .join("hub");
    ensure_model_cached_at(&root)
}

/// Testable core: same work against an explicit cache root.
pub(crate) fn ensure_model_cached_at(root: &Path) -> Result<()> {
    let repo_dir = root.join("models--Qwen--Qwen3-Embedding-0.6B");
    let snap_dir = repo_dir.join("snapshots").join(FALLBACK_REVISION);
    fs::create_dir_all(&snap_dir).context("creating model snapshot dir")?;

    for (name, sha, size) in FILES {
        let dest = snap_dir.join(name);
        if file_matches(&dest, sha) {
            continue;
        }
        tracing::info!("embedder fallback: downloading {name} ({size}B) from ModelScope");
        download_verified(&format!("{MS_BASE}/{name}"), &dest, sha, *size)
            .with_context(|| format!("fetching {name} from ModelScope"))?;
    }

    // Point refs/main at the fallback snapshot so hf-hub's cache-first get()
    // resolves it. Written last: a ref must never name an incomplete
    // snapshot. Overwriting a real HF ref is fine — the bytes are
    // hash-verified identical, and a later successful HF download rewrites
    // the ref itself.
    let refs_dir = repo_dir.join("refs");
    fs::create_dir_all(&refs_dir).context("creating refs dir")?;
    fs::write(refs_dir.join("main"), FALLBACK_REVISION).context("writing model ref")?;
    Ok(())
}

/// True if `path` exists and hashes to `expected_sha`.
fn file_matches(path: &Path, expected_sha: &str) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(_) => return false,
        }
    }
    hasher.hex() == expected_sha
}

/// Stream `url` to a temp file beside `dest`, hashing as it goes; rename
/// into place only when both size and hash match. A failed or tampered
/// download leaves nothing at `dest`.
fn download_verified(url: &str, dest: &Path, expected_sha: &str, expected_size: u64) -> Result<()> {
    let tmp: PathBuf = dest.with_extension("part");
    let resp = ureq::get(url).call().context("requesting model file")?;
    let mut reader = resp.into_body().into_reader();

    let mut out = fs::File::create(&tmp).context("creating temp file")?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).context("reading model bytes")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).context("writing model bytes")?;
        total += n as u64;
    }
    out.flush().ok();
    drop(out);

    let actual = hasher.hex();
    if total != expected_size || actual != expected_sha {
        let _ = fs::remove_file(&tmp);
        return Err(anyhow!(
            "model file failed verification: size {total} (want {expected_size}), sha256 {actual} (want {expected_sha})"
        ));
    }
    fs::rename(&tmp, dest).context("moving verified model file into place")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full fallback against a temp cache root: downloads ~1.2GB from
    /// ModelScope, verifies pins, checks the hf-hub layout resolves.
    #[test]
    #[ignore = "downloads 1.2GB from modelscope.cn; run manually"]
    fn fallback_populates_cache_layout() {
        let root = std::env::temp_dir().join(format!("lm-model-fetch-{}", std::process::id()));
        ensure_model_cached_at(&root).expect("fallback download");
        let repo = root.join("models--Qwen--Qwen3-Embedding-0.6B");
        let rev = std::fs::read_to_string(repo.join("refs/main")).expect("ref");
        for (name, sha, _) in FILES {
            let p = repo.join("snapshots").join(&rev).join(name);
            assert!(file_matches(&p, sha), "{name} missing or wrong hash");
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
