//! HTTP client path — routes CLI data ops to a running daemon.
//!
//! When the daemon is up, the CLI talks to it over HTTP instead of opening
//! its own copy of the LanceDB store. This eliminates the historical
//! "CLI sees a different file from the daemon" split: there's exactly one
//! writer (the daemon), and the CLI is just a typed shell client.
//!
//! Lifecycle commands (`serve`/`start`/`stop`/`restart`/`status`) never
//! route through here — they manage the daemon itself. Only data ops
//! (`add`/`get`/`search`/`list`/`update`/`delete`/`forget`) consult
//! [`try_running_daemon`].
//!
//! Fallback: when no daemon is reachable, [`super::run`] falls back to the
//! direct `MemoryStore` path. That keeps the offline / first-run experience
//! working without requiring the user to start the daemon manually.

use crate::cli::{
    AddArgs, CliMemoryType, CliOrigin, CliOutcome, FilterArgs, ForgetArgs, ListArgs, OutputFormat,
    SearchArgs, UpdateArgs,
};
use crate::daemon::pidfile;
use crate::memory::Memory;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// Health-probe budget. Must be short — runs on every CLI invocation, so a
/// hung daemon should not stall an offline command.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Per-operation budget. Search + add embed on the daemon side and can take
/// a few seconds on cold start.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Bulk-import budget. `add --stdin` now ships the whole batch in one
/// request, so the single call embeds + commits every row — far more work
/// than a single add. A generous ceiling keeps a large import from tripping
/// the 30s `OP_TIMEOUT`; the work is bounded (one commit) so it won't wedge.
const BULK_TIMEOUT: Duration = Duration::from_secs(600);

/// Probe the daemon: read its pidfile, confirm the pid is alive, then
/// confirm `/api/health` returns 200. Returns the base URL on success.
///
/// Failure modes that all collapse to `None` (CLI falls back to direct
/// store): missing pidfile, dead pid, refused connection, non-2xx health,
/// timeout. Any of these mean "no daemon" — the CLI should not produce a
/// noisy error here.
pub(crate) async fn try_running_daemon(skill_dir: &Path) -> Option<String> {
    let info = pidfile::read(skill_dir).ok().flatten()?;
    if !pidfile::pid_is_alive(info.pid) {
        return None;
    }
    let base = format!("http://127.0.0.1:{}", info.port);
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;
    let resp = client.get(format!("{base}/api/health")).send().await.ok()?;
    if resp.status().is_success() {
        Some(base)
    } else {
        None
    }
}

/// Probe; if the daemon isn't up, autostart it (same pattern Linggen's
/// engine uses in `engine::capability_tools::dispatch`) and re-probe.
///
/// On any failure we fall back to `None`, so the caller can use direct
/// `MemoryStore` mode. Failures we surface to stderr so the user can see
/// *why* the daemon path was skipped — silent fallback would hide real
/// problems (e.g. port in use, binary missing).
pub(crate) async fn try_running_or_start(
    data_dir: &Path,
    skill_dir: &Path,
) -> Option<String> {
    if let Some(url) = try_running_daemon(skill_dir).await {
        return Some(url);
    }

    // No daemon reachable — start one. Matches Linggen engine autostart
    // semantics: data_dir flows through `LINGGEN_DATA_DIR` so the child's
    // store path resolution matches ours.
    match crate::daemon::lifecycle::start(data_dir, skill_dir, crate::daemon::DEFAULT_PORT).await {
        Ok(_) => try_running_daemon(skill_dir).await,
        Err(e) => {
            eprintln!("ling-mem: autostart failed ({e}); using direct store");
            None
        }
    }
}

/// POST `body` to `<base>/<path>` and unwrap the standard envelope.
async fn post(base: &str, path: &str, body: &Value) -> Result<Value> {
    post_with_timeout(base, path, body, OP_TIMEOUT).await
}

/// POST with an explicit client timeout. Bulk imports use a longer budget
/// than the default [`OP_TIMEOUT`] because one request now does all the work.
async fn post_with_timeout(
    base: &str,
    path: &str,
    body: &Value,
    timeout: Duration,
) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("building HTTP client")?;
    let url = format!("{base}{path}");
    let resp = client
        .post(&url)
        .json(body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let envelope: Value = resp
        .json()
        .await
        .with_context(|| format!("parsing JSON response from {url}"))?;
    if !status.is_success() {
        let msg = envelope
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        let code = envelope
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("ERROR");
        return Err(anyhow!("daemon error [{code}]: {msg}"));
    }
    match envelope.get("ok").and_then(|v| v.as_bool()) {
        Some(true) => Ok(envelope.get("data").cloned().unwrap_or(Value::Null)),
        _ => Err(anyhow!(
            "daemon response missing or false `ok` field: {envelope}"
        )),
    }
}

