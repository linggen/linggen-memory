//! Core types for the facts store.
//!
//! `Fact` is the in-memory representation of a row in LanceDB's `facts` table.
//! The enums (`FactType`, `Outcome`, `Origin`) are validated at the CLI
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

/// A single memory fact — one row in the `facts` LanceDB table.
///
/// Fields mapping to nullable columns are `Option<T>`. `contexts` and `tags`
/// may be empty but are never null.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fact {
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

    pub r#type: FactType,

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

impl Fact {
    /// Minimal constructor for a fresh fact. Generates a new id and stamps
    /// `created_at` to `now`, truncated to microsecond precision so the
    /// value round-trips through LanceDB (which stores `Timestamp(Microsecond)`).
    /// Without the truncation, Linux platforms where `Utc::now()` returns
    /// nanosecond precision would fail equality checks after store roundtrip.
    /// All optional fields default to `None` / empty.
    pub fn new(content: impl Into<String>, r#type: FactType, origin: Origin) -> Self {
        Self {
            id: Uuid::new_v4(),
            content: content.into(),
            vector: None,
            contexts: Vec::new(),
            tags: Vec::new(),
            r#type,
            outcome: None,
            origin,
            cwd: None,
            created_at: Utc::now().trunc_subsecs(6),
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

// ── FactType ────────────────────────────────────────────────────────────────

/// Canonical categories of memory facts. Seven values; `activity` is
/// intentionally excluded — it was the catch-all that caused drift in v1.
///
/// The CLI validates user input against these values. Ingested facts with
/// unrecognized types fall back to `Fact` with a stderr warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FactType {
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

impl FactType {
    pub const ALL: &'static [FactType] = &[
        FactType::Fact,
        FactType::Preference,
        FactType::Decision,
        FactType::Tried,
        FactType::Fixed,
        FactType::Learned,
        FactType::Built,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            FactType::Fact => "fact",
            FactType::Preference => "preference",
            FactType::Decision => "decision",
            FactType::Tried => "tried",
            FactType::Fixed => "fixed",
            FactType::Learned => "learned",
            FactType::Built => "built",
        }
    }
}

impl fmt::Display for FactType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FactType {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "fact" => Ok(FactType::Fact),
            "preference" => Ok(FactType::Preference),
            "decision" => Ok(FactType::Decision),
            "tried" => Ok(FactType::Tried),
            "fixed" => Ok(FactType::Fixed),
            "learned" => Ok(FactType::Learned),
            "built" => Ok(FactType::Built),
            _ => Err(ParseEnumError {
                field: "type",
                value: s.to_string(),
                allowed: FactType::ALL.iter().map(|t| t.as_str()).collect(),
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
    fn fact_type_roundtrip() {
        for t in FactType::ALL {
            let s = t.as_str();
            assert_eq!(FactType::from_str(s).unwrap(), *t);
            assert_eq!(s.to_ascii_uppercase().parse::<FactType>().unwrap(), *t);
        }
    }

    #[test]
    fn fact_type_rejects_unknown() {
        let err = FactType::from_str("activity").unwrap_err();
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
    fn fact_new_sets_sensible_defaults() {
        let f = Fact::new("user likes jazz", FactType::Preference, Origin::User);
        assert_eq!(f.content, "user likes jazz");
        assert_eq!(f.r#type, FactType::Preference);
        assert_eq!(f.origin, Origin::User);
        assert!(f.vector.is_none());
        assert!(f.contexts.is_empty());
        assert!(f.tags.is_empty());
        assert!(f.outcome.is_none());
        assert!(f.cwd.is_none());
        assert!(f.occurred_at.is_none());
        assert!(f.source_session.is_none());
    }

    #[test]
    fn effective_timestamp_falls_back_to_created_at() {
        let f = Fact::new("c", FactType::Fact, Origin::Derived);
        assert_eq!(f.effective_timestamp(), f.created_at);

        let occurred = Utc::now() - chrono::Duration::hours(6);
        let mut f2 = f.clone();
        f2.occurred_at = Some(occurred);
        assert_eq!(f2.effective_timestamp(), occurred);
    }

    #[test]
    fn json_roundtrip_preserves_from_rename() {
        let mut f = Fact::new("x", FactType::Fact, Origin::User);
        f.contexts = vec!["code/linggen".into()];
        f.tags = vec!["intent:learn".into()];

        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"from\":\"user\""));
        assert!(!json.contains("\"origin\""));

        let parsed: Fact = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, f);
    }

    #[test]
    fn json_omits_null_optionals() {
        let f = Fact::new("x", FactType::Fact, Origin::Derived);
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains("\"vector\""));
        assert!(!json.contains("\"outcome\""));
        assert!(!json.contains("\"cwd\""));
        assert!(!json.contains("\"occurred_at\""));
        assert!(!json.contains("\"source_session\""));
    }
}
