//! `ling-mem` CLI — clap-parsed subcommands, dispatched to [`FactsStore`].
//!
//! The contract is documented in `doc/tech-spec.md`:
//! - **Default output**: NDJSON on stdout, one fact per line for list-like
//!   results; single JSON object for single-row results.
//! - **Errors**: JSON on stderr, non-zero exit (`{"error": "...", "code": ""}`).
//! - **Human format**: `--format text` for friendly text, `--format json`
//!   (default) for NDJSON.
//! - **Data dir**: `--data-dir`, then `$LINGGEN_DATA_DIR`, then `~/.linggen/`.
//! - **Search**: takes a plain text query; the query is embedded on the fly
//!   via [`crate::embed::Embedder`] (MiniLM-L6-v2) and filtered against the
//!   LanceDB vector column.
//! - **Add**: auto-embeds content for any inserted fact that doesn't already
//!   carry a vector, so the row is immediately searchable.

use crate::facts::{FactPatch, FactType, FactsStore, Filters, Origin, Outcome, SortOrder};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use uuid::Uuid;

mod client;

/// Top-level CLI. All subcommands share the flags declared here (data dir,
/// output format, quiet).
#[derive(Debug, Parser)]
#[command(name = "ling-mem", version, about = "Semantic memory store")]
pub struct Cli {
    /// Override the data directory. Falls back to `$LINGGEN_DATA_DIR`, then
    /// `~/.linggen/`. LanceDB lives under `<data-dir>/memory/facts.lancedb/`.
    #[arg(long, global = true, env = "LINGGEN_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Json)]
    pub format: OutputFormat,

    /// Suppress non-essential progress output.
    #[arg(long, global = true)]
    pub quiet: bool,

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
    Get { id: Uuid },

    /// Semantic search — vector query + metadata filters.
    Search(SearchArgs),

    /// Non-semantic browse — metadata filters only.
    List(ListArgs),

    /// Modify fields of an existing fact.
    Update(UpdateArgs),

    /// Hard-delete a fact by id.
    Delete {
        id: Uuid,
        /// Skip confirmation (required for scripted use).
        #[arg(long)]
        yes: bool,
    },

    /// Bulk-delete by filter. Refuses empty filters.
    Forget(ForgetArgs),

    // Session-scanning utilities (`collect` + `extract`) used to live here.
    // They moved to `skills/memory/scripts/` as bash helpers — the daemon is
    // a pure data service; reading session files isn't its concern.

    /// Run the HTTP daemon in the foreground. Blocks until SIGTERM / SIGINT.
    /// What `start` re-exec's, and what launchd/systemd should call.
    Serve {
        #[arg(long, default_value_t = crate::daemon::DEFAULT_PORT)]
        port: u16,
    },

    /// Spawn the daemon in the background and wait for it to bind.
    Start {
        #[arg(long, default_value_t = crate::daemon::DEFAULT_PORT)]
        port: u16,
    },

    /// SIGTERM the running daemon and wait for it to exit.
    Stop,

    /// Stop + start.
    Restart {
        #[arg(long, default_value_t = crate::daemon::DEFAULT_PORT)]
        port: u16,
    },

    /// Report daemon state: pidfile, liveness, health probe.
    Status,
}

// ── Argument structs ────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct AddArgs {
    /// The fact text. Omit when using `--stdin`.
    pub content: Option<String>,

    #[arg(long, value_enum, default_value_t = CliFactType::Fact)]
    pub r#type: CliFactType,

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
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Text query — embedded on the fly via MiniLM-L6-v2 (384-dim).
    /// First-run downloads the ONNX model (~23 MB); subsequent calls
    /// load from cache.
    pub query: String,

    #[command(flatten)]
    pub filters: FilterArgs,

    #[arg(long, default_value_t = 10)]
    pub limit: usize,
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

#[derive(Debug, Args, Default, Clone)]
pub struct FilterArgs {
    #[arg(long = "context", value_name = "CONTEXT")]
    pub contexts: Vec<String>,

    #[arg(long = "type", value_enum, value_name = "TYPE")]
    pub types: Vec<CliFactType>,

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
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    pub id: Uuid,

    #[arg(long)]
    pub content: Option<String>,

    #[arg(long = "context", value_name = "CONTEXT")]
    pub contexts: Option<Vec<String>>,

    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Option<Vec<String>>,

