//! Foreground daemon: binds the HTTP server, writes the pidfile, waits
//! for shutdown signal, removes the pidfile, exits.
//!
//! `ling-mem serve` re-exec target — also what `start` spawns in the
//! background. Blocks until SIGTERM / SIGINT, drains open requests for at
//! most [`SHUTDOWN_GRACE`], removes the pidfile, then ends the process at
//! once ([`exit_now`]) — `stop` gives it 10 s, and a teardown after serving
//! (the tokio runtime joining its blocking pool, the warm Metal embedder's
//! exit-time cleanup) once held a stopped daemon alive past that.
//!
//! Binds the LAN (`0.0.0.0`) by default, or loopback when the console's
//! network setting says "this machine only"; an explicit `--host` overrides
//! both (`http::network`). Off-machine callers still need a paired device's
//! token — `http::gate` — so a machine that never paired anything opens a port
//! but not its store.

use crate::daemon::pidfile::{self, PidInfo};
use crate::embed::{Embedder, LoadOnce};
use crate::http;
use crate::http::state::AppState;
use crate::memory::{MemoryStore, Recall};
use anyhow::{Context, Result};
use chrono::Utc;
use std::future::IntoFuture;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

/// How long a stopping daemon waits for open requests to finish — well under
/// `stop`'s 10 s, so a daemon behaving as designed is gone before `stop`
/// escalates. Long-lived connections (an MCP host's SSE stream) never end on
/// their own; past this they are dropped with the process.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// One change log for both tables (`memory::changes`) — what a replica pulls.
/// A store that has never had one is seeded with every existing row, so a
/// replica's first pull is a full copy.
async fn attach_change_log(
    data_dir: &Path,
    semantic: &MemoryStore,
    episodic: &MemoryStore,
) -> Result<()> {
    use crate::memory::changes::{log_path, ChangeLog, Op};
    let log = Arc::new(ChangeLog::open(log_path(data_dir)).context("opening the change log")?);
    if log.needs_seed() {
        for store in [semantic, episodic] {
            let ids = store.ids_where(None).await?;
            log.record(store.table_name(), Op::Upsert, &ids);
        }
        tracing::info!("change log seeded at seq {}", log.head());
    }
    semantic.attach_changes(Arc::clone(&log));
    episodic.attach_changes(log);
    Ok(())
}

/// Bind, write pidfile, serve, clean up.
///
/// `data_dir` is the Linggen home (e.g. `~/.linggen/`); MemoryStore opens
/// its LanceDB tree underneath at `<data_dir>/memory/memory.lancedb/`.
/// `skill_dir` is the per-skill data root at `<data_dir>/memory/linggen-memory/`
/// where the pidfile lives. `port` is the requested port; `host` is the
/// `--host` flag if one was typed, else the saved network setting decides.
pub async fn run(data_dir: &Path, skill_dir: &Path, port: u16, host: Option<IpAddr>) -> Result<()> {
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
        // Stale pidfile — owner crashed without cleanup. Left in place until
        // this process has actually bound: `pidfile::write` replaces it
        // atomically after the bind, so a bind that fails leaves the file —
        // and whatever it still describes — untouched.
        tracing::warn!(
            "stale pidfile (pid {} not alive); overwriting after bind",
            existing.pid
        );
    }

    // The one place the bind is decided, so no starter has to pass `--host`.
    let setting = crate::http::config::load(data_dir).await.network;
    let host = crate::http::network::resolve_bind(host, setting);

    let addr = SocketAddr::new(host, port);
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    let bound = listener.local_addr().context("reading bound local addr")?;
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
    // before the MemoryStore so the launch ping can race with LanceDB init.
    let telemetry = crate::telemetry::Telemetry::new("ling-mem", data_dir);
    telemetry.launch();

    // Shared resources: the stores (LanceDB connections) open here, before
    // serving — nearly every request reads them. Built once, shared across
    // every request via Arc<AppState>.
    let semantic = Arc::new(
        MemoryStore::open_semantic(data_dir)
            .await
            .with_context(|| format!("opening memory store under {}", data_dir.display()))?,
    );
    let episodic = Arc::new(
        MemoryStore::open_episodic(data_dir)
            .await
            .with_context(|| format!("opening episodic store under {}", data_dir.display()))?,
    );
    attach_change_log(data_dir, &semantic, &episodic).await?;
    // Recall shares the semantic handle with the write path so the table
    // has one LanceDB connection across reads and writes. The episodic
    // handle is reused for write routing in `http::memory::add` when the
    // caller sets `episodic: true` — same single connection per table.
    let recall = Arc::new(Recall::new(Arc::clone(&semantic), Arc::clone(&episodic)));
    // The embedding model loads in the background, not here: until
    // `axum::serve` runs, the bound listener only queues connections, and
    // a cold-disk model load (several seconds) held every early request —
    // `session_start` included, which never embeds — past its host's cap.
    let embedder = Arc::new(LoadOnce::new(Embedder::new));
    let state = Arc::new(AppState::new(
        semantic,
        episodic,
        recall,
        Arc::clone(&embedder),
        data_dir.to_path_buf(),
        bound,
    ));

    // Background storage upkeep: compaction + version pruning, on a
    // measured condition, in idle windows. Silent by design — see
    // `daemon::maintenance`.
    crate::daemon::maintenance::spawn(Arc::clone(&state));
    // A replica follows its primary; a primary's loop just sleeps.
    crate::replica::spawn(Arc::clone(&state));
    warm_embedder(embedder);

    let router = http::build_router(state, telemetry);
    // Connect info, because the gate's first question is where the caller is:
    // loopback is inside the fence, anything else must present a paired
    // device's token.
    let (stopping_tx, stopping) = tokio::sync::watch::channel(false);
    let signal = async move {
        shutdown_signal().await;
        let _ = stopping_tx.send(true);
    };
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(signal)
    .into_future();
    let result = drain_bounded(server, stopping, SHUTDOWN_GRACE)
        .await
        .context("http server");

    // Only the file this process wrote. A daemon that lost the bind never
    // gets here; this guards the day one somehow does.
    let ours = pidfile::read(skill_dir)
        .ok()
        .flatten()
        .map_or(true, |i| i.pid == std::process::id());
    if ours {
        pidfile::remove(skill_dir);
    }
    tracing::info!("ling-mem daemon stopped");
    exit_now(result)
}

