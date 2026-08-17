//! `ling-mem` CLI — clap-parsed subcommands, dispatched to [`MemoryStore`].
//!
//! The contract is documented in `doc/tech-spec.md`:
//! - **Default output**: NDJSON on stdout, one fact per line for list-like
//!   results; single JSON object for single-row results.
//! - **Errors**: JSON on stderr, non-zero exit (`{"error": "...", "code": ""}`).
//! - **Human format**: `--format text` for friendly text, `--format json`
//!   (default) for NDJSON.
//! - **Data dir**: `--data-dir`, then `$LINGGEN_DATA_DIR`, then `~/.linggen/`.
//! - **Search**: takes a plain text query; the query is embedded on the fly
//!   via [`crate::embed::Embedder`] (Qwen3-Embedding-0.6B) and filtered against the
//!   LanceDB vector column.
//! - **Add**: auto-embeds content for any inserted fact that doesn't already
//!   carry a vector, so the row is immediately searchable.

use crate::memory::{
    Filters, MemoryPatch, MemoryStore, MemoryType, Origin, Outcome, Recall, SortOrder, Tier,
};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

mod client;

/// Top-level CLI. All subcommands share the flags declared here (data dir,
/// output format, quiet).
#[derive(Debug, Parser)]
#[command(name = "ling-mem", version, about = "Semantic memory store")]
pub struct Cli {
    /// Override the data directory. Falls back to `$LINGGEN_DATA_DIR`, then
    /// `~/.linggen/`. LanceDB lives under `<data-dir>/memory/memory.lancedb/`.
    #[arg(long, global = true, env = "LINGGEN_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Json)]
    pub format: OutputFormat,

    /// Suppress non-essential progress output.
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Target the episodic store (the staging table) instead of the
    /// curated `semantic` table. Episodic rows are the raw, undeduped pool the
    /// consolidation pass later promotes or evicts. Bypasses the HTTP
    /// daemon — episodic is a direct-store concern only.
    #[arg(long, global = true)]
    pub episodic: bool,

    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Json,
    Text,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Insert one or many facts. Positional text becomes one fact; supply
    /// `--stdin` to read newline-delimited JSON facts instead.
    Add(AddArgs),

    /// Fetch one fact by id.
    Get { id: String },

    /// Semantic search — vector query + metadata filters.
    Search(SearchArgs),

    /// Non-semantic browse — metadata filters only.
    List(ListArgs),

    /// Modify fields of an existing fact. Aliased as `update` for back-
    /// compat with pre-rename scripts; new callers should prefer `edit`,
    /// which doesn't collide with the more conventional binary-update sense
    /// of the word.
    #[command(alias = "update")]
    Edit(UpdateArgs),

    /// Hard-delete a fact by id.
    Delete {
        id: String,
        /// Skip confirmation (required for scripted use).
        #[arg(long)]
        yes: bool,
    },

    /// Bulk-delete by filter. Refuses empty filters. Accepts the same
    /// filter args as `list`/`search`, including `--older-than 30d` and
    /// the global `--episodic` flag, so `ling-mem --episodic forget
    /// --older-than 30d --yes` covers the past-TTL decay sweep that
    /// used to be a separate `evict` verb.
    Forget(ForgetArgs),

    /// Per-day dream-state rollup: each day's episodic row counts +
    /// per-verb flags (`scanned`, `dreamed`), plus `first_unscanned` /
    /// `first_undreamed` and past-day summary counts (`total_days`,
    /// `scanned_days`, `dreamed_days`). `--undreamed` narrows to days
    /// awaiting a dream pass, oldest first — the dream worklist.
    /// Requires the daemon.
    Days(DaysArgs),

    /// Stamp a day as remembered after a remember pass judged its rows.
    /// Called by the agent that ran the pass (Linggen mission / skill
    /// page / CC host agent). Requires the daemon.
    RememberDay(RememberDayArgs),

    /// Stamp a day scanned (`harvested_at` only): a scan pass covered
    /// this day's session logs. Does not touch `remembered_at` — the
    /// staged rows go pending and a dream pass judges them. Requires
    /// the daemon.
    HarvestDay {
        /// Local calendar day, `YYYY-MM-DD`.
        date: String,
    },

