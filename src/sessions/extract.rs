//! Flatten a session `.jsonl` file into a `[role]: text` transcript.
//!
//! Rust port of `extract_session.sh`. Output goes to `out` (stdout in CLI
//! use) as plain text — *not* NDJSON. Each line is `[user]:` or
//! `[assistant]:` followed by a message capped at 2000 characters. After
//! all lines are composed, a noise-stripping pass removes injected content
//! (system-reminder tags, command markers, fenced code blocks) and
//! collapses 3+ newlines to 2.

use super::Source;
use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Local, NaiveDate, TimeZone};
use once_cell::sync::Lazy;
use regex::Regex;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

const MAX_MSG_CHARS: usize = 2000;

static SYSTEM_REMINDER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)<system-reminder>.*?</system-reminder>").unwrap()
});

static COMMAND_TAG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)<command-(?:name|message|args)>.*?</command-(?:name|message|args)>").unwrap()
});

static FENCED_CODE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)```.*?```").unwrap()
});

static MULTI_NEWLINE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{3,}").unwrap());

/// Flatten `path` to `[role]: text` lines for `target_date`, writing the
/// noise-stripped result to `out`.
pub fn run<W: Write>(
    path: &Path,
    source: Source,
    target_date: NaiveDate,
    out: &mut W,
) -> Result<()> {
    if !path.is_file() {
        return Err(anyhow!("file not found: {}", path.display()));
    }

    let mut raw = String::new();
    match source {
        Source::ClaudeCode => flatten_cc(path, target_date, &mut raw)?,
        Source::Linggen => flatten_linggen(path, target_date, &mut raw)?,
    }

    let stripped = strip_noise(&raw);
    out.write_all(stripped.as_bytes())
        .context("writing flattened transcript")?;
    Ok(())
}

/// Apply the same noise pass the shell does: drop system-reminder /
/// command-* tags and fenced code blocks, then collapse 3+ newlines to 2.
pub fn strip_noise(input: &str) -> String {
    let a = SYSTEM_REMINDER_RE.replace_all(input, "");
    let b = COMMAND_TAG_RE.replace_all(&a, "");
    let c = FENCED_CODE_RE.replace_all(&b, "");
    MULTI_NEWLINE_RE.replace_all(&c, "\n\n").into_owned()
}

// ── Claude Code ─────────────────────────────────────────────────────────────

fn flatten_cc(path: &Path, target_date: NaiveDate, out: &mut String) -> Result<()> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let date_prefix = target_date.format("%Y-%m-%d").to_string();

    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty != "user" && ty != "assistant" {
            continue;
        }

        let ts = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
        if !ts.starts_with(&date_prefix) {
            continue;
        }

        // role: prefer message.role, fall back to the outer type
        let role = v
            .get("message")
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or(ty);

        let content = match v.get("message").and_then(|m| m.get("content")) {
            Some(c) => extract_cc_content(c),
            None => String::new(),
        };

        append_line(out, role, &content);
    }
    Ok(())
}

/// Extract a text payload from CC's polymorphic `content` field:
/// - string → use as-is
/// - array → concatenate text items with newlines
/// - else → empty
fn extract_cc_content(content: &serde_json::Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let parts: Vec<&str> = arr
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    item.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        return parts.join("\n");
    }
    String::new()
}

// ── Linggen ─────────────────────────────────────────────────────────────────

