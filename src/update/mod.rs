//! Self-update for the `ling-mem` binary.
//!
//! Two callable surfaces:
//!
//! | Function | Used by |
//! |:---------|:--------|
//! | [`check`] / [`check_quiet`] | `ling-mem update --check`, and (cached) `ling-mem start` |
//! | [`apply`]                   | `ling-mem update [--yes]` |
//!
//! Release source: `linggen/linggen-memory` GitHub releases. Tag pattern
//! `vX.Y.Z`. Asset layout per release (see `scripts/release.sh`):
//!
//! ```text
//! ling-mem-<slug>.tar.gz          (+ .sha256 sibling)
//! ```
//!
//! Slug values: `macos-aarch64`, `macos-x86_64`, `linux-x86_64`,
//! `linux-aarch64`. The tarball contains a flat `ling-mem` binary plus
//! `README.md` and `LICENSE`.
//!
//! Update flow:
//! 1. Resolve current platform → asset name.
//! 2. Hit `releases/latest`; pick the matching `.tar.gz` + `.sha256`.
//! 3. Download tarball + checksum to a temp dir under the binary's parent.
//! 4. Verify SHA256 (streaming, in-process — no `sha2` crate dep).
//! 5. Extract via `tar -xzf` (already a release-pipeline dep).
//! 6. Stop the daemon if running, atomic-rename the new binary into place
//!    (keeping the prior at `<bin-dir>/ling-mem.prev`), then restart by
//!    explicitly invoking the new binary path so the running (old) process
//!    doesn't relaunch its own inode.
//!
//! Network errors during `--check` are swallowed when used from `start`
//! (`check_quiet`) — `start` should never fail just because GitHub is down.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPO: &str = "linggen/linggen-memory";
const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/linggen/linggen-memory/releases/latest";
const USER_AGENT: &str = concat!("ling-mem/", env!("CARGO_PKG_VERSION"));
const CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const NETWORK_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Result of an update probe — what `--check` prints, and what `start`
/// embeds in its lifecycle JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub current: String,
    pub latest: Option<String>,
    pub url: Option<String>,
    pub notes_summary: Option<String>,
    /// Set when the platform isn't releasable (e.g. unsupported arch) or
    /// when the latest release lacks an asset for our platform. The CLI
    /// surfaces this so the user knows why the update path is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsupported: Option<String>,
}

impl UpdateInfo {
    fn current_only() -> Self {
        Self {
            available: false,
            current: env!("CARGO_PKG_VERSION").to_string(),
            latest: None,
            url: None,
            notes_summary: None,
            unsupported: None,
        }
    }

    fn unsupported(reason: impl Into<String>) -> Self {
        let mut info = Self::current_only();
        info.unsupported = Some(reason.into());
        info
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    fetched_at: u64,
    info: UpdateInfo,
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(".ling-mem-update-cache.json")
}

fn read_cache(data_dir: &Path) -> Option<UpdateInfo> {
    let raw = fs::read(cache_path(data_dir)).ok()?;
    let entry: CacheEntry = serde_json::from_slice(&raw).ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if now.saturating_sub(entry.fetched_at) > CACHE_TTL_SECS {
        return None;
    }
    // Cache is keyed on the binary's own version. If the running binary
    // is newer than what was cached (just upgraded), discard.
    if entry.info.current != env!("CARGO_PKG_VERSION") {
        return None;
    }
    Some(entry.info)
}

fn write_cache(data_dir: &Path, info: &UpdateInfo) {
    let entry = CacheEntry {
        fetched_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        info: info.clone(),
    };
    if let Some(parent) = cache_path(data_dir).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        cache_path(data_dir),
        serde_json::to_vec(&entry).unwrap_or_default(),
    );
}

/// Slug used in release asset names — must match
/// `scripts/lib-common.sh::detect_platform`.
fn platform_slug() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-aarch64"),
        ("macos", "x86_64") => Some("macos-x86_64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        _ => None,
    }
}

fn asset_name(slug: &str) -> String {
    format!("ling-mem-{slug}.tar.gz")
}