    /// Forget sweep — the dream pipeline's mechanical third stage: evict
    /// episodic rows that are past TTL, belong to a remembered day, and
    /// were judged (created before the day's `remembered_at`). Never
    /// touches un-judged rows. Requires the daemon.
    Sweep {
        /// Report what would be evicted without deleting anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Store-state summary: per-tier row counts, disk footprint, last
    /// dream, TTL, schema + embedding model. The same numbers the
    /// console and the memory app render. Requires the daemon.
    Stats,

    /// Condense scan — mechanical, read-only detection of stale
    /// same-subject chains in long-term memory. `--kind cited` (default)
    /// groups rows that cite another row's id verbatim; `--kind marker`
    /// lists rows with provisional-state language plus their nearest
    /// neighbors for LLM confirmation. Judgment and merges belong to the
    /// caller (the condense mission / host agent). Requires the daemon.
    Chains(ChainsArgs),

    /// Review queue — items the dream audit could not solve with
    /// confidence (uncertain merges, stale status claims, user-voice
    /// contradictions). Listing is read-only; solving happens in a host
    /// agent (`/linggen:solve`). Requires the daemon.
    Issues(IssuesArgs),

    /// Queue one review item (the audit's holding pen). Idempotent per
    /// (kind, row_ids) — re-queueing an unfixed suspect returns the
    /// existing item. Requires the daemon.
    IssueAdd(IssueAddArgs),

    /// Close one review-queue item by id after solving (or dismissing)
    /// it. Requires the daemon.
    IssueResolve(IssueResolveArgs),

    // Session-scanning utilities (`collect` + `extract`) used to live here.
    // They moved to `skills/memory/scripts/` as bash helpers — the daemon is
    // a pure data service; reading session files isn't its concern.

    /// Spawn the daemon. Forks to background by default and waits for
    /// the port to bind; pass `--foreground` to block in the current
    /// process (what `launchd` / `systemd` / docker want).
    ///
    /// Aliased as `serve` for back-compat with earlier scripts/launchd
    /// configs — `serve` implies `--foreground`.
    #[command(hide = true)]
    Start {
        #[arg(long, default_value_t = crate::daemon::DEFAULT_PORT)]
        port: u16,
        /// Bind address. Loopback by default — the ordinary case is this
        /// machine's own agents. Widen it (e.g. `0.0.0.0`) only to share ONE
        /// store with a second machine on the LAN, which requires a paired
        /// device or the daemon refuses to start.
        #[arg(long, default_value = "127.0.0.1")]
        host: std::net::IpAddr,
        /// Block in the current process instead of forking. Use this
        /// when something else (launchd, systemd, docker, the user's
        /// terminal) is the supervising parent.
        #[arg(long)]
        foreground: bool,
    },

    /// Back-compat alias for `start --foreground`. Hidden from --help;
    /// kept callable so existing launchd / systemd / supervisor configs
    /// invoking `ling-mem serve` keep working unchanged.
    #[command(hide = true)]
    Serve {
        #[arg(long, default_value_t = crate::daemon::DEFAULT_PORT)]
        port: u16,
        /// Bind address. Loopback by default — the ordinary case is this
        /// machine's own agents. Widen it (e.g. `0.0.0.0`) only to share ONE
        /// store with a second machine on the LAN, which requires a paired
        /// device or the daemon refuses to start.
        #[arg(long, default_value = "127.0.0.1")]
        host: std::net::IpAddr,
    },

    /// SIGTERM the running daemon and wait for it to exit. Hidden —
    /// the CLI manages daemon lifecycle transparently; explicit stop
    /// is power-user / installer territory.
    #[command(hide = true)]
    Stop,

    /// Stop + start. Hidden for the same reason as `stop`.
    #[command(hide = true)]
    Restart {
        #[arg(long, default_value_t = crate::daemon::DEFAULT_PORT)]
        port: u16,
        /// Bind address. Loopback by default — the ordinary case is this
        /// machine's own agents. Widen it (e.g. `0.0.0.0`) only to share ONE
        /// store with a second machine on the LAN, which requires a paired
        /// device or the daemon refuses to start.
        #[arg(long, default_value = "127.0.0.1")]
        host: std::net::IpAddr,
    },

    /// Report daemon state: pidfile, liveness, health probe. Also surfaces
    /// the most recent (cached) upgrade probe so callers know whether a
    /// newer binary is available without making any network call.
    Status,

    /// Check for or apply a `ling-mem` upgrade from GitHub releases.
    /// With `--check`, report the latest version without downloading.
    /// Otherwise download, verify, swap the binary, and restart the
    /// daemon if it was running. Aliased as `self-update` for back-compat.
    #[command(alias = "self-update")]
    Upgrade {
        /// Print version info only; don't download or swap.
        #[arg(long)]
        check: bool,

        /// Reinstall even if already on the latest version.
        #[arg(long)]
        force: bool,

        /// Confirm the swap. Required for the actual upgrade path —
        /// scripted/agent callers must pass it explicitly.
        #[arg(long)]
        yes: bool,

        /// Daemon port to use when restarting after the swap.
        #[arg(long, default_value_t = crate::daemon::DEFAULT_PORT)]
        port: u16,
    },

    /// Dump every fact as newline-delimited JSON (one object per line),
    /// schema-agnostic, embeddings omitted. Pass `-` for stdout. With the
    /// global `--episodic` flag, exports the staging table. The escape hatch
    /// for a major store-schema break: export → reset → import.
    Export {
        /// Output file path, or `-` for stdout.
        #[arg(default_value = "-")]
        file: String,
    },

    /// Load facts from a newline-delimited JSON file produced by `export`
    /// (or any NDJSON of fact objects). Re-embeds `content` and inserts
    /// without dedup. Pass `-` for stdin. Honors the global `--episodic` flag.
    Import {
        /// Input file path, or `-` for stdin.
        #[arg(default_value = "-")]
        file: String,
    },
}

// ── Argument structs ────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct AddArgs {
    /// The fact text. Omit when using `--stdin`.
    pub content: Option<String>,

    #[arg(long, value_enum, default_value_t = CliMemoryType::Fact)]
    pub r#type: CliMemoryType,

    /// Storage tier. `core` is the small always-loaded identity/preference
    /// set; `semantic` is the broader RAG-retrieved pool.
    #[arg(long, value_enum, default_value_t = CliTier::Semantic)]
    pub tier: CliTier,

    #[arg(long = "context", value_name = "CONTEXT")]
    pub contexts: Vec<String>,

    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,

    #[arg(long, value_enum, default_value_t = CliOrigin::Derived)]
    pub from: CliOrigin,

    #[arg(long, value_enum)]
    pub outcome: Option<CliOutcome>,

    #[arg(long)]
    pub cwd: Option<String>,

    /// RFC-3339 timestamp for when the described thing occurred.
    #[arg(long)]
    pub occurred_at: Option<DateTime<Utc>>,

    #[arg(long)]
    pub source_session: Option<String>,

    /// Read newline-delimited JSON facts from stdin, one per line.
    #[arg(long)]
    pub stdin: bool,

    /// Insert as a new row even if a near-duplicate exists. Bulk stdin
    /// imports always skip dedup regardless of this flag.
    #[arg(long)]
    pub skip_dedup: bool,

    /// Writing host identifier (`claude-code`, `codex`, `openclaw`,
    /// `linggen`). When omitted, auto-detected from common host env vars
    /// (`CLAUDECODE`, `CODEX_HOME`, `OPENCLAW_HOME`, `LINGGEN_AGENT_SESSION`);
    /// `$LING_MEM_HOST` overrides detection. Stored on the row for
    /// cross-host provenance in the dashboard.
    #[arg(long, env = "LING_MEM_HOST")]
    pub host: Option<String>,

    /// Row ids this new row replaces — the merge/digest verb. Atomic on
    /// the daemon: the survivor is inserted and every listed semantic
    /// loser is ARCHIVED (`expired_at` + `superseded_by`, recoverable via
    /// `list --superseded-by`); episodic losers are deleted. Replaces the
    /// old two-step add-then-delete, which hard-deleted what should have
    /// been archived. User-voice losers additionally need
    /// `--user-directed`.
    #[arg(long = "replace", value_name = "ID")]
    pub replace_ids: Vec<String>,

    /// Assert the user directed this change (required when --replace
    /// targets rows in the user's voice; see the merge law).
    #[arg(long)]
    pub user_directed: bool,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Text query — embedded on the fly via Qwen3-Embedding-0.6B (1024-dim).
    /// First-run downloads the model weights (~1.2 GB BF16); subsequent
    /// calls load from cache.
    pub query: String,

    #[command(flatten)]
    pub filters: FilterArgs,

    #[arg(long, default_value_t = 10)]
    pub limit: usize,

    /// Drop rows whose cosine similarity to the query falls below this
    /// threshold. Range `[-1.0, 1.0]`; in practice Qwen3-Embedding-0.6B
    /// scores land in `[0.0, 1.0]`. Try 0.3 to drop noise; omit to disable.
    #[arg(long)]
    pub min_score: Option<f32>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    #[command(flatten)]
    pub filters: FilterArgs,

    #[arg(long, value_enum, default_value_t = CliSort::Newest)]
    pub sort: CliSort,

