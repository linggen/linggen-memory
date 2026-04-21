use anyhow::Context;
use axum::{
    body::Body,
    extract::DefaultBodyLimit,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::delete,
    routing::get,
    routing::post,
    Router,
};
use dashmap::DashMap;
use dirs::data_dir;
use embeddings::{EmbeddingModel, TextChunker};
use rust_embed::RustEmbed;
use std::net::SocketAddr;
use std::time::Duration;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use storage::{MetadataStore, VectorStore};
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(RustEmbed)]
#[folder = "../../frontend/dist/"]
struct Assets;

async fn static_handler(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    let path = path.trim_start_matches('/');

    if path.is_empty() || path == "index.html" {
        return index_handler().await;
    }

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(content.data))
                .unwrap()
        }
        None => {
            if path.contains('.') {
                StatusCode::NOT_FOUND.into_response()
            } else {
                index_handler().await
            }
        }
    }
}

async fn index_handler() -> Response {
    match Assets::get("index.html") {
        Some(content) => Response::builder()
            .header(header::CONTENT_TYPE, "text/html")
            .body(Body::from(content.data))
            .unwrap(),
        None => (StatusCode::NOT_FOUND, "Frontend assets not found").into_response(),
    }
}

use crate::analytics;
use crate::handlers::{
    add_resource, cancel_job, clear_all_data, create_folder, create_pack, delete_folder,
    delete_pack, delete_uploaded_file, download_skill, get_app_status, get_pack, list_jobs,
    list_library, list_resources, list_uploaded_files, remove_resource, rename_folder, rename_pack,
    rename_resource, save_pack, shutdown_server, update_resource_patterns, upload_file,
    upload_file_stream, AppState,
};
use crate::job_manager::JobManager;

//.linggen/memory/2025-12-16T19:20:50.051071+00:00__hello-memory-test__mem_48748d35-9c3f-4526-b85e-c84dc5f9ef92.md
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    // On Unix, `kill(pid, 0)` checks for existence without sending a signal.
    // If the PID doesn't exist, errno = ESRCH.
    unsafe {
        let res = libc::kill(pid as i32, 0);
        if res == 0 {
            return true;
        }
        // If we don't have permission, the process still exists.
        std::io::Error::last_os_error()
            .raw_os_error()
            .is_some_and(|e| e == libc::EPERM)
    }
}

#[cfg(unix)]
fn get_parent_pid() -> u32 {
    unsafe { libc::getppid() as u32 }
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    true
}

pub async fn start_server(host: &str, port: u16, parent_pid: Option<u32>) -> anyhow::Result<()> {
    setup_tracing();
    info!(
        "Linggen Backend API v{} starting on port {}...",
        env!("CARGO_PKG_VERSION"),
        port
    );

    setup_parent_monitor(parent_pid);

    let (base_data_dir, metadata_path, lancedb_path, library_path) = resolve_data_paths();
    info!("Using metadata store at {:?}", metadata_path);
    info!("Using LanceDB store at {:?}", lancedb_path);
    info!("Using Library store at {:?}", library_path);

    let metadata_store = init_metadata_store(&metadata_path).await?;

    init_analytics(&metadata_store).await;

    let embedding_model = init_embedding_model(&metadata_store);
    let chunker = Arc::new(TextChunker::new());

    let (vector_store, internal_index_store) = init_vector_stores(&lancedb_path).await;

    cleanup_interrupted_jobs(&metadata_store);

    let job_manager = Arc::new(JobManager::new(1)); // Limit to 1 concurrent job

    let (linggen_dir, memory_dir) = init_memory_store(&base_data_dir);
    let memory_store = Arc::new(
        crate::memory::MemoryStore::new(memory_dir.clone())
            .expect("Failed to initialize memory store"),
    );

    let (broadcast_tx, _) = tokio::sync::broadcast::channel(100);

    let app_state = Arc::new(AppState {
        embedding_model: embedding_model.clone(),
        chunker: chunker.clone(),
        vector_store: vector_store.clone(),
        internal_index_store: internal_index_store.clone(),
        metadata_store: metadata_store.clone(),
        memory_store: memory_store.clone(),
        cancellation_flags: DashMap::new(),
        job_manager: job_manager.clone(),
        broadcast_tx: broadcast_tx.clone(),
        library_path: library_path.clone(),
    });

    init_watchers(
        &app_state,
        &internal_index_store,
        &embedding_model,
        &chunker,
        &broadcast_tx,
        &metadata_store,
        &linggen_dir,
    );

    let app = create_router(app_state);

    // Run it
    let bind_addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    let actual_addr = listener.local_addr()?;
    info!("listening on {}", actual_addr);
    axum::serve(listener, app).await?;

    Ok(())
}