/// Cached version probe. Falls through to a network call on cache miss.
/// `bypass_cache=true` always hits the network.
pub async fn check(data_dir: &Path, bypass_cache: bool) -> Result<UpdateInfo> {
    let Some(slug) = platform_slug() else {
        return Ok(UpdateInfo::unsupported(format!(
            "no release asset for {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
    };

    if !bypass_cache {
        if let Some(cached) = read_cache(data_dir) {
            return Ok(cached);
        }
    }

    let info = fetch_latest(slug).await.context("update check failed")?;
    write_cache(data_dir, &info);
    Ok(info)
}

/// Best-effort version probe used during `start`. Network failures are
/// swallowed — `start` should never fail just because GitHub is down.
pub async fn check_quiet(data_dir: &Path) -> UpdateInfo {
    match check(data_dir, false).await {
        Ok(info) => info,
        Err(_) => UpdateInfo::current_only(),
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
}

async fn fetch_latest(slug: &str) -> Result<UpdateInfo> {
    let client = reqwest::Client::builder()
        .timeout(NETWORK_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()?;

    let mut req = client.get(RELEASES_LATEST_URL);
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            req = req.bearer_auth(token);
        }
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("GitHub returned {}", resp.status()));
    }
    let release: Release = resp.json().await?;

    let latest_ver = release.tag_name.trim_start_matches('v').to_string();
    let current = env!("CARGO_PKG_VERSION");

    let asset = asset_name(slug);
    let asset_match = release.assets.iter().any(|a| a.name == asset);

    let available = asset_match && version_lt(current, &latest_ver);
    let notes_summary = release
        .body
        .as_deref()
        .map(headline)
        .filter(|s| !s.is_empty());

    Ok(UpdateInfo {
        available,
        current: current.to_string(),
        latest: Some(latest_ver),
        url: Some(release.html_url),
        notes_summary,
        unsupported: if asset_match {
            None
        } else {
            Some(format!("no `{asset}` in latest release"))
        },
    })
}

/// First non-empty line of release notes, trimmed and bounded.
fn headline(body: &str) -> String {
    body.lines()
        .map(|l| l.trim_start_matches(|c: char| c == '#' || c.is_whitespace()))
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| {
            if l.chars().count() > 200 {
                let head: String = l.chars().take(199).collect();
                format!("{head}…")
            } else {
                l.to_string()
            }
        })
        .unwrap_or_default()
}

/// Naive semver compare. Strips any pre-release / build suffix at the
/// first non-`.`/non-digit char (so `0.3.0-rc.1` collapses to `0.3.0`),
/// then splits on `.` and compares numerically. The release pipeline only
/// ever tags `vX.Y.Z`, so this is sufficient — we just need to not crash
/// on the rare hand-edited tag.
fn version_lt(current: &str, latest: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        let core: String = v
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        core.split('.')
            .map(|s| s.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let a = parse(current);
    let b = parse(latest);
    let len = a.len().max(b.len());
    for i in 0..len {
        let ai = a.get(i).copied().unwrap_or(0);
        let bi = b.get(i).copied().unwrap_or(0);
        if ai != bi {
            return ai < bi;
        }
    }
    false
}

/// Outcome of an `apply` call — rendered as JSON to stdout.
#[derive(Debug, Serialize)]
pub struct UpdateOutcome {
    pub updated: bool,
    pub from: String,
    pub to: String,
    pub restarted: bool,
    /// Set when no swap happened (e.g. already-current).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub struct ApplyOptions<'a> {
    pub data_dir: &'a Path,
    pub skill_dir: &'a Path,
    pub port: u16,
    pub force: bool,
}

/// Run a real update: download, verify, swap, optionally restart.
pub async fn apply(opts: ApplyOptions<'_>) -> Result<UpdateOutcome> {
    let info = check(opts.data_dir, true).await?;
    let current = env!("CARGO_PKG_VERSION").to_string();

    if let Some(reason) = &info.unsupported {
        return Err(anyhow!("cannot update: {reason}"));
    }

    let Some(latest) = info.latest.clone() else {
        return Err(anyhow!("update check returned no latest version"));
    };

    if !info.available && !opts.force {
        return Ok(UpdateOutcome {
            updated: false,
            from: current.clone(),
            to: current,
            restarted: false,
            note: Some("already on latest version".to_string()),
        });
    }

    let slug = platform_slug().ok_or_else(|| anyhow!("unsupported platform"))?;
    let exe = std::env::current_exe().context("resolving current executable path")?;
    let bin_dir = exe
        .parent()
        .ok_or_else(|| anyhow!("binary has no parent directory"))?
        .to_path_buf();

    refuse_managed_path(&bin_dir)?;

    let tag = format!("v{latest}");
    let asset = asset_name(slug);
    let download_url = format!("https://github.com/{REPO}/releases/download/{tag}/{asset}");
    let sha_url = format!("{download_url}.sha256");

    let staging = StagingDir::create_under(&bin_dir)?;
    let tarball_path = staging.path().join(&asset);
    let sha_path = staging.path().join(format!("{asset}.sha256"));

    download_to(&download_url, &tarball_path)
        .await
        .with_context(|| format!("downloading {download_url}"))?;
    download_to(&sha_url, &sha_path)
        .await
        .with_context(|| format!("downloading {sha_url}"))?;

    verify_sha256(&tarball_path, &sha_path)?;

    let extracted = extract_binary(&tarball_path, staging.path())?;

    let was_running = stop_daemon_if_running(opts.skill_dir).await?;

    let new_canonical = bin_dir.join("ling-mem");
    swap_binary(&extracted, &new_canonical, &bin_dir)?;

    let restarted = if was_running {
        // Spawn the *new* binary explicitly. Using `current_exe()` here is
        // unsafe: on Linux `/proc/self/exe` follows the inode, which is now
        // at `ling-mem.prev`, so we'd relaunch the old version.
        spawn_new_daemon(&new_canonical, opts.data_dir, opts.port)?;
        true
    } else {
        false
    };

    // Best-effort cache invalidation so future `--check` calls reflect reality.
    let _ = fs::remove_file(cache_path(opts.data_dir));

    Ok(UpdateOutcome {
        updated: true,
        from: current,
        to: latest,
        restarted,
        note: None,
    })
}

fn refuse_managed_path(bin_dir: &Path) -> Result<()> {
    let s = bin_dir.to_string_lossy();
    let managed = [
        ("/usr/local/bin", "homebrew or system"),
        ("/usr/local/Cellar", "homebrew"),
        ("/opt/homebrew", "homebrew"),
        ("/usr/bin", "system"),
        ("/opt/local", "macports"),
    ];
    for (prefix, kind) in managed {
        if s.starts_with(prefix) {
            return Err(anyhow!(
                "binary lives under {prefix} ({kind}-managed) — use that package manager to update"
            ));
        }
    }
    Ok(())
}

async fn download_to(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {} for {url}", resp.status()));
    }
    let bytes = resp.bytes().await?;
    fs::write(dest, &bytes).with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

fn verify_sha256(file: &Path, sha_file: &Path) -> Result<()> {
    use std::io::Read;
    let raw = fs::read_to_string(sha_file).context("reading sha256 file")?;
    // `shasum -a 256` format: "<hex>  <filename>"
    let expected = raw
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("empty sha256 file"))?
        .to_lowercase();
    if expected.len() != 64 {
        return Err(anyhow!("malformed sha256: {expected:?}"));
    }

    let mut hasher = Sha256::new();
    let mut f = fs::File::open(file).context("opening tarball for hashing")?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hasher.hex();
    if actual != expected {
        return Err(anyhow!(
            "sha256 mismatch: expected {expected}, got {actual}"
        ));
    }
    Ok(())
}

/// Extract the `ling-mem` binary from the tarball into `into/ling-mem.new`,
/// chmod 755, and sanity-check `--version`. Uses `tar` from the host (the
/// release pipeline already requires it).
fn extract_binary(tarball: &Path, into: &Path) -> Result<PathBuf> {
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(into)
        .status()
        .context("spawning `tar -xzf`")?;
    if !status.success() {
        return Err(anyhow!("`tar -xzf` exited with {status}"));
    }

    let extracted_at_top = into.join("ling-mem");
    if !extracted_at_top.is_file() {
        return Err(anyhow!(
            "expected `ling-mem` at top level of tarball; not found in {}",
            into.display()
        ));
    }

    // Move to a `.new` name so the extracted-from-tar inode is what we
    // ultimately rename into place.
    let new_path = into.join("ling-mem.new");
    fs::rename(&extracted_at_top, &new_path)
        .with_context(|| format!("renaming staged binary to {}", new_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&new_path)?.permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&new_path, perm)?;
    }

    let probe = std::process::Command::new(&new_path)
        .arg("--version")
        .output()
        .with_context(|| format!("running {} --version", new_path.display()))?;
    if !probe.status.success() {
        return Err(anyhow!(
            "extracted binary --version exited with {}",
            probe.status
        ));
    }
    let stdout = String::from_utf8_lossy(&probe.stdout);
    if !stdout.starts_with("ling-mem ") {
        return Err(anyhow!(
            "extracted binary --version unexpected output: {stdout:?}"
        ));
    }

    Ok(new_path)
}