    #[arg(long, default_value_t = 50)]
    pub limit: usize,

    /// Skip this many rows in sort order. Pairs with `--limit` to page
    /// through results larger than one batch.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

/// Parse a human duration ("30d", "12h") into a `DateTime<Utc>` cutoff —
/// i.e. `now - duration`. The result is the absolute timestamp callers
/// pass to `--until` / `filters.until`. Returning a timestamp (not a
/// `Duration`) keeps `cmd_list` / `client::list` purely a "set the
/// filter" path, no further math.
fn parse_duration_to_cutoff(s: &str) -> Result<DateTime<Utc>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("duration must not be empty (e.g. 30d, 12h, 1w)".to_string());
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let n: i64 = num_str
        .parse()
        .map_err(|_| format!("could not parse number from '{s}' (expected like '30d')"))?;
    if n < 0 {
        return Err(format!("duration must be non-negative ('{s}')"));
    }
    let seconds = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 60 * 60,
        "d" => n * 24 * 60 * 60,
        "w" => n * 7 * 24 * 60 * 60,
        _ => {
            return Err(format!(
                "unknown duration unit '{unit}' in '{s}' (use s|m|h|d|w)"
            ))
        }
    };
    let cutoff = Utc::now() - chrono::Duration::seconds(seconds);
    Ok(cutoff)
}

#[derive(Debug, Args, Default, Clone)]
pub struct FilterArgs {
    #[arg(long = "context", value_name = "CONTEXT")]
    pub contexts: Vec<String>,

    #[arg(long = "type", value_enum, value_name = "TYPE")]
    pub types: Vec<CliMemoryType>,

    #[arg(long, value_enum)]
    pub tier: Option<CliTier>,

    #[arg(long, value_enum)]
    pub from: Option<CliOrigin>,

    #[arg(long, value_enum)]
    pub outcome: Option<CliOutcome>,

    /// RFC-3339 lower bound on `COALESCE(occurred_at, created_at)`.
    #[arg(long)]
    pub since: Option<DateTime<Utc>>,

    /// RFC-3339 upper bound on `COALESCE(occurred_at, created_at)`.
    #[arg(long)]
    pub until: Option<DateTime<Utc>>,

    /// Sugar over `--until`: select rows older than this duration from
    /// now. Accepts `<n><unit>` where unit is one of s/m/h/d/w
    /// (seconds/minutes/hours/days/weeks). Examples: `30d`, `12h`,
    /// `1w`. Available on `list`, `search`, and `forget` — the dream
    /// worklist + past-TTL eviction both use it instead of computing
    /// dates in shell. When both `--until` and `--older-than` are
    /// passed, the stricter (older) cutoff wins.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration_to_cutoff)]
    pub older_than: Option<DateTime<Utc>>,

    /// One local calendar day, `YYYY-MM-DD` — sugar over
    /// `--since`/`--until` covering exactly that day. The remember stage
    /// lists a single day's worklist with this. Explicit bounds win.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub day: Option<String>,

    /// Scope to the work at this path: rows written under it, plus every
    /// row that belongs to no project. Paths nest, so a parent directory
    /// covers the repos inside it. `forget` ignores this on its own — it
    /// matches unscoped rows by design and would take most of the store.
    #[arg(long = "cwd-scope", visible_alias = "project", value_name = "PATH")]
    pub cwd_scope: Option<String>,

    /// Include archived rows — losers a `replace_ids` merge or digest
    /// expired out of live memory. Off by default: the archive is for
    /// provenance and unpack, not recall.
    #[arg(long = "include-expired")]
    pub include_expired: bool,

    /// Unpack a merge/digest: only the archived rows this survivor id
    /// replaced. Implies --include-expired.
    #[arg(long = "superseded-by", value_name = "ID")]
    pub superseded_by: Option<String>,
}

#[derive(Debug, Args)]
pub struct DaysArgs {
    /// Only days awaiting a dream pass, oldest first (the worklist).
    /// `--pending` is the pre-flags spelling, kept as an alias.
    #[arg(long, alias = "pending")]
    pub undreamed: bool,

    /// Inclusive lower bound, `YYYY-MM-DD`.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub from: Option<String>,

    /// Inclusive upper bound, `YYYY-MM-DD`.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub to: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct ChainsArgs {
    /// `cited` = id-citation chains (auto-accept quality); `marker` =
    /// provisional-state candidates needing confirmation; `subject` =
    /// same-subject vector clusters for the v2 digest pass.
    #[arg(long, default_value = "cited", value_parser = ["cited", "marker", "subject"])]
    pub kind: String,

    /// Clusters per page.
    #[arg(long, default_value_t = 10)]
    pub limit: usize,

    /// Pagination offset over the cluster list.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Only clusters mergeable unattended — every row an agent note
    /// (`from=derived`, `tier=semantic`). What the condense mission uses.
    #[arg(long)]
    pub derived_only: bool,
}

#[derive(Debug, Args)]
pub struct IssuesArgs {
    /// Which items to list.
    #[arg(long, default_value = "open", value_parser = ["open", "resolved", "dismissed", "all"])]
    pub status: String,

    /// Max items.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

#[derive(Debug, Args)]
pub struct IssueAddArgs {
    /// What the audit saw: `chain` (uncertain merge candidate),
    /// `stale-status` (claim likely overtaken by the world), or
    /// `contradiction` (conflicting rows needing the user's pick).
    #[arg(long, value_parser = ["chain", "stale-status", "contradiction"])]
    pub kind: String,

    /// Memory row id(s) the item is about (repeatable).
    #[arg(long = "row", value_name = "ROW_ID")]
    pub row_ids: Vec<String>,

    /// What a solver should check — the item's whole context.
    pub note: String,
}

#[derive(Debug, Args)]
pub struct IssueResolveArgs {
    /// The issue id (from `ling-mem issues`).
    pub id: String,

    /// `resolved` (fixed in the store) or `dismissed` (not worth fixing).
    #[arg(long, default_value = "resolved", value_parser = ["resolved", "dismissed"])]
    pub outcome: String,

    /// One-line record of what was done.
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Debug, Args)]
pub struct RememberDayArgs {
    /// Local calendar day that was judged, `YYYY-MM-DD`.
    pub date: String,

    /// Rows judged in this pass (accumulates onto the day's total).
    #[arg(long, default_value_t = 0)]
    pub judged: u32,

    /// Rows promoted to semantic in this pass (accumulates).
    #[arg(long, default_value_t = 0)]
    pub promoted: u32,

    /// Also stamp `harvested_at` (a harvest pass covered this day).
    #[arg(long)]
    pub harvested: bool,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    pub id: String,

    #[arg(long)]
    pub content: Option<String>,