    #[arg(long, value_enum)]
    pub r#type: Option<CliFactType>,

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
pub enum CliFactType {
    Fact,
    Preference,
    Decision,
    Tried,
    Fixed,
    Learned,
    Built,
}

impl From<CliFactType> for FactType {
    fn from(v: CliFactType) -> FactType {
        match v {
            CliFactType::Fact => FactType::Fact,
            CliFactType::Preference => FactType::Preference,
            CliFactType::Decision => FactType::Decision,
            CliFactType::Tried => FactType::Tried,
            CliFactType::Fixed => FactType::Fixed,
            CliFactType::Learned => FactType::Learned,
            CliFactType::Built => FactType::Built,
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
    fn into_filters(self) -> Filters {
        Filters {
            contexts: self.contexts,
            types: self.types.into_iter().map(Into::into).collect(),
            origin: self.from.map(Into::into),
            outcome: self.outcome.map(Into::into),
            since: self.since,
            until: self.until,
        }
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

/// Run the CLI. Dispatches to the appropriate subcommand and returns its
/// result. Errors propagate to `main` which serializes them to stderr.
pub async fn run(cli: Cli) -> Result<()> {
    let format = cli.format;

    // Lifecycle commands don't need the LanceDB store — route them
    // before opening it.
    let data_dir = resolve_data_dir(cli.data_dir)?;
    let skill_dir = crate::daemon::skill_dir(&data_dir);
    match cli.cmd {
        Command::Serve { port } => {
            return crate::daemon::serve::run(&data_dir, &skill_dir, port).await
        }
        Command::Start { port } => {
            let outcome = crate::daemon::lifecycle::start(&data_dir, &skill_dir, port).await?;
            return emit_lifecycle(&outcome);
        }
        Command::Stop => {
            let outcome = crate::daemon::lifecycle::stop(&skill_dir).await?;
            return emit_lifecycle(&outcome);
        }
        Command::Restart { port } => {
            let outcome =
                crate::daemon::lifecycle::restart(&data_dir, &skill_dir, port).await?;
            return emit_lifecycle(&outcome);
        }
        Command::Status => {
            let value = crate::daemon::lifecycle::status(&skill_dir).await?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }
        _ => {}
    }

    // Prefer the running daemon when one is reachable. This eliminates the
    // CLI/daemon data-path split: with the daemon up, the CLI is a typed
    // shell client over HTTP and there's exactly one writer to the store.
    // If no daemon is running, autostart one (matches Linggen engine
    // semantics in `engine::capability_tools::dispatch`). Direct
    // `FactsStore` mode below remains a fallback for when autostart fails
    // (e.g. port in use, binary missing).
    if let Some(base_url) = client::try_running_or_start(&data_dir, &skill_dir).await {
        return match cli.cmd {
            Command::Add(args) => client::add(&base_url, args, format).await,
            Command::Get { id } => client::get(&base_url, id, format).await,
            Command::Search(args) => client::search(&base_url, args, format).await,
            Command::List(args) => client::list(&base_url, args, format).await,
            Command::Update(args) => client::update(&base_url, args, format).await,
            Command::Delete { id, yes } => client::delete(&base_url, id, yes, format).await,
            Command::Forget(args) => client::forget(&base_url, args, format).await,
            Command::Serve { .. }
            | Command::Start { .. }
            | Command::Stop
            | Command::Restart { .. }
            | Command::Status => unreachable!("handled above"),
        };
    }

    let store = FactsStore::open(&data_dir)
        .await
        .with_context(|| format!("opening store at {}", data_dir.display()))?;

    match cli.cmd {
        Command::Add(args) => cmd_add(&store, args, format).await,
        Command::Get { id } => cmd_get(&store, id, format).await,
        Command::Search(args) => cmd_search(&store, args, format).await,
        Command::List(args) => cmd_list(&store, args, format).await,
        Command::Update(args) => cmd_update(&store, args, format).await,
        Command::Delete { id, yes } => cmd_delete(&store, id, yes, format).await,
        Command::Forget(args) => cmd_forget(&store, args, format).await,
        Command::Serve { .. }
        | Command::Start { .. }
        | Command::Stop
        | Command::Restart { .. }
        | Command::Status => unreachable!("handled above"),
    }
}

fn emit_lifecycle(outcome: &crate::daemon::lifecycle::LifecycleOutcome) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&outcome.to_json())?);
    Ok(())
}

// ── Subcommand handlers ─────────────────────────────────────────────────────

async fn cmd_add(store: &FactsStore, args: AddArgs, format: OutputFormat) -> Result<()> {
    // Bulk stdin path: always plain insert. Dedup against hundreds of incoming
    // rows would be O(N) searches per call; callers who want dedup for bulk
    // imports should run `analyze-clean` afterwards.
    if args.stdin {
        let mut facts = read_stdin_facts()?;
        embed_missing(&mut facts)?;
        store.insert(&facts).await?;
        return emit_added(&facts, format);
    }

    let content = args
        .content
        .clone()
        .ok_or_else(|| anyhow!("add: provide content or use --stdin"))?;

    let mut fact = crate::facts::Fact::new(content, args.r#type.into(), args.from.into());
    fact.contexts = args.contexts;
    fact.tags = args.tags;
    fact.outcome = args.outcome.map(Into::into);
    fact.cwd = args.cwd;
    fact.occurred_at = args.occurred_at;
    fact.source_session = args.source_session;

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

fn emit_added(facts: &[crate::facts::Fact], format: OutputFormat) -> Result<()> {
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
    outcome: &crate::facts::InsertOutcome,
    format: OutputFormat,
) -> Result<()> {
    use crate::facts::InsertOutcome;
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

async fn cmd_get(store: &FactsStore, id: Uuid, format: OutputFormat) -> Result<()> {
    match store.get(id).await? {
        Some(f) => emit_fact(&f, format),
        None => Err(not_found("no fact with that id")),
    }
}

async fn cmd_search(store: &FactsStore, args: SearchArgs, format: OutputFormat) -> Result<()> {
    let embedder =
        crate::embed::Embedder::new().context("initializing embedder for search query")?;
    let vec = embedder.embed_one(&args.query)?;
    let results = store
        .search(&vec, &args.filters.into_filters(), args.limit)
        .await?;
    emit_facts(&results, format)
}

/// Embed `content` into `vector` for every fact missing a vector but
/// carrying content. Zero cost when every fact already has a vector
/// (e.g. stdin NDJSON from a prior embedding pipeline). Batches all
/// missing-vector facts into a single model call.
fn embed_missing(facts: &mut [crate::facts::Fact]) -> Result<()> {
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

async fn cmd_list(store: &FactsStore, args: ListArgs, format: OutputFormat) -> Result<()> {
    let results = store
        .list(
            &args.filters.into_filters(),
            args.sort.into(),
            args.limit,
            args.offset,
        )
        .await?;
    emit_facts(&results, format)
}

async fn cmd_update(store: &FactsStore, args: UpdateArgs, format: OutputFormat) -> Result<()> {
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

    let patch = FactPatch {
        content: args.content,
        contexts: args.contexts,
        tags: args.tags,
        r#type: args.r#type.map(Into::into),
        origin: args.from.map(Into::into),
        outcome: outcome_patch,
        cwd: cwd_patch,
        ..Default::default()
    };

    match store.update(args.id, &patch).await? {
        Some(f) => emit_fact(&f, format),
        None => Err(not_found("no fact with that id")),
    }
}

async fn cmd_delete(store: &FactsStore, id: Uuid, yes: bool, format: OutputFormat) -> Result<()> {
    if !yes {
        return Err(anyhow!(
            "refusing to delete without --yes (scripted calls must pass the flag)"
        ));
    }
    let removed = store.delete(id).await?;
    match format {
        OutputFormat::Json => writeln_ndjson(&serde_json::json!({
            "id": id.to_string(),
            "removed": removed,
        })),
        OutputFormat::Text => {
            println!("{} {}", if removed { "deleted" } else { "not found" }, id);
            Ok(())
        }
    }
}

async fn cmd_forget(store: &FactsStore, args: ForgetArgs, format: OutputFormat) -> Result<()> {
    if !args.yes {
        return Err(anyhow!(
            "refusing to forget without --yes (bulk delete requires explicit confirmation)"
        ));
    }
    let filters = args.filters.into_filters();
    let removed = store.forget(&filters).await?;
    match format {
        OutputFormat::Json => writeln_ndjson(&serde_json::json!({ "removed": removed })),
        OutputFormat::Text => {
            println!("forgot {removed} fact(s)");
            Ok(())
        }
    }
}

// Session-scanning utilities (`cmd_collect` + `cmd_extract`) moved to
// bash scripts in `skills/memory/scripts/`. The daemon stays focused on
// the fact store.

// ── I/O helpers ─────────────────────────────────────────────────────────────

fn read_stdin_facts() -> Result<Vec<crate::facts::Fact>> {
    let mut out = Vec::new();
    let stdin = std::io::stdin();
    for (i, line) in stdin.lock().lines().enumerate() {
        let line = line.with_context(|| format!("reading stdin line {}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let fact: crate::facts::Fact = serde_json::from_str(&line)
            .with_context(|| format!("parsing JSON on stdin line {}", i + 1))?;
        out.push(fact);
    }
    Ok(out)
}

fn emit_facts(facts: &[crate::facts::Fact], format: OutputFormat) -> Result<()> {
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

fn emit_fact(fact: &crate::facts::Fact, format: OutputFormat) -> Result<()> {
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
            types: vec![CliFactType::Fixed, CliFactType::Tried],
            from: Some(CliOrigin::User),
            outcome: Some(CliOutcome::Positive),
            since: None,
            until: None,
        };
        let filters = fa.into_filters();
        assert_eq!(filters.contexts, vec!["code/linggen".to_string()]);
        assert_eq!(filters.types.len(), 2);
        assert_eq!(filters.origin, Some(Origin::User));
        assert_eq!(filters.outcome, Some(Outcome::Positive));
    }
}