async fn stop_daemon_if_running(skill_dir: &Path) -> Result<bool> {
    use crate::daemon::lifecycle::{stop, LifecycleOutcome};
    match stop(skill_dir).await? {
        LifecycleOutcome::Stopped { .. } => Ok(true),
        LifecycleOutcome::NotRunning => Ok(false),
        // `stop` only ever returns Stopped or NotRunning.
        _ => Ok(false),
    }
}

/// Atomic-rename: move the running binary aside (rollback copy), then put
/// the new binary at its canonical path. On failure mid-way, restore.
fn swap_binary(new_bin: &Path, current_canonical: &Path, bin_dir: &Path) -> Result<()> {
    let prev = bin_dir.join("ling-mem.prev");
    if prev.exists() {
        let _ = fs::remove_file(&prev);
    }
    if current_canonical.exists() {
        fs::rename(current_canonical, &prev).with_context(|| {
            format!(
                "moving {} → {} (rollback copy)",
                current_canonical.display(),
                prev.display()
            )
        })?;
    }
    if let Err(e) = fs::rename(new_bin, current_canonical) {
        // Best-effort restore.
        let _ = fs::rename(&prev, current_canonical);
        return Err(anyhow!(
            "installing new binary at {}: {e}",
            current_canonical.display()
        ));
    }
    Ok(())
}

