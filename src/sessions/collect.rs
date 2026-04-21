//! Scan session stores and emit an NDJSON manifest for a target date.
//!
//! This is the Rust port of `collect_sessions.sh`. The output shape matches
//! the shell script with one addition (`project_root`) so the extraction
//! subagent can auto-tag facts with the originating project context.

use super::Source;
use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// One row in the manifest. JSON shape matches the v1 shell script plus a
/// new `project_root` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEntry {
    pub filepath: String,
    pub source: Source,
    pub label: String,
    /// Target date the manifest was built for (YYYY-MM-DD). Not the file's
    /// mtime — the scanner picked this file because its mtime matched.
    pub date: String,
    pub bytes: u64,
    pub user_turns: usize,
    /// Workspace directory the session ran in. `None` when we can't figure
    /// it out (Linggen sessions pre-date the `cwd:` field, CC project
    /// directory is un-decodeable, etc.).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub project_root: Option<String>,
}

/// Scan both stores for sessions whose file mtime falls on `target_date`,
/// and write one JSON object per session to `out`.
///
/// `home` is the user's home directory — respected so callers (tests,
/// odd environments) can point at a sandbox without setting `$HOME`.
pub fn run<W: Write>(home: &Path, target_date: NaiveDate, out: &mut W) -> Result<()> {
    let date_str = target_date.format("%Y-%m-%d").to_string();

    let cc_dir = home.join(".claude").join("projects");
    if cc_dir.is_dir() {
        collect_cc(&cc_dir, target_date, &date_str, out)?;
    }

    let ling_dir = home.join(".linggen").join("sessions");
    if ling_dir.is_dir() {
        collect_linggen(&ling_dir, target_date, &date_str, out)?;
    }

    Ok(())
}

// ── Claude Code ─────────────────────────────────────────────────────────────

fn collect_cc<W: Write>(
    cc_dir: &Path,
    target_date: NaiveDate,
    date_str: &str,
    out: &mut W,
) -> Result<()> {
    for project_dir in list_subdirs(cc_dir)? {
        let project_name = match project_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let project_root = decode_cc_project_dir(&project_name);

        for entry in fs::read_dir(&project_dir)
            .with_context(|| format!("reading {}", project_dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if !mtime_on(&path, target_date)? {
                continue;
            }

            let session_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let label = format!("{project_name}/{session_name}");
            let bytes = file_bytes(&path)?;
            let user_turns = count_user_turns_cc(&path)?;

            write_entry(
                out,
                &SessionEntry {
                    filepath: path.to_string_lossy().to_string(),
                    source: Source::ClaudeCode,
                    label,
                    date: date_str.to_string(),
                    bytes,
                    user_turns,
                    project_root: project_root.clone(),
                },
            )?;
        }
    }
    Ok(())
}

/// CC encodes the project path by replacing path separators with dashes
/// and prefixing with a dash, so `/Users/x/workspace/y` → `-Users-x-workspace-y`.
/// Best-effort reverse; returns `None` if the string doesn't start with `-`.
fn decode_cc_project_dir(encoded: &str) -> Option<String> {
    let rest = encoded.strip_prefix('-')?;
    Some(format!("/{}", rest.replace('-', "/")))
}

/// Count user turns in a CC session. A CC "user" row is a real user
/// message when its `message.content` has a text item or is a non-empty
/// string; rows that only contain tool_result entries don't count.
fn count_user_turns_cc(path: &Path) -> Result<usize> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut count = 0usize;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let Some(content) = v.get("message").and_then(|m| m.get("content")) else {
            continue;
        };
        if let Some(arr) = content.as_array() {
            let any_text = arr.iter().any(|item| {
                item.get("type").and_then(|t| t.as_str()) == Some("text")
                    && item
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| !s.is_empty())
                        .unwrap_or(false)
            });
            if any_text {
                count += 1;
            }
        } else if let Some(s) = content.as_str() {
            if !s.is_empty() {
                count += 1;
            }
        }
    }
    Ok(count)
}

// ── Linggen ─────────────────────────────────────────────────────────────────

fn collect_linggen<W: Write>(
    ling_dir: &Path,
    target_date: NaiveDate,
    date_str: &str,
    out: &mut W,
) -> Result<()> {
    for session_dir in list_subdirs(ling_dir)? {
        let jsonl = session_dir.join("messages.jsonl");
        if !jsonl.is_file() {
            continue;
        }
        if !mtime_on(&jsonl, target_date)? {
            continue;
        }

        // Only ingest user-initiated sessions — mirrors the shell guard.
        let meta = match read_linggen_meta(&session_dir.join("session.yaml")) {
            Some(m) => m,
            None => continue,
        };
        if meta.creator.as_deref() != Some("user") {
            continue;
        }

        let session_name = session_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let label = meta.title.clone().unwrap_or_else(|| session_name.clone());
        let bytes = file_bytes(&jsonl)?;
        let user_turns = count_user_turns_linggen(&jsonl)?;

        write_entry(
            out,
            &SessionEntry {
                filepath: jsonl.to_string_lossy().to_string(),
                source: Source::Linggen,
                label,
                date: date_str.to_string(),
                bytes,
                user_turns,
                project_root: meta.cwd,
            },
        )?;
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
struct LinggenMeta {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    creator: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

fn read_linggen_meta(path: &Path) -> Option<LinggenMeta> {
    let text = fs::read_to_string(path).ok()?;
    serde_yml::from_str(&text).ok()
}

/// Count Linggen user turns: rows with `from_id == "user"`.
fn count_user_turns_linggen(path: &Path) -> Result<usize> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut count = 0usize;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("from_id").and_then(|f| f.as_str()) == Some("user") {
            count += 1;
        }
    }
    Ok(count)
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn list_subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    Ok(out)
}

fn file_bytes(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .len())
}

