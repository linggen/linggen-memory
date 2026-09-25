//! `POST /api/memory/session_start` — what every host loads when a session
//! begins: the core rows (who the user is) and the standing rules that apply
//! here (how they want the work done), rendered once, in the daemon, so every
//! host injects the same block.
//!
//! A rule is a `type = preference` row in long-term memory (tier semantic or
//! core, never episodic staging). Its project is its `cwd`: null = global.
//! A session loads the global rules plus the rules written at its own cwd or
//! at any ANCESTOR of it — a rule from `~/work/linggen` applies in
//! `~/work/linggen/skills/x`, one from `~/work/sanji` does not, and neither
//! does one written deeper than the session (see `Filters::cwd_lineage`).
//!
//! Rules share a character budget. Rows past it are counted in
//! `over_budget` and named in the block — never dropped silently.

use super::envelope::{ok, ApiError};
use super::state::SharedState;
use crate::memory::{cwd_lineage, AccountScope, Filters, Memory, MemoryType, SortOrder, Tier};
use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

/// Core is meant to be a handful of rows; the cap keeps a runaway tag from
/// inflating every session.
const CORE_LIMIT: usize = 200;
/// Enough for every preference row a person could plausibly hold; the char
/// budget, not this, decides what loads.
const RULES_FETCH_LIMIT: usize = 1000;

pub fn router() -> Router<SharedState> {
    Router::new().route("/api/memory/session_start", post(session_start))
}

#[derive(Debug, Default, Deserialize)]
pub struct SessionStartRequest {
    /// The session's working directory, host-filled. Absent, or a dir that
    /// is not a project ($HOME, ~/.linggen, a temp dir) = global rules only.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Override the configured `session_rules_chars` for this call.
    #[serde(default)]
    pub budget_chars: Option<usize>,
    #[serde(default)]
    pub account: Option<String>,
}

async fn session_start(
    State(state): State<SharedState>,
    Json(req): Json<SessionStartRequest>,
) -> Result<Response, ApiError> {
    let budget = match req.budget_chars {
        Some(b) => b,
        None => {
            crate::http::config::load(&state.data_dir)
                .await
                .session_rules_chars
        }
    };
    let account = AccountScope::from_args(req.account, false);
    let project = req.cwd.as_deref().and_then(project_dir);

    let core = state
        .store
        .list(
            &Filters {
                tier: Some(Tier::Core),
                account: account.clone(),
                ..Default::default()
            },
            SortOrder::Newest,
            CORE_LIMIT,
            0,
        )
        .await?;

    let rules = state
        .store
        .list(
            &rules_filter(project.as_deref(), account),
            SortOrder::Oldest,
            RULES_FETCH_LIMIT,
            0,
        )
        .await?;
    // A core preference is already in the core section.
    let rules: Vec<Memory> = rules
        .into_iter()
        .filter(|r| r.tier != Tier::Core && r.tier != Tier::Episodic)
        .collect();

    let start = SessionStart::build(core, rules, project, budget);
    Ok(ok(start.to_json()))
}

/// The rules a session at `project` loads: preferences, global or written at
/// the project or above it. `None` = a session in no project: globals only.
pub fn rules_filter(project: Option<&str>, account: AccountScope) -> Filters {
    Filters {
        types: vec![MemoryType::Preference],
        cwd_lineage: Some(project.map(cwd_lineage).unwrap_or_default()),
        account,
        ..Default::default()
    }
}

/// The session's cwd, if it is a project. Same rule every host applies
/// (stamp-cwd.sh, recall.sh, the engine's `is_project_dir`): the home dir,
/// `~/.linggen` and temp dirs are nobody's project.
pub fn project_dir(cwd: &str) -> Option<String> {
    let cwd = cwd.trim();
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    if let Some(home) = dirs::home_dir() {
        if path == home || path.starts_with(home.join(".linggen")) {
            return None;
        }
    }
    let tmp = std::env::temp_dir();
    if path.starts_with(&tmp) || path.starts_with("/tmp") || path.starts_with("/private/tmp") {
        return None;
    }
    Some(trimmed.to_string())
}

/// Path depth, for ordering rules shallow-to-deep (global = 0).
fn depth(cwd: Option<&str>) -> usize {
    cwd.map(|c| {
        c.trim_end_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .count()
    })
    .map(|d| d + 1)
    .unwrap_or(0)
}

fn line(row: &Memory) -> String {
    format!("- {} (id={})", row.content.trim(), row.id)
}

pub struct SessionStart {
    pub core: Vec<Memory>,
    pub rules: Vec<Memory>,
    pub skipped: Vec<Memory>,
    pub project: Option<String>,
    pub budget: usize,
    pub block: String,
}

impl SessionStart {
    /// Order the rules (global first, then shallow to deep, oldest first
    /// within a level), load them until the budget is spent, and render the
    /// block. The first rule that does not fit ends the loading: what is
    /// loaded is always a prefix of the order, so a deeper rule never slips
    /// in ahead of a shallower one that did not fit.
    pub fn build(
        core: Vec<Memory>,
        mut rules: Vec<Memory>,
        project: Option<String>,
        budget: usize,
    ) -> Self {
        rules.sort_by(|a, b| {
            depth(a.cwd.as_deref())
                .cmp(&depth(b.cwd.as_deref()))
                .then(a.created_at.cmp(&b.created_at))
        });
        let mut used = 0usize;
        let mut loaded = Vec::new();
        let mut skipped = Vec::new();
        for r in rules {
            let cost = line(&r).chars().count() + 1;
            if skipped.is_empty() && used + cost <= budget {
                used += cost;
                loaded.push(r);
            } else {
                skipped.push(r);
            }
        }
        let block = render(&core, &loaded, &skipped, project.is_some(), budget);
        Self {
            core,
            rules: loaded,
            skipped,
            project,
            budget,
            block,
        }
    }

