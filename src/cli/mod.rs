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

    /// Bulk-delete by filter. Refuses empty filters.
    Forget(ForgetArgs),

    /// Delete episodic rows older than a cutoff — the consolidation
    /// pass's decay sweep. Always targets the episodic store (the `semantic`
    /// table is curated and never auto-evicted); the `--episodic` flag is
    /// not needed. The engine owns the TTL policy and passes the resolved
    /// absolute cutoff; this binary stays policy-free.
    Evict(EvictArgs),

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

    /// Report daemon state: pidfile, liveness, health probe. Also surfaces
    /// the most recent (cached) upgrade probe so callers know whether a
    /// newer binary is available without making any network call.
    Status,

    /// Seed the data directory: ensure `<data-dir>/memory/` exists and that
    /// `identity.md` and `style.md` are present (touching empty files when
    /// missing). Idempotent — never overwrites existing content. Mirrors
    /// the `seed_core_memory` step that `install.sh` performs, so hosts
    /// that bypass `install.sh` (OpenClaw via ClawHub, recovery after a
    /// data-dir wipe) can call it directly.
    Init,

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

#[derive(Debug, Args)]
pub struct EvictArgs {
    /// RFC-3339 cutoff. Rows whose `COALESCE(updated_at, created_at)` is
    /// older than this are deleted. The caller computes the absolute
    /// instant (`now − TTL`); no `--ttl` / duration form by design.
    #[arg(long)]
    pub before: DateTime<Utc>,
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
}

impl From<CliTier> for Tier {
    fn from(v: CliTier) -> Tier {
        match v {
            CliTier::Core => Tier::Core,
            CliTier::Semantic => Tier::Semantic,
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
            tier: self.tier.map(Into::into),
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
        Command::Serve { port } => {
            return crate::daemon::serve::run(&data_dir, &skill_dir, port).await
        }
        Command::Start { port } => {
            let outcome = crate::daemon::lifecycle::start(&data_dir, &skill_dir, port).await?;
            let update = crate::update::check_quiet(&data_dir).await;
            return emit_lifecycle_with_update(&outcome, Some(&update));
        }
        Command::Stop => {
            let outcome = crate::daemon::lifecycle::stop(&skill_dir).await?;
            return emit_lifecycle(&outcome);
        }
        Command::Restart { port } => {
            let outcome =
                crate::daemon::lifecycle::restart(&data_dir, &skill_dir, port).await?;
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
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }
        Command::Init => {
            return cmd_init(&data_dir);
        }
        Command::Upgrade {
            check,
            force,
            yes,
            port,
        } => {
            return cmd_upgrade(&data_dir, &skill_dir, check, force, yes, port).await;
        }
        // `evict` is episodic-only by definition: it never touches the
        // curated `semantic` table and the HTTP daemon doesn't expose the
        // episodic store, so route it straight to the direct episodic
        // store rather than through the daemon-client dispatch below.
        Command::Evict(ref args) => {
            let before = args.before;
            let store = open_store(&data_dir, true)
                .await
                .with_context(|| format!("opening episodic store at {}", data_dir.display()))?;
            return cmd_evict(&store, before, format).await;
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
                Command::Serve { .. }
                | Command::Start { .. }
                | Command::Stop
                | Command::Restart { .. }
                | Command::Status
                | Command::Init
                | Command::Evict(_)
                | Command::Upgrade { .. } => unreachable!("handled above"),
            };
        }
    }

    let store = open_store(&data_dir, cli.episodic)
        .await
        .with_context(|| format!("opening store at {}", data_dir.display()))?;

