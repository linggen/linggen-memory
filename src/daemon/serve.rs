//! Foreground daemon: binds the HTTP server, writes the pidfile, waits
//! for shutdown signal, removes the pidfile, exits.
//!
//! `ling-mem serve` re-exec target — also what `start` spawns in the
//! background. Blocks until SIGTERM / SIGINT.

use crate::daemon::pidfile::{self, PidInfo};
use crate::embed::Embedder;
use crate::facts::FactsStore;
use crate::http;
use crate::http::state::AppState;
use anyhow::{Context, Result};
use chrono::Utc;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Bind, write pidfile, serve, clean up.
///
/// `data_dir` is the Linggen home (e.g. `~/.linggen/`); FactsStore opens
/// its LanceDB tree underneath at `<data_dir>/memory/facts.lancedb/`.
/// `skill_dir` is the per-skill data root at `<data_dir>/memory/linggen-memory/`
/// where the pidfile lives. `port` is the requested bind port on `127.0.0.1`.
pub async fn run(data_dir: &Path, skill_dir: &Path, port: u16) -> Result<()> {
    // The daemon is the only path that emits logs (the CLI stays quiet by
    // design). `start` redirects this process's stderr into serve.log; the
    // subscriber is what makes `tracing::*` actually write there. `try_init`
    // so a test harness that already set a global subscriber doesn't panic.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    // Refuse to start if another daemon is already live — overlapping
    // writers would corrupt the LanceDB store.
    if let Some(existing) = pidfile::read(skill_dir)? {
        if pidfile::pid_is_alive(existing.pid) {
            anyhow::bail!(
                "daemon already running (pid {} on port {}); run `ling-mem stop` first",
                existing.pid,
                existing.port
            );
        }
        // Stale pidfile — owner crashed without cleanup. Remove and proceed.
        tracing::warn!("stale pidfile (pid {} not alive); overwriting", existing.pid);
        pidfile::remove(skill_dir);
    }

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    let bound = listener
        .local_addr()
        .context("reading bound local addr")?;
    let bound_port = bound.port();

    let info = PidInfo {
        pid: std::process::id(),
        port: bound_port,
        started_at: Utc::now(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    pidfile::write(skill_dir, &info).context("writing pidfile")?;
    tracing::info!("ling-mem daemon listening on {bound}");
    eprintln!("ling-mem daemon listening on http://{bound}");

    // Anonymous usage telemetry (install/upgrade/heartbeat). See
    // src/telemetry/mod.rs for the field list and opt-out paths. Constructed
    // before the FactsStore so the launch ping can race with LanceDB init.
    let telemetry = crate::telemetry::Telemetry::new("ling-mem", data_dir);
    telemetry.launch();

    // Shared resources: FactsStore + Embedder are expensive to initialize
    // (LanceDB connection, ONNX model load). Build once, share across
    // every request via Arc<AppState>.
    let store = FactsStore::open(data_dir)
        .await
        .with_context(|| format!("opening facts store under {}", data_dir.display()))?;
    let embedder = Embedder::new().context("initializing embedder")?;
    let state = Arc::new(AppState {
        store: Arc::new(store),
        embedder: Arc::new(embedder),
    });

    let router = http::build_router(state, telemetry);
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("http server");

    pidfile::remove(skill_dir);
    tracing::info!("ling-mem daemon stopped");
    result
}

/// Resolve on SIGTERM (or SIGINT / Ctrl-C). Completes the future on any.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => { tracing::info!("received SIGINT") },
        _ = terminate => { tracing::info!("received SIGTERM") },
    }
}
