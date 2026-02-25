use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

fn pid_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".linggen/ling-mem.pid")
}

fn log_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".linggen/ling-mem.log")
}

fn is_process_running(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn is_port_listening(port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

pub async fn start(port: u16) -> Result<()> {
    if is_port_listening(port).await {
        println!("Memory server already running on port {}", port);
        return Ok(());
    }

    let pid_path = pid_file();
    if let Some(parent) = pid_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let exe = std::env::current_exe().context("Failed to get current executable path")?;
    let log = log_file();

    let log_out = fs::File::create(&log).context("Failed to create daemon log file")?;
    let log_err = log_out.try_clone()?;

    let child = std::process::Command::new(&exe)
        .args(["--port", &port.to_string()])
        .stdout(log_out)
        .stderr(log_err)
        .stdin(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .context("Failed to spawn daemon process")?;

    let pid = child.id();
    fs::write(&pid_path, pid.to_string())?;

    // Poll until ready (30 x 100ms = 3s)
    let mut ready = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if is_port_listening(port).await {
            ready = true;
            break;
        }
    }

    if ready {
        println!("Memory server started on http://localhost:{} (PID {})", port, pid);
    } else {
        println!(
            "Memory server spawned (PID {}) but not yet reachable on port {}",
            pid, port
        );
        println!("Check logs at {}", log.display());
    }

    Ok(())
}

pub async fn stop() -> Result<()> {
    let pid_path = pid_file();
    let pid = match fs::read_to_string(&pid_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    {
        Some(p) => p,
        None => {
            println!("Memory server: no PID file found; may not be running.");
            return Ok(());
        }
    };

    if !is_process_running(pid) {
        println!("Memory server: process {} is not running. Cleaning up PID file.", pid);
        let _ = fs::remove_file(&pid_path);
        return Ok(());
    }

    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();

    tokio::time::sleep(Duration::from_millis(500)).await;

    if is_process_running(pid) {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let _ = fs::remove_file(&pid_path);
    println!("Memory server stopped (PID {})", pid);
    Ok(())
}

pub async fn status(port: u16) -> Result<()> {
    println!("ling-mem status\n");

    println!("  Version:   v{}", env!("CARGO_PKG_VERSION"));
    println!("  Port:      {}", port);

    let listening = is_port_listening(port).await;
    let pid = fs::read_to_string(pid_file())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());

    match (listening, pid) {
        (true, Some(pid)) => println!("  Status:    running (PID {})", pid),
        (true, None) => println!("  Status:    running"),
        (false, Some(pid)) => {
            if is_process_running(pid) {
                println!("  Status:    process alive (PID {}) but port not listening", pid);
            } else {
                println!("  Status:    not running (stale PID)");
            }
        }
        (false, None) => println!("  Status:    not running"),
    }

    // Data directory
    let data_dir = dirs::data_dir()
        .map(|d| d.join("Linggen"))
        .unwrap_or_else(|| PathBuf::from("~/.local/share/Linggen"));
    if data_dir.exists() {
        println!("  Data dir:  {}", data_dir.display());
    } else {
        println!("  Data dir:  {} (not created yet)", data_dir.display());
    }

    // If server is running, try to get source count
    if listening {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
        {
            let url = format!("http://127.0.0.1:{}/api/resources", port);
            if let Ok(resp) = client.get(&url).send().await {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if let Some(resources) = body.get("resources").and_then(|r| r.as_array()) {
                        println!("  Sources:   {}", resources.len());
                    }
                }
            }
        }
    }

    println!();
    Ok(())
}