    match cli.cmd {
        Command::Add(args) => cmd_add(&store, args, format).await,
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
        Command::Serve { .. }
        | Command::Start { .. }
        | Command::Stop
        | Command::Restart { .. }
        | Command::Status
        | Command::Init
        | Command::Evict(_)
        | Command::Upgrade { .. } => unreachable!("handled above"),
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

async fn cmd_add(store: &MemoryStore, args: AddArgs, format: OutputFormat) -> Result<()> {
    // Bulk stdin path: always plain insert. Dedup against hundreds of incoming
    // rows would be O(N) searches per call; callers who want dedup for bulk
    // imports should run `analyze-clean` afterwards.
    if args.stdin {
        // A stdin row that carries its own `tier` keeps it; rows without one
        // inherit the `--tier` flag (default `semantic`). `read_stdin_facts`
        // reports which rows omitted the key so we only override those.
        let (mut facts, tier_absent) = read_stdin_facts()?;
        let default_tier: Tier = args.tier.into();
        for (i, f) in facts.iter_mut().enumerate() {
            if tier_absent[i] {
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
    fact.tier = args.tier.into();
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
        .search_scored(
            &vec,
            &args.filters.into_filters(),
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
            &args.filters.into_filters(),
            args.limit,
            args.min_score,
        )
        .await?;
    emit_scored_facts(&results, format)
}

/// Emit scored search hits, attaching the cosine similarity to each row.
/// JSON output adds a `score` field; text output prefixes the score so
/// users can eyeball relevance without parsing JSON.
fn emit_scored_facts(scored: &[(crate::memory::Memory, f32)], format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            for (f, score) in scored {
                let mut v = serde_json::to_value(f)
                    .context("serializing fact to JSON for scored search output")?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("score".into(), serde_json::json!(score));
                }
                let line = serde_json::to_string(&v)
                    .context("encoding scored fact JSON")?;
                println!("{line}");
            }
        }
        OutputFormat::Text => {
            for (f, score) in scored {
                println!(
                    "{:.2} {} [{}] {}",
                    score,
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

async fn cmd_evict(
    store: &MemoryStore,
    before: DateTime<Utc>,
    format: OutputFormat,
) -> Result<()> {
    let evicted = store.evict_expired(before).await?;
    match format {
        OutputFormat::Json => writeln_ndjson(&serde_json::json!({ "evicted": evicted })),
        OutputFormat::Text => {
            println!("evicted {evicted} episodic fact(s)");
            Ok(())
        }
    }
}

// Session-scanning utilities (`cmd_collect` + `cmd_extract`) moved to
// bash scripts in `skills/memory/scripts/`. The daemon stays focused on
// the fact store.

/// Implements `ling-mem init` — idempotent seeding of the data directory
/// and core memory files. Mirrors the `seed_core_memory` step in
/// `install.sh` so non-`install.sh` install paths (OpenClaw via ClawHub,
/// or recovery after a `rm -rf ~/.linggen`) can self-recover by running
/// this command. Output reports which files were newly created.
fn cmd_init(data_dir: &std::path::Path) -> Result<()> {
    let memory_dir = data_dir.join("memory");
    std::fs::create_dir_all(&memory_dir)
        .with_context(|| format!("creating {}", memory_dir.display()))?;

    fn touch_if_missing(path: &std::path::Path) -> Result<bool> {
        if path.exists() {
            return Ok(false);
        }
        std::fs::write(path, b"")
            .with_context(|| format!("creating {}", path.display()))?;
        Ok(true)
    }

    let identity = memory_dir.join("identity.md");
    let style = memory_dir.join("style.md");
    let identity_created = touch_if_missing(&identity)?;
    let style_created = touch_if_missing(&style)?;

    let report = serde_json::json!({
        "data_dir": data_dir,
        "memory_dir": memory_dir,
        "identity_md": { "path": identity, "created": identity_created },
        "style_md":    { "path": style,    "created": style_created },
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

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
    let mut out = Vec::new();
    let mut tier_absent = Vec::new();
    let stdin = std::io::stdin();
    for (i, line) in stdin.lock().lines().enumerate() {
        let line = line.with_context(|| format!("reading stdin line {}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line)
            .with_context(|| format!("parsing JSON on stdin line {}", i + 1))?;
        let has_tier = value.get("tier").is_some();
        let fact: crate::memory::Memory = serde_json::from_value(value)
            .with_context(|| format!("parsing JSON on stdin line {}", i + 1))?;
        out.push(fact);
        tier_absent.push(!has_tier);
    }
    Ok((out, tier_absent))
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
        };
        let filters = fa.into_filters();
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
        let filters = FilterArgs::default().into_filters();
        assert_eq!(filters.tier, None);
    }
}
