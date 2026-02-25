use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

// ---------------------------------------------------------------------------
// HTTP client helpers (lightweight, no dependency on linggen-agent)
// ---------------------------------------------------------------------------

struct MemClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct CreateSourceRequest {
    name: String,
    resource_type: String,
    path: String,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
}

#[derive(Deserialize)]
struct CreateSourceResponse {
    id: String,
}

#[derive(Deserialize)]
struct ListSourcesResponse {
    resources: Vec<SourceEntry>,
}

#[derive(Deserialize)]
struct SourceEntry {
    id: String,
    name: String,
    resource_type: String,
    path: String,
    stats: Option<SourceStats>,
}

#[derive(Deserialize)]
struct SourceStats {
    chunk_count: usize,
    file_count: usize,
}

#[derive(Serialize)]
struct IndexRequest {
    source_id: String,
    mode: String,
}

#[derive(Deserialize)]
struct IndexResponse {
    job_id: String,
}

#[derive(Deserialize)]
struct ListJobsResponse {
    jobs: Vec<JobEntry>,
}

#[derive(Deserialize)]
struct JobEntry {
    id: String,
    status: String,
    files_indexed: Option<usize>,
    total_files: Option<usize>,
    chunks_created: Option<usize>,
    error: Option<String>,
}

impl MemClient {
    fn new(base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        Self { base_url, client }
    }