fn setup_tracing() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(true)
        .with_line_number(true)
        .with_level(true)
        .compact()
        .init();
}

fn setup_parent_monitor(parent_pid: Option<u32>) {
    if let Some(ppid) = parent_pid {
        info!(
            "Parent PID set to {} (will auto-exit when parent exits)",
            ppid
        );
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if !pid_is_alive(ppid) {
                    eprintln!(
                        "[linggen-server] Parent PID {} exited; shutting down backend sidecar",
                        ppid
                    );
                    std::process::exit(0);
                }

                #[cfg(unix)]
                if get_parent_pid() == 1 {
                    eprintln!(
                        "[linggen-server] Backend sidecar was orphaned (adopted by init); shutting down"
                    );
                    std::process::exit(0);
                }
            }
        });
    }
}

fn resolve_data_paths() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let base_data_dir = if let Ok(dir) = std::env::var("LINGGEN_DATA_DIR") {
        PathBuf::from(dir)
    } else {
        data_dir().map(|d| d.join("Linggen")).unwrap_or_else(|| {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            if cwd.join("data").is_dir() {
                cwd.join("data")
            } else {
                cwd.join("Linggen")
            }
        })
    };

    let metadata_path = base_data_dir.join("metadata.redb");
    let lancedb_path = base_data_dir.join("lancedb");

    // Library packs (skills/policies/templates).
    //
    // Linux gotcha:
    // - When started via systemd, `HOME` may be unset or point somewhere else.
    // - `dirs::home_dir()` can then resolve to a different user (or None).
    //
    // We solve this by:
    // - Allowing an explicit override: LINGGEN_LIBRARY_DIR
    // - Falling back to resolving the *current process user* home via /etc/passwd (UID)
    // - Trying XDG_DATA_HOME as an additional fallback
    let library_path = resolve_library_path(&base_data_dir);

    (base_data_dir, metadata_path, lancedb_path, library_path)
}

fn resolve_library_path(base_data_dir: &Path) -> PathBuf {
    // 1) Explicit override (best for systemd/root/service installs)
    if let Ok(dir) = std::env::var("LINGGEN_LIBRARY_DIR") {
        return PathBuf::from(dir);
    }

    // 2) $HOME (or /etc/passwd lookup) -> ~/.linggen/library (legacy but supported)
    if let Some(home) = resolve_home_dir_linux_safe() {
        let p = home.join(".linggen/library");
        if p.is_dir() {
            return p;
        }
        // If it doesn't exist yet, still prefer this default location.
        return p;
    }

    // 3) XDG data dir fallback
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg).join("Linggen/library");
        if p.is_dir() {
            return p;
        }
    }

    // 4) Final fallback: within LINGGEN_DATA_DIR
    base_data_dir.join("library")
}

fn resolve_home_dir_linux_safe() -> Option<PathBuf> {
    // Prefer HOME if set (works for normal shells).
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return Some(PathBuf::from(home));
        }
    }

    // Fall back to the current process user's home.
    // This fixes cases where HOME is unset (common in systemd services).
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        unsafe {
            let uid = libc::getuid();

            // getpwuid_r requires a caller-provided buffer
            let mut pwd: libc::passwd = std::mem::zeroed();
            let mut result: *mut libc::passwd = std::ptr::null_mut();
            let mut buf = vec![0u8; 16 * 1024];

            let rc = libc::getpwuid_r(
                uid,
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            );

            if rc == 0 && !result.is_null() && !pwd.pw_dir.is_null() {
                if let Ok(s) = CStr::from_ptr(pwd.pw_dir).to_str() {
                    if !s.trim().is_empty() {
                        return Some(PathBuf::from(s));
                    }
                }
            }
        }
    }

    // As a last attempt, use dirs (may still be None on some service contexts).
    dirs::home_dir()
}