// ── Subcommand routers ──────────────────────────────────────────────────────

pub(crate) async fn add(base: &str, args: AddArgs, format: OutputFormat) -> Result<()> {
    if args.stdin {
        return add_stdin(base, format).await;
    }
    let content = args
        .content
        .ok_or_else(|| anyhow!("add: provide content or use --stdin"))?;

    let host = args.host.or_else(super::detect_host);
    let body = build_add_body(
        content,
        args.r#type,
        args.contexts,
        args.tags,
        args.from,
        args.outcome,
        args.cwd,
        args.occurred_at,
        args.source_session,
        args.skip_dedup,
        host,
    );

    let data = post(base, "/api/memory/add", &body).await?;
    emit_add_outcome(&data, format)
}

async fn add_stdin(base: &str, format: OutputFormat) -> Result<()> {
    let stdin = std::io::stdin();
    let mut facts: Vec<Value> = Vec::new();
    for (i, line) in stdin.lines().enumerate() {
        let line = line.with_context(|| format!("reading stdin line {}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        // Stdin facts are full Memory JSON (round-trippable from previous
        // emits). Strip `vector` and `id` / `created_at` — the daemon
        // re-embeds and assigns fresh row identity.
        let mut fact: Value = serde_json::from_str(&line)
            .with_context(|| format!("parsing JSON on stdin line {}", i + 1))?;
        if let Some(obj) = fact.as_object_mut() {
            obj.remove("vector");
            obj.remove("id");
            obj.remove("created_at");
            // Bulk imports always skip dedup (matches direct-mode semantics).
            obj.insert("skip_dedup".into(), Value::Bool(true));
        }
        facts.push(fact);
    }
    if facts.is_empty() {
        if matches!(format, OutputFormat::Text) {
            eprintln!("(no facts on stdin)");
        }
        return Ok(());
    }

    // One batched request → one commit per table on the daemon side. This
    // replaces the old per-row POST loop (one HTTP call + one LanceDB
    // version per row), which made large imports degrade super-linearly and
    // eventually trip `OP_TIMEOUT`.
    let body = json!({ "facts": facts });
    let data = post_with_timeout(base, "/api/memory/add_batch", &body, BULK_TIMEOUT).await?;

    match format {
        // Preserve the previous NDJSON-per-row output contract.
        OutputFormat::Json => {
            if let Some(arr) = data.get("facts").and_then(|v| v.as_array()) {
                for f in arr {
                    emit_add_outcome(&json!({ "action": "added", "fact": f }), format)?;
                }
            }
        }
        OutputFormat::Text => {
            let count = data.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
            println!("added {count} facts");
        }
    }
    Ok(())
}

pub(crate) async fn get(base: &str, id: &str, format: OutputFormat) -> Result<()> {
    let data = post(base, "/api/memory/get", &json!({ "id": id })).await?;
    emit_fact_value(&data, format)
}

pub(crate) async fn search(base: &str, args: SearchArgs, format: OutputFormat) -> Result<()> {
    let mut body = filter_body(&args.filters);
    body["query"] = Value::String(args.query);
    body["limit"] = json!(args.limit);
    if let Some(s) = args.min_score {
        body["min_score"] = json!(s);
    }
    let data = post(base, "/api/memory/search", &body).await?;
    emit_scored_fact_array(&data, format)
}

pub(crate) async fn list(base: &str, args: ListArgs, format: OutputFormat) -> Result<()> {
    // `--older-than` is folded into `until` by `filter_body` (which
    // calls the same `FilterArgs::into_filters` mapping used on the
    // direct-store path), so there's nothing extra to forward here.
    let mut body = filter_body(&args.filters);
    body["sort"] = match args.sort {
        crate::cli::CliSort::Newest => json!("newest"),
        crate::cli::CliSort::Oldest => json!("oldest"),
    };
    body["limit"] = json!(args.limit);
    body["offset"] = json!(args.offset);
    let data = post(base, "/api/memory/list", &body).await?;
    emit_fact_array(&data, format)
}

pub(crate) async fn update(base: &str, args: UpdateArgs, format: OutputFormat) -> Result<()> {
    let mut body = json!({ "id": args.id });
    if let Some(content) = args.content {
        body["content"] = Value::String(content);
    }
    if let Some(contexts) = args.contexts {
        body["contexts"] = json!(contexts);
    }
    if let Some(tags) = args.tags {
        body["tags"] = json!(tags);
    }
    if let Some(t) = args.r#type {
        body["type"] = Value::String(cli_memory_type_str(t).to_string());
    }
    if let Some(t) = args.tier {
        body["tier"] = Value::String(cli_tier_str(t).to_string());
    }
    if let Some(o) = args.from {
        body["from"] = Value::String(cli_origin_str(o).to_string());
    }
    if let Some(o) = args.outcome {
        body["outcome"] = Value::String(cli_outcome_str(o).to_string());
    }
    if args.clear_outcome {
        body["clear_outcome"] = Value::Bool(true);
    }
    if let Some(cwd) = args.cwd {
        body["cwd"] = Value::String(cwd);
    }
    if args.clear_cwd {
        body["clear_cwd"] = Value::Bool(true);
    }
    let data = post(base, "/api/memory/update", &body).await?;
    emit_fact_value(&data, format)
}

pub(crate) async fn delete(
    base: &str,
    id: &str,
    yes: bool,
    format: OutputFormat,
) -> Result<()> {
    if !yes {
        return Err(anyhow!(
            "refusing to delete without --yes (scripted calls must pass the flag)"
        ));
    }
    let data = post(base, "/api/memory/delete", &json!({ "id": id })).await?;
    match format {
        OutputFormat::Json => writeln_ndjson(&data),
        OutputFormat::Text => {
            let removed = data.get("removed").and_then(|v| v.as_bool()).unwrap_or(false);
            println!("{} {}", if removed { "deleted" } else { "not found" }, id);
            Ok(())
        }
    }
}

pub(crate) async fn forget(base: &str, args: ForgetArgs, format: OutputFormat) -> Result<()> {
    if !args.yes {
        return Err(anyhow!(
            "refusing to forget without --yes (bulk delete requires explicit confirmation)"
        ));
    }
    let body = filter_body(&args.filters);
    let data = post(base, "/api/memory/forget", &body).await?;
    match format {
        OutputFormat::Json => writeln_ndjson(&data),
        OutputFormat::Text => {
            let removed = data.get("removed").and_then(|v| v.as_u64()).unwrap_or(0);
            println!("forgot {removed} fact(s)");
            Ok(())
        }
    }
}

// ── Body builders ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_add_body(
    content: String,
    r#type: CliMemoryType,
    contexts: Vec<String>,
    tags: Vec<String>,
    from: CliOrigin,
    outcome: Option<CliOutcome>,
    cwd: Option<String>,
    occurred_at: Option<DateTime<Utc>>,
    source_session: Option<String>,
    skip_dedup: bool,
    host: Option<String>,
) -> Value {
    let mut body = json!({
        "content": content,
        "type": cli_memory_type_str(r#type),
        "from": cli_origin_str(from),
        "skip_dedup": skip_dedup,
    });
    if !contexts.is_empty() {
        body["contexts"] = json!(contexts);
    }
    if !tags.is_empty() {
        body["tags"] = json!(tags);
    }
    if let Some(o) = outcome {
        body["outcome"] = Value::String(cli_outcome_str(o).to_string());
    }
    if let Some(c) = cwd {
        body["cwd"] = Value::String(c);
    }
    if let Some(t) = occurred_at {
        body["occurred_at"] = Value::String(t.to_rfc3339());
    }
    if let Some(s) = source_session {
        body["source_session"] = Value::String(s);
    }
    if let Some(h) = host {
        body["host"] = Value::String(h);
    }
    body
}

fn filter_body(filters: &FilterArgs) -> Value {
    let mut body = json!({});
    if !filters.contexts.is_empty() {
        body["contexts"] = json!(filters.contexts);
    }
    // Daemon's FilterDTO accepts a single `type`. Multiple-type CLI flags
    // collapse to the first value (matches what direct mode does today;
    // server-side filters are inclusive on type).
    if let Some(t) = filters.types.first().copied() {
        body["type"] = Value::String(cli_memory_type_str(t).to_string());
    }
    if let Some(o) = filters.from {
        body["from"] = Value::String(cli_origin_str(o).to_string());
    }
    if let Some(o) = filters.outcome {
        body["outcome"] = Value::String(cli_outcome_str(o).to_string());
    }
    if let Some(t) = filters.since {
        body["since"] = Value::String(t.to_rfc3339());
    }
    // Fold `--older-than 30d` into `until` on the wire (the daemon
    // doesn't know about `older_than`). When both are present, pick
    // the stricter (older) cutoff — same rule as `FilterArgs::into_filters`.
    let until = match (filters.until, filters.older_than) {
        (Some(a), Some(b)) => Some(if a < b { a } else { b }),
        (a, b) => a.or(b),
    };
    if let Some(t) = until {
        body["until"] = Value::String(t.to_rfc3339());
    }
    // Tier was missing here for the entire daemon path — direct-store mode
    // forwarded it via `FilterArgs::into_filters()`, but with the daemon up
    // (the common case) the filter was silently dropped. Effect: `--tier
    // core` returned every row, and the engine's "Core facts" block was
    // really "all rows up to limit". Fixing here, not at the daemon's
    // FilterDTO, because the DTO already understood tier — only the CLI
    // shipper omitted it.
    if let Some(t) = filters.tier {
        body["tier"] = Value::String(cli_tier_str(t).to_string());
    }
    body
}

// ── Emission ────────────────────────────────────────────────────────────────

fn emit_add_outcome(data: &Value, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => writeln_ndjson(data),
        OutputFormat::Text => {
            let action = data
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("added");
            let fact = data.get("fact").unwrap_or(&Value::Null);
            let id = fact.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let content = fact.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let truncated = truncate(content, 80);
            match action {
                "merged" => {
                    let prev = data
                        .get("previous_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let sim = data
                        .get("similarity")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    println!(
                        "merged into {id} (similarity {sim:.2}, previous_id {prev}) — {truncated}"
                    );
                }
                _ => println!("added {id} — {truncated}"),
            }
            Ok(())
        }
    }
}

fn emit_fact_value(data: &Value, format: OutputFormat) -> Result<()> {
    let fact: Memory = serde_json::from_value(data.clone())
        .context("parsing fact from daemon response")?;
    super::emit_fact(&fact, format)
}

fn emit_fact_array(data: &Value, format: OutputFormat) -> Result<()> {
    let arr = data
        .as_array()
        .ok_or_else(|| anyhow!("expected array, got {data}"))?;
    let facts: Vec<Memory> = arr
        .iter()
        .map(|v| serde_json::from_value::<Memory>(v.clone()))
        .collect::<Result<Vec<_>, _>>()
        .context("parsing facts from daemon response")?;
    super::emit_facts(&facts, format)
}

/// Emit a search response — each row carries a `score` field which we
/// strip before deserializing the Memory, then attach back when printing.
/// JSON output: re-add `score`. Text output: prefix with `0.NN`.
fn emit_scored_fact_array(data: &Value, format: OutputFormat) -> Result<()> {
    let arr = data
        .as_array()
        .ok_or_else(|| anyhow!("expected array, got {data}"))?;
    let mut scored: Vec<(Memory, f32)> = Vec::with_capacity(arr.len());
    for v in arr {
        let mut row = v.clone();
        let score = row
            .as_object_mut()
            .and_then(|o| o.remove("score"))
            .and_then(|s| s.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.0);
        let fact: Memory = serde_json::from_value(row)
            .context("parsing fact from daemon response")?;
        scored.push((fact, score));
    }
    super::emit_scored_facts(&scored, format)
}

fn writeln_ndjson<T: serde::Serialize>(value: &T) -> Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, value).context("writing JSON to stdout")?;
    lock.write_all(b"\n").context("writing newline to stdout")?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let prefix: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{prefix}…")
    }
}