fn flatten_linggen(path: &Path, target_date: NaiveDate, out: &mut String) -> Result<()> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;

    // Day window in local time — matches shell semantics.
    let day_start = Local
        .from_local_datetime(&target_date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .ok_or_else(|| anyhow!("ambiguous local midnight for {target_date}"))?;
    let start = day_start.timestamp();
    let end = (day_start + Duration::days(1)).timestamp();

    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        let from = v.get("from_id").and_then(|f| f.as_str()).unwrap_or("");
        if from != "user" && from != "ling" {
            continue;
        }

        let is_observation = v
            .get("is_observation")
            .and_then(|o| o.as_bool())
            .unwrap_or(false);
        if is_observation {
            continue;
        }

        let ts = v.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
        if ts < start || ts >= end {
            continue;
        }

        let content = v
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let role = if from == "ling" { "assistant" } else { "user" };
        append_line(out, role, &content);
    }
    Ok(())
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn append_line(out: &mut String, role: &str, content: &str) {
    if content.is_empty() {
        return;
    }
    let clipped: String = content.chars().take(MAX_MSG_CHARS).collect();
    out.push('[');
    out.push_str(role);
    out.push_str("]: ");
    out.push_str(&clipped);
    out.push('\n');
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // strip_noise ————————————————————————————————————————————

    #[test]
    fn strip_noise_removes_system_reminder() {
        let input = "hello <system-reminder>rules</system-reminder> world";
        assert_eq!(strip_noise(input), "hello  world");
    }

    #[test]
    fn strip_noise_removes_command_tags() {
        let input = "<command-name>foo</command-name><command-message>bar</command-message>\nbody";
        assert_eq!(strip_noise(input), "\nbody");
    }

    #[test]
    fn strip_noise_removes_code_fences() {
        let input = "before ```rust\nfn main() {}\n``` after";
        assert_eq!(strip_noise(input), "before  after");
    }

    #[test]
    fn strip_noise_collapses_newlines() {
        assert_eq!(strip_noise("a\n\n\n\nb"), "a\n\nb");
        assert_eq!(strip_noise("a\n\nb"), "a\n\nb"); // leave 2 alone
    }

    #[test]
    fn strip_noise_multiline_reminder() {
        let input = "start\n<system-reminder>\n  line one\n  line two\n</system-reminder>\nend";
        assert_eq!(strip_noise(input), "start\n\nend");
    }

    // CC extraction ——————————————————————————————————————————

    #[test]
    fn cc_flattens_array_content() {
        let c = serde_json::json!([
            { "type": "text", "text": "hello" },
            { "type": "text", "text": "world" }
        ]);
        assert_eq!(extract_cc_content(&c), "hello\nworld");
    }

    #[test]
    fn cc_skips_non_text_content_blocks() {
        let c = serde_json::json!([
            { "type": "tool_use", "name": "Bash" },
            { "type": "text", "text": "done" }
        ]);
        assert_eq!(extract_cc_content(&c), "done");
    }

    #[test]
    fn cc_end_to_end() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"2026-04-21T10:00:00Z","message":{{"role":"user","content":"hello"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-04-21T10:00:01Z","message":{{"role":"assistant","content":[{{"type":"text","text":"hi"}}]}}}}"#
        )
        .unwrap();
        // Wrong date — filtered out.
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"2026-04-20T10:00:00Z","message":{{"role":"user","content":"yesterday"}}}}"#
        )
        .unwrap();
        // Tool-only assistant message — has no text, skipped.
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-04-21T10:00:02Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Bash"}}]}}}}"#
        )
        .unwrap();

        let target = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let mut buf = Vec::new();
        run(f.path(), Source::ClaudeCode, target, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();

        assert_eq!(out, "[user]: hello\n[assistant]: hi\n");
    }

    // Linggen extraction ——————————————————————————————————————

    #[test]
    fn linggen_end_to_end() {
        let mut f = NamedTempFile::new().unwrap();
        let target = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let day_start = Local
            .from_local_datetime(&target.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp();

        writeln!(
            f,
            r#"{{"from_id":"user","timestamp":{},"content":"hey"}}"#,
            day_start + 10
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"from_id":"ling","timestamp":{},"content":"hi"}}"#,
            day_start + 20
        )
        .unwrap();
        // Observation → skipped
        writeln!(
            f,
            r#"{{"from_id":"ling","timestamp":{},"content":"internal","is_observation":true}}"#,
            day_start + 30
        )
        .unwrap();
        // Outside day window → skipped
        writeln!(
            f,
            r#"{{"from_id":"user","timestamp":{},"content":"tomorrow"}}"#,
            day_start + 86400 + 1
        )
        .unwrap();

        let mut buf = Vec::new();
        run(f.path(), Source::Linggen, target, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();

        assert_eq!(out, "[user]: hey\n[assistant]: hi\n");
    }

    #[test]
    fn message_longer_than_cap_is_truncated() {
        let mut f = NamedTempFile::new().unwrap();
        let long: String = "a".repeat(3000);
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"2026-04-21T00:00:00Z","message":{{"role":"user","content":{}}}}}"#,
            serde_json::Value::String(long)
        )
        .unwrap();

        let target = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let mut buf = Vec::new();
        run(f.path(), Source::ClaudeCode, target, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        // `[user]: ` prefix + exactly 2000 'a's + '\n'
        assert_eq!(out.len(), "[user]: ".len() + MAX_MSG_CHARS + 1);
    }

    #[test]
    fn missing_file_is_clear_error() {
        let target = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let mut buf = Vec::new();
        let err = run(Path::new("/no/such/file.jsonl"), Source::ClaudeCode, target, &mut buf)
            .unwrap_err();
        assert!(err.to_string().contains("file not found"));
    }
}