async fn init_metadata_store(metadata_path: &Path) -> anyhow::Result<Arc<MetadataStore>> {
    let mut metadata_store_result = MetadataStore::new(metadata_path);
    let mut lock_retries = 0;
    let max_lock_retries = 20;

    while metadata_store_result.is_err() && lock_retries < max_lock_retries {
        let err_msg = metadata_store_result.as_ref().err().unwrap().to_string();
        if err_msg.contains("Database already open") || err_msg.contains("Cannot acquire lock") {
            warn!(
                "Metadata database is locked, retrying in 500ms... (attempt {}/{})",
                lock_retries + 1,
                max_lock_retries
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
            metadata_store_result = MetadataStore::new(metadata_path);
            lock_retries += 1;
        } else {
            break;
        }
    }

    match metadata_store_result {
        Ok(s) => Ok(Arc::new(s)),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Database already open") || msg.contains("Cannot acquire lock") {
                Err(anyhow::anyhow!(
                    "Linggen metadata database is still locked after {} seconds.\n\
                     - Another Linggen backend is likely running.\n\
                     - Stop the other backend OR run this instance with a separate data dir via LINGGEN_DATA_DIR.\n\
                     \n\
                     DB path: {}",
                    max_lock_retries / 2,
                    metadata_path.display()
                ))
            } else {
                Err(e).with_context(|| {
                    format!(
                        "Failed to initialize metadata store at {}",
                        metadata_path.display()
                    )
                })
            }
        }
    }
}

async fn init_analytics(metadata_store: &Arc<MetadataStore>) {
    info!("Initializing analytics...");
    let first_launch = metadata_store
        .get_setting("installation_id")
        .ok()
        .flatten()
        .is_none();
    match analytics::AnalyticsClient::initialize(metadata_store).await {
        Ok(analytics_client) => {
            info!(
                "Analytics initialized (enabled: {})",
                analytics_client.is_enabled()
            );
            let analytics_client_clone = analytics_client.clone();
            tokio::spawn(async move {
                analytics_client_clone.track_app_started(first_launch).await;
            });
        }
        Err(e) => {
            info!("Failed to initialize analytics (non-fatal): {}", e);
        }
    }
}

fn init_embedding_model(
    metadata_store: &Arc<MetadataStore>,
) -> Arc<RwLock<Option<EmbeddingModel>>> {
    info!("Loading embedding model (async)...");
    // Keys for *embedding* model init. (LLM init uses separate keys.)
    let _ = metadata_store.set_setting("embedding_model_initialized", "false");
    let _ = metadata_store.set_setting("embedding_init_progress", "Starting...");

    let embedding_model = Arc::new(RwLock::new(None));
    let embedding_model_clone = embedding_model.clone();
    let metadata_store_clone = metadata_store.clone();

    tokio::spawn(async move {
        let _ = metadata_store_clone
            .set_setting("embedding_init_progress", "Downloading embedding model...");
        let model_result = tokio::task::spawn_blocking(|| EmbeddingModel::new()).await;

        match model_result {
            Ok(Ok(model)) => {
                let mut lock = embedding_model_clone.write().await;
                *lock = Some(model);
                let _ = metadata_store_clone.set_setting("embedding_init_progress", "Ready");
                let _ = metadata_store_clone.set_setting("embedding_model_initialized", "true");
                info!("Embedding model loaded successfully");
            }
            Ok(Err(e)) => {
                let error_msg = format!("Failed to load embedding model: {}", e);
                let _ = metadata_store_clone.set_setting("embedding_init_error", &error_msg);
                let _ = metadata_store_clone.set_setting("embedding_model_initialized", "error");
                tracing::error!("{}", error_msg);
            }
            Err(e) => {
                tracing::error!("Failed to join embedding model task: {}", e);
            }
        }
    });

    embedding_model
}