// ── Enum string mapping ─────────────────────────────────────────────────────
//
// Server-side DTOs accept lowercase variant names for MemoryType / Origin /
// Outcome (via serde's `rename_all = "lowercase"`). Mapping CLI enums to
// strings here keeps the request bodies small and round-trippable.

fn cli_memory_type_str(t: CliMemoryType) -> &'static str {
    match t {
        CliMemoryType::Fact => "fact",
        CliMemoryType::Preference => "preference",
        CliMemoryType::Decision => "decision",
        CliMemoryType::Tried => "tried",
        CliMemoryType::Fixed => "fixed",
        CliMemoryType::Learned => "learned",
        CliMemoryType::Built => "built",
    }
}

fn cli_origin_str(o: CliOrigin) -> &'static str {
    match o {
        CliOrigin::User => "user",
        CliOrigin::Agent => "agent",
        CliOrigin::Derived => "derived",
    }
}

fn cli_outcome_str(o: CliOutcome) -> &'static str {
    match o {
        CliOutcome::Positive => "positive",
        CliOutcome::Negative => "negative",
        CliOutcome::Neutral => "neutral",
    }
}

fn cli_tier_str(t: crate::cli::CliTier) -> &'static str {
    match t {
        crate::cli::CliTier::Core => "core",
        crate::cli::CliTier::Semantic => "semantic",
        crate::cli::CliTier::Episodic => "episodic",
    }
}
