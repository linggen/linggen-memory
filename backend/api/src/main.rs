use anyhow::Result;
use clap::{Parser, Subcommand};

mod analytics;
mod cli;
mod handlers;
mod internal_indexer;
mod job_manager;
mod memory;
mod server;

#[derive(Parser, Debug)]
#[command(name = "ling-mem", version)]
#[command(about = "Linggen Memory — semantic memory and RAG server", long_about = None)]
struct Cli {
    /// Host address to bind to (default: 127.0.0.1)
    #[arg(long, global = true)]
    host: Option<String>,

    /// Port to listen on
    #[arg(long, short, global = true)]
    port: Option<u16>,

    /// Run as background daemon
    #[arg(short, long, default_value_t = false)]
    daemon: bool,

    /// Parent process PID (auto-exit when parent dies)
    #[arg(long, hide = true)]
    parent_pid: Option<u32>,

    #[command(subcommand)]
    cmd: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Stop background daemon
    Stop,
    /// Show server status
    Status,
    /// Index a local directory
    Index {
        /// Path to the directory to index (defaults to current directory)
        path: Option<std::path::PathBuf>,

        /// Indexing mode: auto, full, or incremental
        #[arg(long, default_value = "auto")]
        mode: String,

        /// Override the default source name
        #[arg(long)]
        name: Option<String>,

        /// Include patterns (glob patterns)
        #[arg(long = "include")]
        include_patterns: Vec<String>,

        /// Exclude patterns (glob patterns)
        #[arg(long = "exclude")]
        exclude_patterns: Vec<String>,

        /// Wait for the indexing job to complete (default: true, use --no-wait to disable)
        #[arg(long, default_value = "true")]
        wait: bool,
    },
    /// Self-install latest version
    Install,
    /// Self-update to latest version
    Update,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let host = cli
        .host
        .or_else(|| std::env::var("LINGGEN_HOST").ok())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let port = cli
        .port
        .or_else(|| {
            std::env::var("LINGGEN_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(8787);

    let server_url = format!("http://127.0.0.1:{}", port);

    match &cli.cmd {
        Some(Command::Stop) => {
            return cli::daemon::stop().await;
        }
        Some(Command::Status) => {
            return cli::daemon::status(port).await;
        }
        Some(Command::Index {
            path,
            mode,
            name,
            include_patterns,
            exclude_patterns,
            wait,
        }) => {
            return cli::index::run(
                &server_url,
                path.clone(),
                mode.clone(),
                name.clone(),
                include_patterns.clone(),
                exclude_patterns.clone(),
                *wait,
            )
            .await;
        }
        Some(Command::Install) | Some(Command::Update) => {
            return cli::self_update::run().await;
        }
        None => {}
    }

    // Daemon mode: spawn self in background and exit
    if cli.daemon {
        return cli::daemon::start(port).await;
    }

    // Default: start server in foreground
    server::start_server(&host, port, cli.parent_pid).await
}
