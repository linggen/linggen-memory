//! Core types for the memory store.
//!
//! `Memory` is the in-memory representation of a row in a LanceDB memory table.
//! The enums (`MemoryType`, `Outcome`, `Origin`) are validated at the CLI
//! boundary and stored as plain Utf8 columns — Arrow does not have a true
//! enum type, so keeping validation in Rust gives us typo-safety without
//! migration cost.
//!
//! Schema matches `doc/tech-spec.md`. Any field change here must also update
//! `schema.rs` and bump the store's format version.

use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// A single memory — one row in a LanceDB memory table.
///
/// Fields mapping to nullable columns are `Option<T>`. `contexts` and `tags`
/// may be empty but are never null.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    pub id: Uuid,
    pub content: String,

    /// Embedding of `content`. May briefly be `None` between insert and embed
    /// passes in batch pipelines; search filters ignore rows without a vector.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub vector: Option<Vec<f32>>,

    /// Scope tags — usually hierarchical path-like: `code/linggen`,
    /// `music/piano`. Shorter-on-average than `tags`; the primary filter
    /// dimension.
    #[serde(default)]
    pub contexts: Vec<String>,

    /// Secondary metadata — topic, intent, people, mood. Free-form labels
    /// with prefix convention for pseudo-structure: `intent:learn`,
    /// `topic:coding`, `person:bob`. Promote a prefix to a first-class field
    /// only if it becomes heavily filtered in practice.
    #[serde(default)]
    pub tags: Vec<String>,

    pub r#type: MemoryType,

    /// Storage tier. `core` facts are the small, durable identity/preference
    /// set surfaced eagerly; `semantic` facts are the broader RAG-retrieved
    /// pool. Older JSON without this field defaults to `semantic`.
    #[serde(default)]
    pub tier: Tier,

    /// Only meaningful for action-flavored types (`tried`, `fixed`,
    /// `decision`). Nullable — a `preference` fact has no outcome.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub outcome: Option<Outcome>,

    /// Who authored the fact — distinguishes user utterances from agent
    /// actions from inferred patterns. Serialized as `"from"` for natural
    /// JSON output; the Rust field is `origin` to avoid the keyword clash.
    #[serde(rename = "from")]
    pub origin: Origin,

    /// Working directory captured at extraction time. Nullable — manual adds
    /// have none. Useful for filtering by project area and as an auto-tag
    /// hint during extraction.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cwd: Option<String>,

    /// When the fact entered memory.
    pub created_at: DateTime<Utc>,

    /// When the fact's stored content was last mutated by an `update`.
    /// Null until the first edit; bumped to `now` on every `update`.
    /// Together with `created_at` it forms the decay/TTL clock:
    /// `COALESCE(updated_at, created_at)` — touching a fact resets its age.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub updated_at: Option<DateTime<Utc>>,

    /// When the described thing happened. Falls back to `created_at` in
    /// queries when absent. Differs from `created_at` when extraction runs
    /// later than the events it captures (e.g. nightly extraction of day's
    /// sessions).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub occurred_at: Option<DateTime<Utc>>,

    /// Session id the fact was extracted from. Nullable — manual adds have
    /// none. Serves as the escape hatch: if a fact is ambiguous later, the
    /// original session can be re-read.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_session: Option<String>,
}

impl Memory {
    /// Minimal constructor for a fresh fact. Generates a new id and stamps
    /// `created_at` to `now`, truncated to microsecond precision so the
    /// value round-trips through LanceDB (which stores `Timestamp(Microsecond)`).
    /// Without the truncation, Linux platforms where `Utc::now()` returns
    /// nanosecond precision would fail equality checks after store roundtrip.
    /// All optional fields default to `None` / empty.
    pub fn new(content: impl Into<String>, r#type: MemoryType, origin: Origin) -> Self {
        Self {
            id: Uuid::new_v4(),
            content: content.into(),
            vector: None,
            contexts: Vec::new(),
            tags: Vec::new(),
            r#type,
            tier: Tier::default(),
            outcome: None,
            origin,
            cwd: None,
            created_at: Utc::now().trunc_subsecs(6),
            updated_at: None,
            occurred_at: None,
            source_session: None,
        }
    }

    /// `occurred_at` if set, otherwise `created_at`. Used as the effective
    /// timestamp for age-based queries.
    pub fn effective_timestamp(&self) -> DateTime<Utc> {
        self.occurred_at.unwrap_or(self.created_at)
    }
}