async fn init_vector_stores(
    lancedb_path: &Path,
) -> (Arc<VectorStore>, Arc<storage::InternalIndexStore>) {
    info!("Connecting to LanceDB...");
    let lancedb_uri = lancedb_path.to_str().expect("Invalid LanceDB path");

    let vector_store = Arc::new(
        VectorStore::new(lancedb_uri)
            .await
            .expect("Failed to initialize vector store"),
    );

    let internal_index_store = Arc::new(
        storage::InternalIndexStore::new(lancedb_uri)
            .await
            .expect("Failed to initialize internal index store"),
    );

    (vector_store, internal_index_store)
}

fn cleanup_interrupted_jobs(metadata_store: &Arc<MetadataStore>) {
    info!("Checking for interrupted jobs...");
    if let Ok(jobs) = metadata_store.get_jobs(None) {
        let interrupted_jobs: Vec<_> = jobs
            .iter()
            .filter(|j| {
                matches!(
                    j.status,
                    linggen_core::JobStatus::Pending | linggen_core::JobStatus::Running
                )
            })
            .collect();

        for job in interrupted_jobs {
            let reason = match job.status {
                linggen_core::JobStatus::Pending => {
                    "Server was restarted before job started (was waiting in queue)"
                }
                linggen_core::JobStatus::Running => "Server was restarted while job was running",
                _ => "Server was restarted",
            };

            let mut failed_job = job.clone();
            failed_job.status = linggen_core::JobStatus::Failed;
            failed_job.finished_at = Some(chrono::Utc::now().to_rfc3339());
            failed_job.error = Some(reason.to_string());
            let _ = metadata_store.update_job(&failed_job);
        }
    }
}

fn init_memory_store(base_data_dir: &Path) -> (PathBuf, PathBuf) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (linggen_dir, memory_dir) = resolve_memory_dirs(&cwd, base_data_dir);

    for nested in find_all_linggen_dirs(&cwd) {
        let nested_memory = nested.join("memory");
        if nested_memory.exists() && nested_memory != memory_dir {
            let _ = migrate_memory_files(&nested_memory, &memory_dir);
        }
    }
    info!("Using memory store at {:?}", memory_dir);
    (linggen_dir, memory_dir)
}

fn init_watchers(
    _app_state: &Arc<AppState>,
    internal_index_store: &Arc<storage::InternalIndexStore>,
    embedding_model: &Arc<RwLock<Option<EmbeddingModel>>>,
    chunker: &Arc<TextChunker>,
    broadcast_tx: &tokio::sync::broadcast::Sender<serde_json::Value>,
    metadata_store: &Arc<MetadataStore>,
    linggen_dir: &Path,
) {
    let internal_index_store = internal_index_store.clone();
    let embedding_model = embedding_model.clone();
    let chunker = chunker.clone();
    let broadcast_tx = broadcast_tx.clone();
    let metadata_store = metadata_store.clone();
    let linggen_dir = linggen_dir.to_path_buf();

    tokio::spawn(async move {
        let _ = crate::internal_indexer::start_internal_watcher(
            internal_index_store.clone(),
            embedding_model.clone(),
            chunker.clone(),
            broadcast_tx.clone(),
            "global".to_string(),
            linggen_dir,
        )
        .await;

        if let Ok(sources) = metadata_store.get_sources() {
            for source in sources {
                if source.source_type == linggen_core::SourceType::Local {
                    let project_path = std::path::PathBuf::from(&source.path);
                    let linggen_path = project_path.join(".linggen");

                    if linggen_path.exists() {
                        let _ = crate::internal_indexer::start_internal_watcher(
                            internal_index_store.clone(),
                            embedding_model.clone(),
                            chunker.clone(),
                            broadcast_tx.clone(),
                            source.id.clone(),
                            linggen_path,
                        )
                        .await;
                    }
                }
            }
        }
    });
}

