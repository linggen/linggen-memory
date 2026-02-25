use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct ReleaseManifest {
    version: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    url: String,
}

/// Returns asset name matching the release script convention, e.g. "ling-mem-macos-aarch64".
fn platform_asset_name() -> String {
    let slug = if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "macos-aarch64"
        } else {
            "macos-x86_64"
        }
    } else if cfg!(target_os = "linux") {
        if cfg!(target_arch = "aarch64") {
            "linux-aarch64"
        } else {
            "linux-x86_64"
        }
    } else {
        "unknown"
    };

    format!("ling-mem-{}", slug)
}

pub async fn run() -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    println!("Current ling-mem version: v{}", current_version);

    let client = reqwest::Client::builder()
        .user_agent("ling-mem")
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;

    let manifest_url =
        "https://github.com/linggen/linggen-memory/releases/latest/download/manifest.json";

    let manifest = match client.get(manifest_url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<ReleaseManifest>().await {
            Ok(m) => m,
            Err(e) => {
                println!("Failed to parse release manifest: {}", e);
                println!("No releases available yet.");
                return Ok(());
            }
        },
        Ok(resp) => {
            println!(
                "Release manifest returned HTTP {}. No releases available yet.",
                resp.status()
            );
            return Ok(());
        }
        Err(e) => {
            println!("Failed to fetch release manifest: {}", e);
            println!("No releases available yet.");
            return Ok(());
        }
    };

    if manifest.version == current_version {
        println!("Already up to date (v{}).", current_version);
        return Ok(());
    }

    let asset_name = platform_asset_name();
    let asset = match manifest.assets.iter().find(|a| a.name == asset_name) {
        Some(a) => a,
        None => {
            println!(
                "No release asset for '{}'. Available: {}",
                asset_name,
                manifest
                    .assets
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return Ok(());
        }
    };

    println!("Downloading v{} ...", manifest.version);

    let resp = client
        .get(&asset.url)
        .send()
        .await
        .context("Failed to download release asset")?;

    if !resp.status().is_success() {
        anyhow::bail!("Download failed with HTTP {}", resp.status());
    }

    let bytes = resp.bytes().await.context("Failed to read download")?;

    let current_exe = std::env::current_exe().context("Failed to get current executable path")?;
    let parent = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Failed to get executable parent directory"))?;

    // The release asset is a .tar.gz containing the binary — extract it
    let temp_dir = parent.join(format!(".ling-mem-extract-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);
    let tarball_path = temp_dir.join("download.tar.gz");
    std::fs::write(&tarball_path, &bytes).context("Failed to write temp tarball")?;

    let output = std::process::Command::new("tar")
        .args(["xzf", &tarball_path.to_string_lossy(), "-C", &temp_dir.to_string_lossy()])
        .output()
        .context("Failed to run tar to extract binary")?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        anyhow::bail!(
            "Failed to extract tarball: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let extracted_binary = temp_dir.join("ling-mem");
    if !extracted_binary.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        anyhow::bail!("Binary not found in tarball");
    }

    std::fs::rename(&extracted_binary, &current_exe).context("Failed to replace binary")?;
    let _ = std::fs::remove_dir_all(&temp_dir);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&current_exe, std::fs::Permissions::from_mode(0o755))?;
    }

    println!("Updated v{} -> v{}", current_version, manifest.version);

    Ok(())
}