// ── MemoryType ────────────────────────────────────────────────────────────────

/// Canonical categories of a memory. Seven values; `activity` is
/// intentionally excluded — it was the catch-all that caused drift in v1.
///
/// The CLI validates user input against these values. Ingested facts with
/// unrecognized types fall back to `Fact` with a stderr warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    /// Stable truth about the user / world (identity, hobbies, domain facts).
    Fact,
    /// How the user wants the agent to work — cross-project behavioral rules.
    Preference,
    /// A choice plus its reasoning.
    Decision,
    /// An attempt — pair with `outcome` to record whether it worked.
    Tried,
    /// A bug with symptoms + fix in `content`; pair with `outcome`.
    Fixed,
    /// A cross-project env / tool gotcha.
    Learned,
    /// A specific thing shipped — narrow, not an activity catch-all.
    Built,
}

impl MemoryType {
    pub const ALL: &'static [MemoryType] = &[
        MemoryType::Fact,
        MemoryType::Preference,
        MemoryType::Decision,
        MemoryType::Tried,
        MemoryType::Fixed,
        MemoryType::Learned,
        MemoryType::Built,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::Fact => "fact",
            MemoryType::Preference => "preference",
            MemoryType::Decision => "decision",
            MemoryType::Tried => "tried",
            MemoryType::Fixed => "fixed",
            MemoryType::Learned => "learned",
            MemoryType::Built => "built",
        }
    }
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemoryType {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "fact" => Ok(MemoryType::Fact),
            "preference" => Ok(MemoryType::Preference),
            "decision" => Ok(MemoryType::Decision),
            "tried" => Ok(MemoryType::Tried),
            "fixed" => Ok(MemoryType::Fixed),
            "learned" => Ok(MemoryType::Learned),
            "built" => Ok(MemoryType::Built),
            _ => Err(ParseEnumError {
                field: "type",
                value: s.to_string(),
                allowed: MemoryType::ALL.iter().map(|t| t.as_str()).collect(),
            }),
        }
    }
}

// ── Outcome ─────────────────────────────────────────────────────────────────

/// Result of an action-flavored fact. Only meaningful when `type` is one of
/// `tried`, `fixed`, `decision`. Skipped for `preference`, `fact`, `built`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Positive,
    Negative,
    Neutral,
}

impl Outcome {
    pub const ALL: &'static [Outcome] = &[Outcome::Positive, Outcome::Negative, Outcome::Neutral];

    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Positive => "positive",
            Outcome::Negative => "negative",
            Outcome::Neutral => "neutral",
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Outcome {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "positive" | "pos" | "+" | "worked" => Ok(Outcome::Positive),
            "negative" | "neg" | "-" | "failed" => Ok(Outcome::Negative),
            "neutral" | "mixed" => Ok(Outcome::Neutral),
            _ => Err(ParseEnumError {
                field: "outcome",
                value: s.to_string(),
                allowed: Outcome::ALL.iter().map(|o| o.as_str()).collect(),
            }),
        }
    }
}

// ── Origin (the `from` field) ───────────────────────────────────────────────

/// Who authored the fact. Named `Origin` in Rust to avoid clashing with the
/// `from` keyword; serialized as `"from"` for natural JSON output.
///
/// - `User`: user stated it (direct utterance).
/// - `Agent`: the agent did / decided / observed it.
/// - `Derived`: inferred from patterns across multiple sessions — not
///   attributable to a single utterance.
///
/// Default is `Derived` — the safest choice when the source is ambiguous.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    User,
    Agent,
    #[default]
    Derived,
}

impl Origin {
    pub const ALL: &'static [Origin] = &[Origin::User, Origin::Agent, Origin::Derived];

    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::Agent => "agent",
            Origin::Derived => "derived",
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Origin {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "user" => Ok(Origin::User),
            "agent" | "assistant" | "model" => Ok(Origin::Agent),
            "derived" | "inferred" => Ok(Origin::Derived),
            _ => Err(ParseEnumError {
                field: "from",
                value: s.to_string(),
                allowed: Origin::ALL.iter().map(|o| o.as_str()).collect(),
            }),
        }
    }
}

// ── Tier ────────────────────────────────────────────────────────────────────

/// Storage tier of a fact.
///
/// - `Core`: the small, durable identity/preference set surfaced eagerly
///   (always-on context, not retrieval-gated).
/// - `Semantic`: the broader pool retrieved by semantic search.
///
/// Default is `Semantic` — the safe choice for ingested facts that haven't
/// been explicitly promoted to the core set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Core,
    #[default]
    Semantic,
}