fn create_router(app_state: Arc<AppState>) -> Router {
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers(tower_http::cors::Any);

    let upload_routes = Router::new()
        .route("/api/upload", post(upload_file))
        .route("/api/upload/stream", post(upload_file_stream))
        .route("/api/upload/files", post(list_uploaded_files))
        .route("/api/upload/delete", post(delete_uploaded_file))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(app_state.clone());

    let api_routes = Router::new()
        .route("/api/events", get(crate::handlers::events_handler))
        .route("/api/status", get(get_app_status))
        .route("/api/shutdown", post(shutdown_server))
        .route("/api/query", post(crate::handlers::search::search))
        .route("/api/search", post(crate::handlers::search::search))
        .route(
            "/api/memory/search",
            post(crate::handlers::memory::list_memories),
        )
        .route(
            "/api/memory/search_semantic",
            post(crate::handlers::memory_search_semantic),
        )
        .route(
            "/api/memory/fetch_by_meta",
            post(crate::handlers::memory::fetch_memory_by_meta),
        )
        .route(
            "/api/memory/read",
            get(crate::handlers::memory::read_memory),
        )
        .route(
            "/api/memory/create",
            post(crate::handlers::memory::create_memory),
        )
        .route(
            "/api/memory/update",
            post(crate::handlers::memory::update_memory),
        )
        .route(
            "/api/memory/delete",
            post(crate::handlers::memory::delete_memory),
        )
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs/cancel", post(cancel_job))
        .route("/api/resources", post(add_resource))
        .route("/api/resources", get(list_resources))
        .route("/api/resources/remove", post(remove_resource))
        .route("/api/resources/rename", post(rename_resource))
        .route("/api/resources/patterns", post(update_resource_patterns))
        .route("/api/library", get(list_library))
        .route("/api/library/packs", post(create_pack))
        .route("/api/library/packs/rename", post(rename_pack))
        .route(
            "/api/library/packs/*pack_id",
            get(get_pack).put(save_pack).delete(delete_pack),
        )
        .route("/api/library/folders", post(create_folder))
        .route("/api/library/folders/rename", post(rename_folder))
        .route("/api/library/folders/:folder_name", delete(delete_folder))
        .route("/api/library/download_skill", post(download_skill))
        .route("/api/skills_sh/search", get(crate::handlers::skills_sh::search_skills_sh))
        .route(
            "/api/settings",
            get(crate::handlers::settings::get_settings)
                .put(crate::handlers::settings::update_settings),
        )
        .route("/api/clear_all_data", post(clear_all_data))
        .route(
            "/api/sources/:source_id/notes",
            get(crate::handlers::notes::list_notes),
        )
        .route(
            "/api/sources/:source_id/notes/:note_path",
            get(crate::handlers::notes::get_note)
                .put(crate::handlers::notes::save_note)
                .delete(crate::handlers::notes::delete_note),
        )
        .route(
            "/api/sources/:source_id/notes/rename",
            post(crate::handlers::notes::rename_note),
        )
        .route(
            "/api/sources/:source_id/memory",
            get(crate::handlers::list_memory_files),
        )
        .route(
            "/api/sources/:source_id/memory/:file_path",
            get(crate::handlers::get_memory_file)
                .put(crate::handlers::save_memory_file)
                .delete(crate::handlers::delete_memory_file),
        )
        .route(
            "/api/sources/:source_id/memory/rename",
            post(crate::handlers::rename_memory_file),
        )
        .route(
            "/api/sources/:source_id/prompts",
            get(crate::handlers::list_prompts),
        )
        .route(
            "/api/sources/:source_id/prompts/:file_path",
            get(crate::handlers::get_prompt)
                .put(crate::handlers::save_prompt)
                .delete(crate::handlers::delete_prompt),
        )
        .route(
            "/api/sources/:source_id/prompts/rename",
            post(crate::handlers::rename_prompt),
        )
        .route(
            "/api/sources/:source_id/internal/rescan",
            post(crate::handlers::rescan_internal_index),
        )
        .with_state(app_state);

    let combined_routes = api_routes
        .merge(upload_routes)
        .layer(cors)
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024));

    combined_routes
        .route("/", get(index_handler))
        .route("/*path", get(static_handler))
}

