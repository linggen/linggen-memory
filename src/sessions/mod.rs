//! Session scanning + transcript flattening.
//!
//! This module replaces the prior `collect_sessions.sh` + `extract_session.sh`
//! shell scripts with typed Rust, so `ling-mem` is self-contained (no jq,
//! perl, or platform-specific `date`/`stat` shelling). Two public entry
//! points:
//!
//! - [`collect::run`] → NDJSON manifest of sessions on disk for a date.
//! - [`extract::run`] → flattened `[role]: text` transcript for one file.
//!
//! See `doc/tech-spec.md` for the CLI contract and `doc/product-spec.md`
//! for how these fit into the extraction pipeline.

pub mod collect;

/// Where a session came from — drives parsing rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Source {
    /// Claude Code — `~/.claude/projects/<encoded-path>/<id>.jsonl`.
    /// Entries have `type`, `timestamp` (ISO), nested `message.content`.
    #[serde(rename = "CC")]
    ClaudeCode,
    /// Linggen — `~/.linggen/sessions/<id>/messages.jsonl`.
    /// Entries have `from_id`, `timestamp` (epoch secs), flat `content`.
    #[serde(rename = "Linggen")]
    Linggen,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::ClaudeCode => "CC",
            Source::Linggen => "Linggen",
        }
    }
}

impl std::str::FromStr for Source {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "CC" | "cc" | "claude-code" | "claude_code" => Ok(Source::ClaudeCode),
            "Linggen" | "linggen" => Ok(Source::Linggen),
            other => Err(anyhow::anyhow!(
                "unknown source `{other}` — expected CC or Linggen"
            )),
        }
    }
}