/// Serve until the graceful shutdown completes — or until `grace` has passed
/// since `stopping` turned true, whichever is first. Requests still open then
/// are abandoned (the process exits right after).
async fn drain_bounded<F>(
    server: F,
    mut stopping: tokio::sync::watch::Receiver<bool>,
    grace: Duration,
) -> std::io::Result<()>
where
    F: std::future::Future<Output = std::io::Result<()>>,
{
    tokio::pin!(server);
    tokio::select! {
        done = &mut server => return done,
        // An error means the sender is gone, which only happens once the
        // signal future has run: stopping either way.
        _ = stopping.wait_for(|s| *s) => {}
    }
    match tokio::time::timeout(grace, server).await {
        Ok(done) => done,
        Err(_) => {
            tracing::warn!(
                "connections still open {}s after the stop signal; closing them",
                grace.as_secs()
            );
            Ok(())
        }
    }
}

/// End the process now. Everything that must survive is already on disk:
/// LanceDB commits each write before its request answers, the change log
/// flushes per append, the pidfile is gone. What is left is teardown nobody
/// needs — the tokio runtime waits on every running blocking task (a model
/// load, a DNS lookup that a timed-out request left behind) and exit-time
/// handlers release the Metal device — and that is what kept a stopped daemon
/// alive past `stop`'s 10 s. `_exit` skips both. The error path keeps main's
/// contract: one JSON line on stderr, status 1.
fn exit_now(result: Result<()>) -> ! {
    use std::io::Write;
    let code = match result {
        Ok(()) => 0,
        Err(err) => {
            let payload = serde_json::json!({
                "error": err.to_string(),
                "code": crate::cli::error_code(&err),
            });
            let mut stderr = std::io::stderr().lock();
            let _ = serde_json::to_writer(&mut stderr, &payload);
            let _ = stderr.write_all(b"\n");
            1
        }
    };
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    // SAFETY: `_exit` only ends the process; it touches no Rust state.
    unsafe {
        libc::_exit(code)
    }
    #[cfg(not(unix))]
    std::process::exit(code)
}

/// Load the embedding model and run one forward pass (the first pass on
/// Metal compiles its kernels), so the first search or add after a start
/// does not pay for either. Requests that embed meanwhile await the same
/// load; a failure is logged and the next such request tries again.
fn warm_embedder(embedder: Arc<LoadOnce<Embedder>>) {
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let warmed = match embedder.get().await {
            Ok(e) => e
                .embed_query_serialized("warm up".to_string())
                .await
                .map(|_| ()),
            Err(e) => Err(e),
        };
        match warmed {
            Ok(()) => tracing::info!("embedder: warm in {:?}", started.elapsed()),
            Err(e) => tracing::warn!("embedder: background load failed: {e:#}"),
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_gives_up_on_a_connection_that_never_closes() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let never = std::future::pending::<std::io::Result<()>>();
        tx.send(true).unwrap();
        let started = std::time::Instant::now();
        drain_bounded(never, rx, Duration::from_millis(200))
            .await
            .unwrap();
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(200) && took < Duration::from_secs(2),
            "{took:?}"
        );
    }

    #[tokio::test]
    async fn drain_returns_as_soon_as_the_server_finishes() {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let done = async { Ok::<(), std::io::Error>(()) };
        drain_bounded(done, rx, Duration::from_secs(60))
            .await
            .unwrap();
    }
}