    #[arg(long = "context", value_name = "CONTEXT")]
    pub contexts: Option<Vec<String>>,

    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Option<Vec<String>>,

    #[arg(long, value_enum)]
    pub r#type: Option<CliMemoryType>,

    /// Repair the row's `tier` field — useful when the value drifted
    /// from the row's table identity (e.g. an episodic-table row
    /// stuck on `tier=semantic`). Does NOT move the row across
    /// tables; use `add --episodic` (or `delete` + `add`) for that.
    #[arg(long, value_enum)]
    pub tier: Option<CliTier>,

    #[arg(long, value_enum)]
    pub from: Option<CliOrigin>,

    #[arg(long, value_enum)]
    pub outcome: Option<CliOutcome>,

    /// Clear the outcome (set to null). Ignored if `--outcome` is also given.
    #[arg(long)]
    pub clear_outcome: bool,

    #[arg(long)]
    pub cwd: Option<String>,

    #[arg(long)]
    pub clear_cwd: bool,

    /// Assert the user directed this change (their current message
    /// states it as settled, or they just answered an ask). Required
    /// when rewriting `--content` on a `from=user` row — the daemon
    /// refuses such edits otherwise (the merge law's floor).
    #[arg(long)]
    pub user_directed: bool,
}

#[derive(Debug, Args)]
pub struct ForgetArgs {
    #[command(flatten)]
    pub filters: FilterArgs,

    /// Confirm the bulk delete. Required — enforces a think-twice step.
    #[arg(long)]
    pub yes: bool,
}

// ── CLI ↔ domain-type glue ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliMemoryType {
    Fact,
    Preference,
    Decision,
    Tried,
    Fixed,
    Learned,
    Built,
}