/// Spawn the new binary's `start` subcommand directly, so we don't relaunch
/// our own (now-renamed) inode. Wait briefly for it to print its lifecycle
/// JSON and exit; if anything goes wrong, surface the stderr.
fn spawn_new_daemon(new_bin: &Path, data_dir: &Path, port: u16) -> Result<()> {
    let out = std::process::Command::new(new_bin)
        .arg("start")
        .arg("--port")
        .arg(port.to_string())
        .env("LINGGEN_DATA_DIR", data_dir)
        .output()
        .with_context(|| format!("spawning {} start", new_bin.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!(
            "new binary failed to start daemon (exit {}): {}",
            out.status,
            stderr.trim()
        ));
    }
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────────

struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    fn create_under(base: &Path) -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = base.join(format!(".ling-mem-update-{nanos}"));
        fs::create_dir_all(&dir).with_context(|| format!("creating staging {}", dir.display()))?;
        Ok(Self { path: dir })
    }
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

// SHA-256 — small embedded implementation so we don't add a `sha2` dep just
// for one verify. Pure software, ~80 lines, fine for a 100MB tarball.
struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c,
                0x1f83_d9ab, 0x5be0_cd19,
            ],
            buf: [0; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1,
            0x923f_82a4, 0xab1c_5ed5, 0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3,
            0x72be_5d74, 0x80de_b1fe, 0x9bdc_06a7, 0xc19b_f174, 0xe49b_69c1, 0xefbe_4786,
            0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f, 0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da,
            0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7, 0xc6e0_0bf3, 0xd5a7_9147,
            0x06ca_6351, 0x1429_2967, 0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc, 0x5338_0d13,
            0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85, 0xa2bf_e8a1, 0xa81a_664b,
            0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070,
            0x19a4_c116, 0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a,
            0x5b9c_ca4f, 0x682e_6ff3, 0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208,
            0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7, 0xc671_78f2,
        ];
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        // 0x80 byte, then zeros, then big-endian 64-bit length.
        self.update(&[0x80]);
        while self.buf_len != 56 {
            self.update(&[0]);
        }
        self.update(&bit_len.to_be_bytes());
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn hex(self) -> String {
        let bytes = self.finalize();
        let mut s = String::with_capacity(64);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_lt_basic() {
        assert!(version_lt("0.2.0", "0.2.1"));
        assert!(version_lt("0.2.1", "0.3.0"));
        assert!(version_lt("0.9.0", "1.0.0"));
        assert!(!version_lt("0.2.1", "0.2.1"));
        assert!(!version_lt("0.3.0", "0.2.9"));
    }

    #[test]
    fn version_lt_strips_suffix() {
        // Pre-release / build suffixes collapse to the numeric prefix, so a
        // bare X.Y.Z is considered equal to X.Y.Z-rc.N for our comparison.
        // Acceptable because release.sh only ever tags plain vX.Y.Z.
        assert!(!version_lt("0.3.0", "0.3.0-rc.1"));
        assert!(version_lt("0.2.9", "0.3.0-rc.1"));
    }

    #[test]
    fn headline_picks_first_nonempty_line() {
        assert_eq!(headline(""), "");
        assert_eq!(headline("\n\n  ## Highlights\nbody\n"), "Highlights");
        assert_eq!(headline("First line.\nSecond."), "First line.");
    }

    #[test]
    fn sha256_known_vectors() {
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            h.hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let h = Sha256::new();
        assert_eq!(
            h.hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