async fn root_handler() -> &'static str {
    "Hello from Linggen Backend!"
}

fn is_writable_dir(path: &Path) -> bool {
    // We consider a directory "writable" if we can create it (if missing) and create a temp file.
    if std::fs::create_dir_all(path).is_err() {
        return false;
    }
    let probe = path.join(".linggen_write_probe");
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn resolve_memory_dirs(cwd: &Path, base_data_dir: &Path) -> (PathBuf, PathBuf) {
    // 1. Check for an explicit workspace/repo '.linggen' directory first.
    let candidate_linggen_dir = find_workspace_linggen_dir(cwd);
    let candidate_linggen_dir =
        std::fs::canonicalize(&candidate_linggen_dir).unwrap_or(candidate_linggen_dir);

    let home = dirs::home_dir().and_then(|h| std::fs::canonicalize(h).ok());

    // Only use the workspace candidate if:
    // - It has a parent (not root)
    // - It's writable
    // - It is NOT exactly $HOME/.linggen (we want to use Application Support for global data)
    let is_home_linggen = home
        .map(|h| candidate_linggen_dir == h.join(".linggen"))
        .unwrap_or(false);

    if candidate_linggen_dir.parent().is_some()
        && is_writable_dir(&candidate_linggen_dir)
        && !is_home_linggen
    {
        let memory_dir = candidate_linggen_dir.join("memory");
        return (candidate_linggen_dir, memory_dir);
    }

    // 2. Fallback: Use the stable app data directory (~/Library/Application Support/Linggen/.linggen)
    let fallback_linggen_dir = base_data_dir.join(".linggen");
    let fallback_memory_dir = fallback_linggen_dir.join("memory");

    (fallback_linggen_dir, fallback_memory_dir)
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        if cur.join(".git").is_dir() {
            return Some(cur.to_path_buf());
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return None,
        }
    }
}

/// Find the workspace `.linggen/` directory to use for memory.
///
/// Priority:
/// 1) Git repo root (if any): `<git_root>/.linggen` (created if missing)
/// 2) Nearest `.linggen` under `start` ancestry, but **do not** pick `$HOME/.linggen` as the project store.
fn find_workspace_linggen_dir(start: &Path) -> PathBuf {
    if let Some(git_root) = find_git_root(start) {
        return git_root.join(".linggen");
    }

    // Fall back to nearest `.linggen` but stop before reaching HOME.
    let home = dirs::home_dir();
    let mut cur = start;
    loop {
        let candidate = cur.join(".linggen");
        if candidate.is_dir() {
            return candidate;
        }
        match cur.parent() {
            Some(parent) => {
                if let Some(ref h) = home {
                    if parent == h.as_path() {
                        break;
                    }
                }
                cur = parent;
            }
            None => break,
        }
    }

    // Default: `.linggen` in current working directory
    start.join(".linggen")
}

fn find_all_linggen_dirs(start: &Path) -> Vec<PathBuf> {
    let mut cur = start;
    let mut dirs: Vec<PathBuf> = Vec::new();
    // Stop at git root if present (to avoid pulling in $HOME/.linggen)
    let stop_at = find_git_root(start);
    loop {
        let candidate = cur.join(".linggen");
        if candidate.is_dir() {
            dirs.push(candidate);
        }
        if let Some(ref stop) = stop_at {
            if cur == stop.as_path() {
                break;
            }
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => break,
        }
    }
    dirs
}

fn migrate_memory_files(from_dir: &Path, to_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to_dir)?;
    for entry in std::fs::read_dir(from_dir)? {
        let entry = entry?;
        let src = entry.path();
        if src.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let file_name = match src.file_name() {
            Some(f) => f,
            None => continue,
        };
        let mut dst = to_dir.join(file_name);
        if dst.exists() {
            // Avoid overwriting: prefix with "migrated_"
            dst = to_dir.join(format!("migrated_{}", file_name.to_string_lossy()));
        }
        std::fs::rename(&src, &dst)?;
    }
    // Try to clean up empty legacy dirs (best effort).
    let _ = std::fs::remove_dir(from_dir);
    if let Some(parent) = from_dir.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}