impl From<CliMemoryType> for MemoryType {
    fn from(v: CliMemoryType) -> MemoryType {
        match v {
            CliMemoryType::Fact => MemoryType::Fact,
            CliMemoryType::Preference => MemoryType::Preference,
            CliMemoryType::Decision => MemoryType::Decision,
            CliMemoryType::Tried => MemoryType::Tried,
            CliMemoryType::Fixed => MemoryType::Fixed,
            CliMemoryType::Learned => MemoryType::Learned,
            CliMemoryType::Built => MemoryType::Built,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliOrigin {
    User,
    Agent,
    Derived,
}

impl From<CliOrigin> for Origin {
    fn from(v: CliOrigin) -> Origin {
        match v {
            CliOrigin::User => Origin::User,
            CliOrigin::Agent => Origin::Agent,
            CliOrigin::Derived => Origin::Derived,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliOutcome {
    Positive,
    Negative,
    Neutral,
}

impl From<CliOutcome> for Outcome {
    fn from(v: CliOutcome) -> Outcome {
        match v {
            CliOutcome::Positive => Outcome::Positive,
            CliOutcome::Negative => Outcome::Negative,
            CliOutcome::Neutral => Outcome::Neutral,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliTier {
    Core,
    Semantic,
    /// Episodic is normally selected via the global `--episodic` flag
    /// (the row lives in a separate table). Exposed on `--tier` so
    /// `edit --tier episodic` can repair rows whose `tier` field
    /// drifted from their table identity. Don't pass it to `add` —
    /// use `--episodic` instead so the row lands in the right table.
    Episodic,
}

impl From<CliTier> for Tier {
    fn from(v: CliTier) -> Tier {
        match v {
            CliTier::Core => Tier::Core,
            CliTier::Semantic => Tier::Semantic,
            CliTier::Episodic => Tier::Episodic,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CliSort {
    Newest,
    Oldest,
}

impl From<CliSort> for SortOrder {
    fn from(v: CliSort) -> SortOrder {
        match v {
            CliSort::Newest => SortOrder::Newest,
            CliSort::Oldest => SortOrder::Oldest,
        }
    }
}

impl FilterArgs {
    fn into_filters(self) -> Result<Filters> {
        // `--older-than 30d` is sugar for `--until <now-30d>`. If both
        // are passed, pick the stricter (older) cutoff so the result
        // is the intersection of both predicates.
        let mut until = match (self.until, self.older_than) {
            (Some(a), Some(b)) => Some(if a < b { a } else { b }),
            (a, b) => a.or(b),
        };
        let mut since = self.since;
        // `--day` is sugar over since/until covering one local calendar
        // day. Explicit bounds win; day only fills what's unset.
        if let Some(day) = &self.day {
            let date = chrono::NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d")
                .map_err(|_| anyhow!("invalid --day {day:?}: expected YYYY-MM-DD"))?;
            let (start, end) = crate::http::days::local_day_bounds(date);
            since = since.or(Some(start));
            until = until.or(Some(end));
        }
        Ok(Filters {
            contexts: self.contexts,
            types: self.types.into_iter().map(Into::into).collect(),
            origin: self.from.map(Into::into),
            outcome: self.outcome.map(Into::into),
            since,
            until,
            tier: self.tier.map(Into::into),
            source_session: None,
            cwd_scope: self.cwd_scope,
            include_expired: self.include_expired,
            superseded_by: self.superseded_by,
        })
    }
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// Resolve the effective data directory from CLI flag / env / home default.
fn resolve_data_dir(cli_override: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = cli_override {
        return Ok(p);
    }
    let home = dirs::home_dir().context("no HOME directory available")?;
    Ok(home.join(".linggen"))
}

/// Open the curated `semantic` store or the `episodic` staging store,
/// depending on `episodic`. The single direct-store open site for data
/// ops — keeps the table-routing decision in one place.
async fn open_store(data_dir: &std::path::Path, episodic: bool) -> Result<MemoryStore> {
    if episodic {
        MemoryStore::open_episodic(data_dir).await
    } else {
        MemoryStore::open_semantic(data_dir).await
    }
}

/// Run the CLI. Dispatches to the appropriate subcommand and returns its
/// result. Errors propagate to `main` which serializes them to stderr.
pub async fn run(cli: Cli) -> Result<()> {
    let format = cli.format;

    // Lifecycle commands don't need the LanceDB store — route them
    // before opening it.
    let data_dir = resolve_data_dir(cli.data_dir)?;
    // Pin fastembed's model cache to <data_dir>/cache/fastembed so the
    // ~1.2 GB Qwen3-Embedding-0.6B weights aren't redownloaded into a `.fastembed_cache/`
    // dir every time `ling-mem` is invoked from a different CWD. fastembed
    // reads FASTEMBED_CACHE_DIR before falling back to its CWD-relative
    // default; setting it here once covers every code path that constructs
    // an Embedder.
    if std::env::var_os("FASTEMBED_CACHE_DIR").is_none() {
        let cache_dir = data_dir.join("cache").join("fastembed");
        let _ = std::fs::create_dir_all(&cache_dir);
        std::env::set_var("FASTEMBED_CACHE_DIR", &cache_dir);
    }
    let skill_dir = crate::daemon::skill_dir(&data_dir);
    match cli.cmd {
        // `start --foreground` blocks (what `serve` always did); without
        // it, forks to background and waits for the port to bind. The
        // hidden `serve` alias maps to `start --foreground` so existing
        // launchd/systemd configs keep working.
        Command::Serve { port, host } => {
            return crate::daemon::serve::run(&data_dir, &skill_dir, port, host).await
        }
        Command::Start { port, host, foreground } => {
            if foreground {
                return crate::daemon::serve::run(&data_dir, &skill_dir, port, host).await;
            }
            let outcome =
                crate::daemon::lifecycle::start(&data_dir, &skill_dir, port, host).await?;
            let update = crate::update::check_quiet(&data_dir).await;
            return emit_lifecycle_with_update(&outcome, Some(&update));
        }
        Command::Stop => {
            let outcome = crate::daemon::lifecycle::stop(&skill_dir).await?;
            return emit_lifecycle(&outcome);
        }
        Command::Restart { port, host } => {
            let outcome =
                crate::daemon::lifecycle::restart(&data_dir, &skill_dir, port, host).await?;
            let update = crate::update::check_quiet(&data_dir).await;
            return emit_lifecycle_with_update(&outcome, Some(&update));
        }
        Command::Status => {
            let mut value = crate::daemon::lifecycle::status(&skill_dir).await?;
            // Merge the cached upgrade probe (if any) so callers see
            // available-update info without `status` itself hitting the
            // network. Probes are populated by `start` / `restart` /
            // `upgrade --check` and live for 24h.
            if let (Some(info), Some(map)) = (
                crate::update::read_cached(&data_dir),
                value.as_object_mut(),
            ) {
                let mut update = serde_json::to_value(&info)?;
                if let (Some(fetched_at), Some(update_obj)) = (
                    crate::update::cache_fetched_at(&data_dir),
                    update.as_object_mut(),
                ) {
                    update_obj.insert(
                        "checked_at".to_string(),
                        serde_json::Value::Number(fetched_at.into()),
                    );
                }
                map.insert("update".to_string(), update);
            }
            // Surface store-schema compatibility so callers (autostart's
            // version reconcile, the website "store compatible?" check) can
            // see the on-disk store version vs what this binary speaks.
            if let Some(map) = value.as_object_mut() {
                use crate::memory::schema_version as sv;
                map.insert(
                    "store_schema".to_string(),
                    match sv::read_version(&data_dir) {
                        Some(v) => serde_json::json!(v),
                        None => serde_json::Value::Null,
                    },
                );
                map.insert(
                    "binary_schema".to_string(),
                    serde_json::json!({
                        "writes": sv::STORE_SCHEMA_VERSION,
                        "min_readable": sv::MIN_READABLE_SCHEMA,
                    }),
                );
            }
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }
        Command::Upgrade {
            check,
            force,
            yes,
            port,
        } => {
            return cmd_upgrade(&data_dir, &skill_dir, check, force, yes, port).await;
        }
        _ => {}
    }

    // Export/import are direct-store maintenance ops — never routed through the
    // daemon (bulk local I/O; export must read every row, import bulk-inserts).
    // Handled here so they bypass both the daemon branch and the store match.
    match &cli.cmd {
        Command::Export { .. } | Command::Import { .. } => {
            let store = open_store(&data_dir, cli.episodic)
                .await
                .with_context(|| format!("opening store at {}", data_dir.display()))?;
            return match cli.cmd {
                Command::Export { file } => cmd_export(&store, &file, format).await,
                Command::Import { file } => cmd_import(&store, &file, format, cli.episodic).await,
                _ => unreachable!("matched Export/Import above"),
            };
        }
        _ => {}
    }

    // Prefer the running daemon when one is reachable. This eliminates the
    // CLI/daemon data-path split: with the daemon up, the CLI is a typed
    // shell client over HTTP and there's exactly one writer to the store.
    // If no daemon is running, autostart one (matches Linggen engine
    // semantics in `engine::capability_tools::dispatch`). Direct
    // `MemoryStore` mode below remains a fallback for when autostart fails
    // (e.g. port in use, binary missing).
    //
    // `--episodic` always uses the direct store: the HTTP surface only
    // exposes the curated `semantic` table, so routing episodic ops through
    // the daemon would silently hit the wrong table.
    if !cli.episodic {
        if let Some(base_url) = client::try_running_or_start(&data_dir, &skill_dir).await {
            return match cli.cmd {
                Command::Add(args) => client::add(&base_url, args, format).await,
                Command::Get { id } => client::get(&base_url, &id, format).await,
                Command::Search(args) => client::search(&base_url, args, format).await,
                Command::List(args) => client::list(&base_url, args, format).await,
                Command::Edit(args) => client::update(&base_url, args, format).await,
                Command::Delete { id, yes } => {
                    client::delete(&base_url, &id, yes, format).await
                }
                Command::Forget(args) => client::forget(&base_url, args, format).await,
                Command::Days(args) => client::days(&base_url, args, format).await,
                Command::RememberDay(args) => {
                    client::remember_day(&base_url, args, format).await
                }
                Command::Sweep { dry_run } => client::sweep(&base_url, dry_run, format).await,
                Command::HarvestDay { date } => {
                    client::harvest_day(&base_url, &date, format).await
                }
                Command::Stats => client::stats(&base_url, format).await,
                Command::Chains(args) => client::chains(&base_url, args, format).await,
                Command::Issues(args) => client::issues(&base_url, args, format).await,
                Command::IssueAdd(args) => client::issue_add(&base_url, args, format).await,
                Command::IssueResolve(args) => {
                    client::issue_resolve(&base_url, args, format).await
                }
                Command::Serve { .. }
                | Command::Start { .. }
                | Command::Stop
                | Command::Restart { .. }
                | Command::Status
                | Command::Upgrade { .. }
                | Command::Export { .. }
                | Command::Import { .. } => unreachable!("handled above"),
            };
        }
    }

    let store = open_store(&data_dir, cli.episodic)
        .await
        .with_context(|| format!("opening store at {}", data_dir.display()))?;

    match cli.cmd {
        Command::Add(args) => cmd_add(&store, args, format, cli.episodic).await,
        Command::Get { id } => cmd_get(&store, &id, format).await,
        // Default search spans both tables (recall). `store` is the semantic
        // handle here (`!cli.episodic`); reuse it and open episodic alongside.
        Command::Search(args) if !cli.episodic => {
            let episodic = MemoryStore::open_episodic(&data_dir)
                .await
                .with_context(|| format!("opening episodic store at {}", data_dir.display()))?;
            let recall = Recall::new(Arc::new(store), Arc::new(episodic));
            cmd_search_recall(&recall, args, format).await
        }
        // `--episodic`: single-table escape hatch (consolidator / debugging).
        Command::Search(args) => cmd_search(&store, args, format).await,
        Command::List(args) => cmd_list(&store, args, format).await,
        Command::Edit(args) => cmd_update(&store, args, format).await,
        Command::Delete { id, yes } => cmd_delete(&store, &id, yes, format).await,
        Command::Forget(args) => cmd_forget(&store, args, format).await,
        // Dream-state ops live behind the daemon (it owns `.days.json`);
        // there is deliberately no direct-store fallback — two writers to
        // the sidecar would race.
        Command::Days(_)
        | Command::RememberDay(_)
        | Command::HarvestDay { .. }
        | Command::Sweep { .. }
        | Command::Stats
        | Command::Chains(_)
        | Command::Issues(_)
        | Command::IssueAdd(_)
        | Command::IssueResolve(_) => Err(anyhow!(
            "this command requires the daemon — start it with `ling-mem start`"
        )),
        Command::Serve { .. }
        | Command::Start { .. }
        | Command::Stop
        | Command::Restart { .. }
        | Command::Status
        | Command::Upgrade { .. }
        | Command::Export { .. }
        | Command::Import { .. } => unreachable!("handled above"),
    }
}

fn emit_lifecycle(outcome: &crate::daemon::lifecycle::LifecycleOutcome) -> Result<()> {
    emit_lifecycle_with_update(outcome, None)
}

fn emit_lifecycle_with_update(
    outcome: &crate::daemon::lifecycle::LifecycleOutcome,
    update: Option<&crate::update::UpdateInfo>,
) -> Result<()> {
    let value = outcome.to_json_with_update(update);
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

// ── Subcommand handlers ─────────────────────────────────────────────────────

/// Heuristic host detection from process env vars. Each host sets a
/// distinctive variable when it launches a shell command, so a tool
/// invoked via `Bash` (`ling-mem add ...`) can infer who is asking.
/// Returns `None` when nothing matches — the row stays host=null
/// rather than getting a misleading label.
pub(crate) fn detect_host() -> Option<String> {
    use std::env;
    if env::var_os("CLAUDECODE").is_some() {
        return Some("claude-code".into());
    }
    if env::var_os("CODEX_HOME").is_some() {
        return Some("codex".into());
    }
    if env::var_os("OPENCLAW_HOME").is_some() {
        return Some("openclaw".into());
    }
    if env::var_os("LINGGEN_AGENT_SESSION").is_some() {
        return Some("linggen".into());
    }
    None
}

async fn cmd_add(
    store: &MemoryStore,
    args: AddArgs,
    format: OutputFormat,
    episodic: bool,
) -> Result<()> {
    // Bulk stdin path: always plain insert. Dedup against hundreds of incoming
    // rows would be O(N) searches per call; callers who want dedup for bulk
    // imports should run `analyze-clean` afterwards.
    if args.stdin {
        // A stdin row that carries its own `tier` keeps it; rows without one
        // inherit the `--tier` flag (default `semantic`). `read_stdin_facts`
        // reports which rows omitted the key so we only override those.
        // Episodic-store writes force `tier=Episodic` regardless — the row's
        // table is the source of truth and `tier` must agree.
        let (mut facts, tier_absent) = read_stdin_facts()?;
        let default_tier: Tier = if episodic { Tier::Episodic } else { args.tier.into() };
        for (i, f) in facts.iter_mut().enumerate() {
            if tier_absent[i] || episodic {
                f.tier = default_tier;
            }
        }
        embed_missing(&mut facts)?;
        store.insert(&facts).await?;
        return emit_added(&facts, format);
    }

    let content = args
        .content
        .clone()
        .ok_or_else(|| anyhow!("add: provide content or use --stdin"))?;

    let mut fact = crate::memory::Memory::new(content, args.r#type.into(), args.from.into());
    fact.contexts = args.contexts;
    fact.tags = args.tags;
    // Episodic-store writes pin `tier=Episodic` regardless of `--tier`
    // (the row's table is the source of truth — mirrors the HTTP add
    // path so the dashboard can derive its badge from `tier` alone).
    fact.tier = if episodic { Tier::Episodic } else { args.tier.into() };
    fact.outcome = args.outcome.map(Into::into);
    fact.cwd = args.cwd;
    fact.occurred_at = args.occurred_at;
    fact.source_session = args.source_session;
    fact.host = args.host.or_else(detect_host);

    let mut batch = [fact];
    embed_missing(&mut batch)?;
    let [fact] = batch;

    if args.skip_dedup {
        store.insert(std::slice::from_ref(&fact)).await?;
        return emit_added(std::slice::from_ref(&fact), format);
    }

    let outcome = store.insert_with_dedup(fact).await?;
    emit_outcome(&outcome, format)
}

fn emit_added(facts: &[crate::memory::Memory], format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            for f in facts {
                writeln_ndjson(f)?;
            }
        }
        OutputFormat::Text => {
            for f in facts {
                println!("added {} — {}", f.id, truncate(&f.content, 80));
            }
        }
    }
    Ok(())
}

fn emit_outcome(
    outcome: &crate::memory::InsertOutcome,
    format: OutputFormat,
) -> Result<()> {
    use crate::memory::InsertOutcome;
    match (format, outcome) {
        (OutputFormat::Json, InsertOutcome::Added(f)) => writeln_ndjson(f),
        (OutputFormat::Json, InsertOutcome::Merged { fact, .. }) => writeln_ndjson(fact),
        (OutputFormat::Text, InsertOutcome::Added(f)) => {
            println!("added {} — {}", f.id, truncate(&f.content, 80));
            Ok(())
        }
        (OutputFormat::Text, InsertOutcome::Merged {
            fact,
            similarity,
            previous_id,
        }) => {
            println!(
                "merged into {} (similarity {:.2}, previous_id {}) — {}",
                fact.id,
                similarity,
                previous_id,
                truncate(&fact.content, 80),
            );
            Ok(())
        }
    }
}

async fn cmd_get(store: &MemoryStore, id: &str, format: OutputFormat) -> Result<()> {
    match store.get(id).await? {
        Some(f) => emit_fact(&f, format),
        None => Err(not_found("no fact with that id")),
    }
}

async fn cmd_search(store: &MemoryStore, args: SearchArgs, format: OutputFormat) -> Result<()> {
    let embedder =
        crate::embed::Embedder::new().context("initializing embedder for search query")?;
    let vec = embedder.embed_query(&args.query)?;
    let results = store
        .hybrid_scored(
            &vec,
            &args.query,
            &args.filters.into_filters()?,
            args.limit,
            args.min_score,
        )
        .await?;
    emit_scored_facts(&results, format)
}

/// Default search: dual-table recall (semantic + episodic). Mirrors
/// [`cmd_search`] but routes through [`Recall`] instead of one store.
async fn cmd_search_recall(recall: &Recall, args: SearchArgs, format: OutputFormat) -> Result<()> {
    let embedder =
        crate::embed::Embedder::new().context("initializing embedder for search query")?;
    let vec = embedder.embed_query(&args.query)?;
    let results = recall
        .query(
            &vec,
            &args.query,
            &args.filters.into_filters()?,
            args.limit,
            args.min_score,
        )
        .await?;
    emit_scored_facts(&results, format)
}

/// Emit scored search hits, attaching the cosine similarity to each row.
/// JSON output adds a `score` field; text output prefixes the score so
/// users can eyeball relevance without parsing JSON.
fn emit_scored_facts(
    scored: &[(crate::memory::Memory, f32, f32)],
    format: OutputFormat,
) -> Result<()> {
    // Each hit is (memory, cosine, hybrid). JSON exposes both `score`
    // (cosine) and `hybrid_score` (the blended [0,1] relevance the rows are
    // ordered by); text leads with the raw cosine for at-a-glance similarity.
    match format {
        OutputFormat::Json => {
            for (f, cosine, hybrid) in scored {
                let mut v = serde_json::to_value(f)
                    .context("serializing fact to JSON for scored search output")?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("score".into(), serde_json::json!(cosine));
                    obj.insert("hybrid_score".into(), serde_json::json!(hybrid));
                }
                let line = serde_json::to_string(&v)
                    .context("encoding scored fact JSON")?;
                println!("{line}");
            }
        }
        OutputFormat::Text => {
            for (f, cosine, _) in scored {
                println!(
                    "{:.2} {} [{}] {}",
                    cosine,
                    f.id,
                    f.r#type,
                    truncate(&f.content, 120)
                );
            }
        }
    }
    Ok(())
}

/// Embed `content` into `vector` for every fact missing a vector but
/// carrying content. Zero cost when every fact already has a vector
/// (e.g. stdin NDJSON from a prior embedding pipeline). Batches all
/// missing-vector facts into a single model call.
fn embed_missing(facts: &mut [crate::memory::Memory]) -> Result<()> {
    let idxs: Vec<usize> = facts
        .iter()
        .enumerate()
        .filter(|(_, f)| f.vector.is_none() && !f.content.trim().is_empty())
        .map(|(i, _)| i)
        .collect();

    if idxs.is_empty() {
        return Ok(());
    }

    let embedder = crate::embed::Embedder::new()
        .context("initializing embedder to populate missing vectors")?;
    let texts: Vec<String> = idxs.iter().map(|&i| facts[i].content.clone()).collect();
    let vectors = embedder.embed_many(&texts)?;

    for (idx, vec) in idxs.into_iter().zip(vectors) {
        facts[idx].vector = Some(vec);
    }
    Ok(())
}

async fn cmd_list(store: &MemoryStore, args: ListArgs, format: OutputFormat) -> Result<()> {
    let filters = args.filters.into_filters()?;
    let results = store
        .list(&filters, args.sort.into(), args.limit, args.offset)
        .await?;
    emit_facts(&results, format)
}

async fn cmd_update(store: &MemoryStore, args: UpdateArgs, format: OutputFormat) -> Result<()> {
    let outcome_patch = match (args.outcome, args.clear_outcome) {
        (Some(o), _) => Some(Some(o.into())),
        (None, true) => Some(None),
        (None, false) => None,
    };
    let cwd_patch = match (args.cwd, args.clear_cwd) {
        (Some(v), _) => Some(Some(v)),
        (None, true) => Some(None),
        (None, false) => None,
    };

    let patch = MemoryPatch {
        content: args.content,
        contexts: args.contexts,
        tags: args.tags,
        r#type: args.r#type.map(Into::into),
        tier: args.tier.map(Into::into),
        origin: args.from.map(Into::into),
        outcome: outcome_patch,
        cwd: cwd_patch,
        ..Default::default()
    };

    match store.update(&args.id, &patch).await? {
        Some(f) => emit_fact(&f, format),
        None => Err(not_found("no fact with that id")),
    }
}

async fn cmd_delete(store: &MemoryStore, id: &str, yes: bool, format: OutputFormat) -> Result<()> {
    if !yes {
        return Err(anyhow!(
            "refusing to delete without --yes (scripted calls must pass the flag)"
        ));
    }
    let removed = store.delete(id).await?;
    match format {
        OutputFormat::Json => writeln_ndjson(&serde_json::json!({
            "id": id,
            "removed": removed,
        })),
        OutputFormat::Text => {
            println!("{} {}", if removed { "deleted" } else { "not found" }, id);
            Ok(())
        }
    }
}

async fn cmd_forget(store: &MemoryStore, args: ForgetArgs, format: OutputFormat) -> Result<()> {
    if !args.yes {
        return Err(anyhow!(
            "refusing to forget without --yes (bulk delete requires explicit confirmation)"
        ));
    }
    let filters = args.filters.into_filters()?;
    let removed = store.forget(&filters).await?;
    match format {
        OutputFormat::Json => writeln_ndjson(&serde_json::json!({ "removed": removed })),
        OutputFormat::Text => {
            println!("forgot {removed} fact(s)");
            Ok(())
        }
    }
}

// `cmd_evict` and `cmd_init` were removed in v0.7.1:
//   * `evict --before <ts>` — replaced by `forget --older-than <dur>
//     --episodic --yes`, which subsumes the past-TTL decay sweep into
//     the existing bulk-delete verb.
//   * `init` — the engine no longer reads `identity.md` / `style.md`
//     since the 2026-05-20 core-tier cutover (rows live in LanceDB
//     with `tier=core`). The data directory is auto-created on the
//     first write, so a stand-alone seed step is dead weight.
// Session-scanning utilities (`collect` / `extract`) live as bash
// scripts under `skills/shared-memory/scripts/`; the daemon stays
// focused on the memory store.

async fn cmd_upgrade(
    data_dir: &std::path::Path,
    skill_dir: &std::path::Path,
    check_only: bool,
    force: bool,
    yes: bool,
    port: u16,
) -> Result<()> {
    use crate::update;

    if check_only {
        let info = update::check(data_dir, true).await?;
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    if !yes {
        return Err(anyhow!(
            "refusing to update without --yes (the swap replaces the running binary)"
        ));
    }

    let outcome = update::apply(update::ApplyOptions {
        data_dir,
        skill_dir,
        port,
        force,
    })
    .await?;
    println!("{}", serde_json::to_string_pretty(&outcome)?);
    Ok(())
}

// ── I/O helpers ─────────────────────────────────────────────────────────────

/// Parse NDJSON memories from stdin. Returns the memories plus a parallel
/// `tier_absent` mask: `true` where the source JSON object had no `tier`
/// key, so the caller can apply the `--tier` default only to those rows
/// (a row that names its own tier wins over the flag).
fn read_stdin_facts() -> Result<(Vec<crate::memory::Memory>, Vec<bool>)> {
    read_facts(std::io::stdin().lock(), "stdin")
}

/// Parse newline-delimited JSON facts from any reader. Returns the parsed
/// facts plus a parallel `tier_absent` mask (rows that omitted the `tier`
/// key, so callers can default only those). `src` names the source for
/// error context ("stdin" or a file path).
fn read_facts<R: BufRead>(
    reader: R,
    src: &str,
) -> Result<(Vec<crate::memory::Memory>, Vec<bool>)> {
    let mut out = Vec::new();
    let mut tier_absent = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("reading {src} line {}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line)
            .with_context(|| format!("parsing JSON on {src} line {}", i + 1))?;
        let has_tier = value.get("tier").is_some();
        let fact: crate::memory::Memory = serde_json::from_value(value)
            .with_context(|| format!("parsing JSON on {src} line {}", i + 1))?;
        out.push(fact);
        tier_absent.push(!has_tier);
    }
    Ok((out, tier_absent))
}

async fn cmd_export(store: &MemoryStore, file: &str, _format: OutputFormat) -> Result<()> {
    // No filter, no limit cap: dump every row. `list` fetches all matching
    // rows before sorting, so usize::MAX returns the whole table.
    let mut facts = store
        .list(&Filters::default(), SortOrder::Newest, usize::MAX, 0)
        .await?;
    // Strip embeddings — they bloat the dump ~1024 floats/row and are
    // re-derived from `content` on import.
    for f in &mut facts {
        f.vector = None;
    }

    let mut out: Box<dyn Write> = if file == "-" {
        Box::new(std::io::stdout().lock())
    } else {
        Box::new(std::io::BufWriter::new(
            std::fs::File::create(file).with_context(|| format!("creating export file {file}"))?,
        ))
    };
    for f in &facts {
        serde_json::to_writer(&mut out, f).context("serializing fact to JSON")?;
        out.write_all(b"\n").context("writing newline")?;
    }
    out.flush().context("flushing export output")?;

    // Summary to stderr so stdout stays clean NDJSON when file == "-".
    let dest = if file == "-" { String::new() } else { format!(" to {file}") };
    eprintln!("exported {} facts{dest}", facts.len());
    Ok(())
}

async fn cmd_import(
    store: &MemoryStore,
    file: &str,
    format: OutputFormat,
    episodic: bool,
) -> Result<()> {
    let (mut facts, tier_absent) = if file == "-" {
        read_facts(std::io::stdin().lock(), "stdin")?
    } else {
        let f = std::fs::File::open(file).with_context(|| format!("opening import file {file}"))?;
        read_facts(std::io::BufReader::new(f), file)?
    };

    // Rows that omitted `tier` inherit the target table's default; an
    // episodic-store import pins `tier=Episodic` (the table is the source of
    // truth). Mirrors the `add --stdin` path.
    let default_tier: Tier = if episodic { Tier::Episodic } else { Tier::Semantic };
    for (i, f) in facts.iter_mut().enumerate() {
        if tier_absent[i] || episodic {
            f.tier = default_tier;
        }
    }

    embed_missing(&mut facts)?;
    let n = store.insert(&facts).await?;
    match format {
        OutputFormat::Json => {
            for f in &facts {
                writeln_ndjson(f)?;
            }
        }
        OutputFormat::Text => eprintln!("imported {n} facts"),
    }
    Ok(())
}

fn emit_facts(facts: &[crate::memory::Memory], format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            for f in facts {
                writeln_ndjson(f)?;
            }
        }
        OutputFormat::Text => {
            for f in facts {
                println!("{} [{}] {}", f.id, f.r#type, truncate(&f.content, 120));
            }
        }
    }
    Ok(())
}

fn emit_fact(fact: &crate::memory::Memory, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => writeln_ndjson(fact),
        OutputFormat::Text => {
            println!("id:         {}", fact.id);
            println!("type:       {}", fact.r#type);
            println!("from:       {}", fact.origin);
            if !fact.contexts.is_empty() {
                println!("contexts:   {}", fact.contexts.join(", "));
            }
            if !fact.tags.is_empty() {
                println!("tags:       {}", fact.tags.join(", "));
            }
            if let Some(o) = fact.outcome {
                println!("outcome:    {o}");
            }
            if let Some(cwd) = &fact.cwd {
                println!("cwd:        {cwd}");
            }
            println!("created_at: {}", fact.created_at.to_rfc3339());
            if let Some(t) = fact.occurred_at {
                println!("occurred:   {}", t.to_rfc3339());
            }
            if let Some(s) = &fact.source_session {
                println!("session:    {s}");
            }
            println!();
            println!("{}", fact.content);
            Ok(())
        }
    }
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

fn not_found(msg: &str) -> anyhow::Error {
    // Tagged error so main() can render a useful JSON code on stderr.
    NotFound(msg.to_string()).into()
}

/// Sentinel error type that `main` inspects to decide the exit code /
/// `code` field for JSON error output.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct NotFound(String);

/// Classify an error for structured CLI output.
pub fn error_code(err: &anyhow::Error) -> &'static str {
    if err.downcast_ref::<NotFound>().is_some() {
        "NOT_FOUND"
    } else {
        "ERROR"
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn filter_args_into_filters_preserves_fields() {
        let fa = FilterArgs {
            contexts: vec!["code/linggen".into()],
            types: vec![CliMemoryType::Fixed, CliMemoryType::Tried],
            tier: Some(CliTier::Core),
            from: Some(CliOrigin::User),
            outcome: Some(CliOutcome::Positive),
            since: None,
            until: None,
            older_than: None,
            day: None,
            cwd_scope: None,
            include_expired: false,
            superseded_by: None,
        };
        let filters = fa.into_filters().unwrap();
        assert_eq!(filters.contexts, vec!["code/linggen".to_string()]);
        assert_eq!(filters.types.len(), 2);
        assert_eq!(filters.tier, Some(Tier::Core));
        assert_eq!(filters.origin, Some(Origin::User));
        assert_eq!(filters.outcome, Some(Outcome::Positive));
    }

    #[test]
    fn filter_args_default_tier_is_none() {
        // Omitting `--tier` leaves the filter unconstrained — `into_filters`
        // must not coerce a default tier (that would hide semantic rows).
        let filters = FilterArgs::default().into_filters().unwrap();
        assert_eq!(filters.tier, None);
    }
}