    async fn check_health(&self) -> Result<()> {
        self.client
            .get(format!("{}/api/status", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn list_sources(&self) -> Result<ListSourcesResponse> {
        Ok(self
            .client
            .get(format!("{}/api/resources", self.base_url))
            .send()
            .await?
            .json()
            .await?)
    }

    async fn create_source(&self, req: CreateSourceRequest) -> Result<CreateSourceResponse> {
        Ok(self
            .client
            .post(format!("{}/api/resources", self.base_url))
            .json(&req)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn index_source(&self, source_id: &str, mode: &str) -> Result<IndexResponse> {
        Ok(self
            .client
            .post(format!("{}/api/index_source", self.base_url))
            .json(&IndexRequest {
                source_id: source_id.to_string(),
                mode: mode.to_string(),
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn list_jobs(&self) -> Result<ListJobsResponse> {
        Ok(self
            .client
            .get(format!("{}/api/jobs", self.base_url))
            .send()
            .await?
            .json()
            .await?)
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn logical_current_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(pwd_os) = std::env::var_os("PWD") else {
        return cwd;
    };
    let pwd = PathBuf::from(pwd_os);
    if !pwd.is_absolute() {
        return cwd;
    }
    match (cwd.canonicalize(), pwd.canonicalize()) {
        (Ok(cwd_can), Ok(pwd_can)) if cwd_can == pwd_can => pwd,
        _ => cwd,
    }
}

fn logical_absolute(path: &PathBuf) -> PathBuf {
    let p = if path.is_absolute() {
        path.clone()
    } else {
        logical_current_dir().join(path)
    };

    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }

    if out.as_os_str().is_empty() {
        logical_current_dir()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Wait for job
// ---------------------------------------------------------------------------

async fn wait_for_job(client: &MemClient, job_id: &str) -> Result<()> {
    let mut last_status = String::new();
    let mut last_progress = (0usize, 0usize);

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let jobs = client.list_jobs().await?;
        let job = jobs.jobs.iter().find(|j| j.id == job_id);

        match job {
            Some(job) => {
                let current_progress = (
                    job.files_indexed.unwrap_or(0),
                    job.total_files.unwrap_or(0),
                );
                let status_changed = job.status != last_status;
                let progress_changed = current_progress != last_progress;

                match job.status.as_str() {
                    "Pending" => {
                        if status_changed {
                            println!("   Status: Pending...");
                        }
                    }
                    "Running" => {
                        if status_changed || progress_changed {
                            let (indexed, total) = current_progress;
                            if total > 0 {
                                let pct = (indexed as f64 / total as f64 * 100.0) as u32;
                                println!(
                                    "   Progress: {}/{} files ({}%) - {} chunks",
                                    indexed,
                                    total,
                                    pct,
                                    job.chunks_created.unwrap_or(0)
                                );
                            } else {
                                println!("   Status: Running - processing...");
                            }
                        }
                    }
                    "Completed" => {
                        if status_changed {
                            println!("\nJob completed successfully!");
                            if let Some(files) = job.files_indexed {
                                println!("   Files indexed: {}", files);
                            }
                            if let Some(chunks) = job.chunks_created {
                                println!("   Chunks created: {}", chunks);
                            }
                        }
                        return Ok(());
                    }
                    "Failed" => {
                        if status_changed {
                            println!("\nJob failed");
                            if let Some(error) = &job.error {
                                println!("   Error: {}", error);
                            }
                        }
                        anyhow::bail!("Indexing job failed");
                    }
                    _ => {
                        if status_changed {
                            println!("   Status: {}", job.status);
                        }
                    }
                }

                last_status = job.status.clone();
                last_progress = current_progress;
            }
            None => {
                println!("Job not found");
                break;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Index command
// ---------------------------------------------------------------------------

pub async fn run(
    server_url: &str,
    path: Option<PathBuf>,
    mode: String,
    name: Option<String>,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
    wait: bool,
) -> Result<()> {
    let client = MemClient::new(server_url.to_string());

    if client.check_health().await.is_err() {
        anyhow::bail!(
            "Memory server not reachable at {}. Start it with: ling-mem -d",
            server_url
        );
    }

    let path = path.unwrap_or_else(|| PathBuf::from("."));
    let display_abs_path = logical_absolute(&path);
    let abs_path = display_abs_path
        .canonicalize()
        .with_context(|| format!("Invalid path: {}", display_abs_path.display()))?;

    let abs_path_str = abs_path
        .to_str()
        .context("Path contains invalid UTF-8")?
        .to_string();
    let display_path_str = display_abs_path
        .to_str()
        .context("Path contains invalid UTF-8")?
        .to_string();

    println!("Indexing: {}", display_abs_path.display());

    // Check if source already exists
    let sources = client.list_sources().await?;
    let existing_source = sources.resources.iter().find(|s| {
        if s.resource_type != "local" {
            return false;
        }
        if s.path == display_path_str || s.path == abs_path_str {
            return true;
        }
        PathBuf::from(&s.path)
            .canonicalize()
            .map(|p| p == abs_path)
            .unwrap_or(false)
    });

    let (source_id, is_previously_indexed) = if let Some(source) = existing_source {
        println!("   Found existing source: {}", source.name);

        let has_stats = source
            .stats
            .as_ref()
            .map(|s| s.chunk_count > 0)
            .unwrap_or(false);

        if has_stats {
            let stats = source.stats.as_ref().unwrap();
            println!(
                "   Previously indexed: {} files, {} chunks",
                stats.file_count, stats.chunk_count
            );
        }

        (source.id.clone(), has_stats)
    } else {
        let source_name = name.unwrap_or_else(|| {
            abs_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unnamed")
                .to_string()
        });

        println!("   Creating new source: {}", source_name);

        let req = CreateSourceRequest {
            name: source_name,
            resource_type: "local".to_string(),
            path: display_path_str,
            include_patterns: include_patterns.clone(),
            exclude_patterns: exclude_patterns.clone(),
        };

        let response = client.create_source(req).await?;
        println!("   Source created: {}", response.id);
        (response.id, false)
    };

    // Determine indexing mode
    let mode = mode.to_lowercase();
    let final_mode = match mode.as_str() {
        "auto" => {
            if is_previously_indexed {
                println!("   Mode: auto -> incremental (source already indexed)");
                "incremental"
            } else {
                println!("   Mode: auto -> full (first-time indexing)");
                "full"
            }
        }
        "full" => "full",
        "incremental" => "incremental",
        _ => {
            anyhow::bail!(
                "Invalid mode: {}. Use 'auto', 'full', or 'incremental'",
                mode
            );
        }
    };

    if final_mode == "incremental" && !is_previously_indexed {
        println!("   Warning: Using incremental mode on a new source (no previous index found)");
    }

    println!("Starting {} indexing...", final_mode);

    let response = client.index_source(&source_id, final_mode).await?;
    println!("Indexing job started (Job ID: {})", response.job_id);

    if wait {
        println!("Waiting for job to complete...");
        wait_for_job(&client, &response.job_id).await?;
    } else {
        println!("Use `ling-mem status` to check indexing progress");
    }

    Ok(())
}