impl Tier {
    pub const ALL: &'static [Tier] = &[Tier::Core, Tier::Semantic];

    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Core => "core",
            Tier::Semantic => "semantic",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Tier {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "core" => Ok(Tier::Core),
            "semantic" => Ok(Tier::Semantic),
            _ => Err(ParseEnumError {
                field: "tier",
                value: s.to_string(),
                allowed: Tier::ALL.iter().map(|t| t.as_str()).collect(),
            }),
        }
    }
}

// ── Parse errors ────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
#[error("invalid {field} `{value}` — allowed: {}", allowed.join(", "))]
pub struct ParseEnumError {
    pub field: &'static str,
    pub value: String,
    pub allowed: Vec<&'static str>,
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_type_roundtrip() {
        for t in MemoryType::ALL {
            let s = t.as_str();
            assert_eq!(MemoryType::from_str(s).unwrap(), *t);
            assert_eq!(s.to_ascii_uppercase().parse::<MemoryType>().unwrap(), *t);
        }
    }

    #[test]
    fn memory_type_rejects_unknown() {
        let err = MemoryType::from_str("activity").unwrap_err();
        assert_eq!(err.field, "type");
        assert_eq!(err.value, "activity");
        assert!(err.allowed.contains(&"fact"));
    }

    #[test]
    fn outcome_accepts_aliases() {
        assert_eq!(Outcome::from_str("worked").unwrap(), Outcome::Positive);
        assert_eq!(Outcome::from_str("failed").unwrap(), Outcome::Negative);
        assert_eq!(Outcome::from_str("+").unwrap(), Outcome::Positive);
        assert_eq!(Outcome::from_str("mixed").unwrap(), Outcome::Neutral);
    }

    #[test]
    fn origin_aliases() {
        assert_eq!(Origin::from_str("assistant").unwrap(), Origin::Agent);
        assert_eq!(Origin::from_str("model").unwrap(), Origin::Agent);
        assert_eq!(Origin::from_str("inferred").unwrap(), Origin::Derived);
    }

    #[test]
    fn origin_default_is_derived() {
        assert_eq!(Origin::default(), Origin::Derived);
    }

    #[test]
    fn tier_default_is_semantic() {
        assert_eq!(Tier::default(), Tier::Semantic);
    }

    #[test]
    fn memory_new_sets_sensible_defaults() {
        let f = Memory::new("user likes jazz", MemoryType::Preference, Origin::User);
        assert_eq!(f.content, "user likes jazz");
        assert_eq!(f.r#type, MemoryType::Preference);
        assert_eq!(f.origin, Origin::User);
        assert_eq!(f.tier, Tier::Semantic);
        assert!(f.vector.is_none());
        assert!(f.contexts.is_empty());
        assert!(f.tags.is_empty());
        assert!(f.outcome.is_none());
        assert!(f.cwd.is_none());
        assert!(f.updated_at.is_none());
        assert!(f.occurred_at.is_none());
        assert!(f.source_session.is_none());
    }

    #[test]
    fn effective_timestamp_falls_back_to_created_at() {
        let f = Memory::new("c", MemoryType::Fact, Origin::Derived);
        assert_eq!(f.effective_timestamp(), f.created_at);

        let occurred = Utc::now() - chrono::Duration::hours(6);
        let mut f2 = f.clone();
        f2.occurred_at = Some(occurred);
        assert_eq!(f2.effective_timestamp(), occurred);
    }

    #[test]
    fn json_roundtrip_preserves_from_rename() {
        let mut f = Memory::new("x", MemoryType::Fact, Origin::User);
        f.contexts = vec!["code/linggen".into()];
        f.tags = vec!["intent:learn".into()];

        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"from\":\"user\""));
        assert!(!json.contains("\"origin\""));

        let parsed: Memory = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, f);
    }

    #[test]
    fn json_omits_null_optionals() {
        let f = Memory::new("x", MemoryType::Fact, Origin::Derived);
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains("\"vector\""));
        assert!(!json.contains("\"outcome\""));
        assert!(!json.contains("\"cwd\""));
        assert!(!json.contains("\"updated_at\""));
        assert!(!json.contains("\"occurred_at\""));
        assert!(!json.contains("\"source_session\""));
    }
}
