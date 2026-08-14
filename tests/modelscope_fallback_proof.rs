//! One-shot proof harness for the ModelScope fallback (run with --ignored).
//! Expects the real HF cache repo dir to be absent/partial; exercises the
//! production ensure_model_cached() against the DEFAULT cache root, then
//! loads the embedder, which must resolve entirely from the fallback
//! snapshot (hf-hub is cache-first).
use ling_mem::embed::{ensure_model_cached, Embedder};

#[test]
#[ignore = "touches the real ~/.cache/huggingface hub; run manually"]
fn fallback_then_load() {
    ensure_model_cached().expect("fallback populate");
    let e = Embedder::new().expect("embedder from fallback cache");
    let v = e.embed_one("你好，灵根").expect("embed");
    assert_eq!(v.len(), 1024);
}