fn mtime_on(path: &Path, target: NaiveDate) -> Result<bool> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let mtime = meta
        .modified()
        .with_context(|| format!("mtime for {}", path.display()))?;
    let local: DateTime<Local> = mtime.into();
    Ok(local.date_naive() == target)
}

fn write_entry<W: Write>(out: &mut W, entry: &SessionEntry) -> Result<()> {
    serde_json::to_writer(&mut *out, entry).context("serializing manifest entry")?;
    out.write_all(b"\n").context("writing newline")?;
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_project_dir() {
        assert_eq!(
            decode_cc_project_dir("-Users-lianghuang-workspace-linggen"),
            Some("/Users/lianghuang/workspace/linggen".into())
        );
        assert_eq!(decode_cc_project_dir("no-leading-dash"), None);
    }

    #[test]
    fn source_json_labels() {
        let v: String = serde_json::to_string(&Source::ClaudeCode).unwrap();
        assert_eq!(v, "\"CC\"");
        let v: String = serde_json::to_string(&Source::Linggen).unwrap();
        assert_eq!(v, "\"Linggen\"");
    }

    /// End-to-end: build a fake home with one CC session + one Linggen
    /// session and verify the manifest emits exactly what's expected.
    #[test]
    fn end_to_end_manifest() {
        use std::io::Write as _;
        let home = tempfile::TempDir::new().unwrap();

        // ── CC session
        let cc_proj = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-tmp-proj");
        fs::create_dir_all(&cc_proj).unwrap();
        let cc_file = cc_proj.join("a1b2.jsonl");
        let mut f = fs::File::create(&cc_file).unwrap();
        // Two user messages, one with array content, one with string.
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"hello"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":"world"}}}}"#
        )
        .unwrap();
        // One tool_result-only user message — should NOT count.
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","content":"…"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"role":"assistant","content":"hi"}}}}"#
        )
        .unwrap();

        // ── Linggen session
        let ling_sess = home
            .path()
            .join(".linggen")
            .join("sessions")
            .join("sess-42");
        fs::create_dir_all(&ling_sess).unwrap();
        fs::write(
            ling_sess.join("session.yaml"),
            "id: sess-42\ntitle: demo chat\ncreator: user\ncwd: /Users/x/proj\n",
        )
        .unwrap();
        let ling_file = ling_sess.join("messages.jsonl");
        let mut f = fs::File::create(&ling_file).unwrap();
        writeln!(f, r#"{{"from_id":"user","timestamp":0,"content":"hey"}}"#).unwrap();
        writeln!(f, r#"{{"from_id":"ling","timestamp":1,"content":"hi"}}"#).unwrap();

        // Skip sessions with creator != "user" — add one and assert it's ignored.
        let ling_sess_bot = home
            .path()
            .join(".linggen")
            .join("sessions")
            .join("sess-99-bot");
        fs::create_dir_all(&ling_sess_bot).unwrap();
        fs::write(
            ling_sess_bot.join("session.yaml"),
            "id: sess-99-bot\ncreator: mission\n",
        )
        .unwrap();
        fs::write(
            ling_sess_bot.join("messages.jsonl"),
            r#"{"from_id":"user","content":"x"}
"#,
        )
        .unwrap();

        let today = Local::now().date_naive();
        let mut buf = Vec::new();
        run(home.path(), today, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "manifest should have exactly 2 rows: {lines:?}"
        );

        let entries: Vec<SessionEntry> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        // CC entry
        let cc = entries
            .iter()
            .find(|e| e.source == Source::ClaudeCode)
            .unwrap();
        assert_eq!(cc.label, "-tmp-proj/a1b2");
        assert_eq!(cc.user_turns, 2, "tool_result-only row should not count");
        assert_eq!(cc.project_root.as_deref(), Some("/tmp/proj"));

        // Linggen entry
        let ling = entries
            .iter()
            .find(|e| e.source == Source::Linggen)
            .unwrap();
        assert_eq!(ling.label, "demo chat");
        assert_eq!(ling.user_turns, 1);
        assert_eq!(ling.project_root.as_deref(), Some("/Users/x/proj"));
    }

    #[test]
    fn missing_home_is_silent() {
        let home = tempfile::TempDir::new().unwrap();
        // No .claude or .linggen subdirs — run should succeed with empty output.
        let mut buf = Vec::new();
        run(home.path(), Local::now().date_naive(), &mut buf).unwrap();
        assert!(buf.is_empty());
    }
}