    pub fn to_json(&self) -> Value {
        let public = |rows: &[Memory]| -> Vec<Value> {
            rows.iter()
                .map(|r| {
                    let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
                    if let Some(o) = v.as_object_mut() {
                        o.remove("vector");
                    }
                    v
                })
                .collect()
        };
        json!({
            "core": public(&self.core),
            "rules": public(&self.rules),
            "block": self.block,
            "chars": self.block.chars().count(),
            "over_budget": self.skipped.len(),
            "over_budget_ids": self.skipped.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            "project": self.project,
            "budget_chars": self.budget,
        })
    }
}

fn render(
    core: &[Memory],
    rules: &[Memory],
    skipped: &[Memory],
    in_project: bool,
    budget: usize,
) -> String {
    let mut sections = Vec::new();
    if !core.is_empty() {
        let rows: Vec<String> = core.iter().map(line).collect();
        sections.push(format!(
            "## Core memory — who the user is\n\n{}",
            rows.join("\n")
        ));
    }
    if !rules.is_empty() || !skipped.is_empty() {
        let title = if in_project {
            "## Standing rules — how to work (this project + global)"
        } else {
            "## Standing rules — how to work (global)"
        };
        let mut rows: Vec<String> = rules.iter().map(line).collect();
        if !skipped.is_empty() {
            let ids: Vec<&str> = skipped.iter().map(|r| r.id.as_str()).collect();
            rows.push(format!(
                "- {} more rules not loaded (over the {budget}-char budget) — condense them: {}",
                skipped.len(),
                ids.join(", ")
            ));
        }
        sections.push(format!("{title}\n\n{}", rows.join("\n")));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Origin;

    fn rule(content: &str, cwd: Option<&str>) -> Memory {
        let mut m = Memory::new(content, MemoryType::Preference, Origin::User);
        m.cwd = cwd.map(str::to_string);
        m
    }

    #[test]
    fn home_tmp_and_linggen_state_are_not_projects() {
        let home = dirs::home_dir().unwrap();
        let h = home.to_string_lossy().to_string();
        assert_eq!(project_dir(&h), None);
        assert_eq!(project_dir(&format!("{h}/")), None);
        assert_eq!(project_dir(&format!("{h}/.linggen")), None);
        assert_eq!(project_dir(&format!("{h}/.linggen/missions")), None);
        assert_eq!(project_dir("/tmp/x"), None);
        assert_eq!(project_dir("/private/tmp"), None);
        assert_eq!(project_dir(""), None);
        assert_eq!(
            project_dir(&format!("{h}/work/linggen/")).as_deref(),
            Some(format!("{h}/work/linggen").as_str())
        );
    }

    #[test]
    fn globals_come_first_then_shallow_to_deep() {
        let rules = vec![
            rule("deep", Some("/u/w/linggen/skills")),
            rule("mid", Some("/u/w/linggen")),
            rule("global", None),
        ];
        let s = SessionStart::build(vec![], rules, Some("/u/w/linggen/skills".into()), 10_000);
        let order: Vec<&str> = s.rules.iter().map(|r| r.content.as_str()).collect();
        assert_eq!(order, ["global", "mid", "deep"]);
        assert!(s.block.contains("(this project + global)"));
        assert!(s.skipped.is_empty());
        assert!(!s.block.contains("not loaded"));
    }

    #[test]
    fn rules_past_the_budget_are_counted_and_named_never_dropped() {
        let rules = vec![
            rule(&"a".repeat(50), None),
            rule(&"b".repeat(50), None),
            rule("short", Some("/u/w/p")),
        ];
        let ids: Vec<String> = rules.iter().map(|r| r.id.clone()).collect();
        // Room for the first line only (50 chars + " (id=XXXXXXXXXX)" + "- ").
        let s = SessionStart::build(vec![], rules, Some("/u/w/p".into()), 80);
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.skipped.len(), 2);
        // The short rule would fit, but loading stops at the first miss.
        assert_eq!(s.skipped[1].content, "short");
        let json = s.to_json();
        assert_eq!(json["over_budget"], json!(2));
        assert!(s.block.contains("2 more rules not loaded"), "{}", s.block);
        assert!(s.block.contains(&ids[1]) && s.block.contains(&ids[2]));
        assert_eq!(json["chars"], json!(s.block.chars().count()));
    }

    #[test]
    fn the_block_has_both_sections_with_ids() {
        let mut core = Memory::new("Alex — founder", MemoryType::Fact, Origin::User);
        core.tier = Tier::Core;
        let r = rule("Always run tests", None);
        let s = SessionStart::build(vec![core.clone()], vec![r.clone()], None, 6000);
        assert!(s
            .block
            .starts_with("## Core memory — who the user is\n\n- Alex — founder (id="));
        assert!(s.block.contains("## Standing rules — how to work (global)"));
        assert!(s
            .block
            .contains(&format!("- Always run tests (id={})", r.id)));
    }

    #[test]
    fn an_empty_store_renders_nothing() {
        let s = SessionStart::build(vec![], vec![], None, 6000);
        assert_eq!(s.block, "");
    }
}
