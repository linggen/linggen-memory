#!/usr/bin/env bash
set -euo pipefail

# Bash-only guard (avoid fish/zsh sourcing issues)
if [ -n "${FISH_VERSION:-}" ]; then
  echo "This script is bash-only. Run: bash install.sh"
  exit 1
fi

INSTALL_DIR_DEFAULT="/usr/local/bin"
FALLBACK_DIR="$HOME/.local/bin"

# ling-mem binary version spec. Linggen depends on the ling-mem binary
# (inline memory capture, dream mission, core inject, Memory_* tools).
# A semver RANGE, not an exact tag: install-bin.sh resolves it to the
# highest matching release, so patch/minor ling-mem upgrades flow without
# a re-pin. The store's schema-version guard (linggen-memory/doc/
# schema-versioning.md) makes that data-safe — a compatible subversion
# opens the same store; an incompatible one refuses rather than corrupts.
# Matches the engine's runtime auto-install pin (LING_MEM_PIN in
# memory_tool.rs). `^1` = highest 1.x release (install-bin.sh's `~` form
# needs X.Y, so a major range is `^1`/`1.x`, never `~1`). Override with
# LING_MEM_VERSION (accepts an exact tag or a range).
LING_MEM_PIN_DEFAULT="^1"

# Canonical ling-mem installer — the same script the engine runtime
# auto-install and the linggen skill use. Range-aware, no-downgrade,
# installs a real binary to ~/.local/bin.
LING_MEM_INSTALL_BIN_URL="https://raw.githubusercontent.com/linggen/linggen-memory/main/plugins/linggen/scripts/install-bin.sh"

# Which door this machine came through. Recorded in the install-source
# marker files below and reported once by each binary's first-launch
# telemetry. The DEFAULT is `website`, because an un-overridden run of this
# script means someone pasted the one-liner from linggen.dev — keeping the
# published command clean of env-var noise is the point. Chained installers
# override it: the ClawHub skill bundle sets `clawhub`, the Claude Code /
# Codex plugin hook sets `plugin`, the VS Code extension sets
# `vscode-extension`, Linggen.app writes its own `app` marker. ling-mem
# inherits the same channel unless told otherwise, so one install reports
# one door for both binaries.
LINGGEN_SOURCE="${LINGGEN_SOURCE:-website}"
LING_MEM_SOURCE="${LING_MEM_SOURCE:-$LINGGEN_SOURCE}"
export LING_MEM_SOURCE
# The host agent, when the caller knows it (skill bootstrap, plugin hook).
# No default: a pasted one-liner has no agent, and we never guess.
if [ -n "${LINGGEN_AGENT:-}" ]; then
  LING_MEM_AGENT="${LING_MEM_AGENT:-$LINGGEN_AGENT}"
  export LING_MEM_AGENT
fi

# Every download is checked against its published SHA-256 before install;
# a missing or mismatched one aborts. LINGGEN_RELEASE_BASE=<url> replaces
# GitHub and linggen.dev/dl — the release gate's local copy of a draft, laid
# out <url>/<owner>/<repo>/<asset>. ling's SHA-256 then comes from that
# manifest.json; install-bin.sh (ling-mem) honours the same variable.
RELEASE_BASE="${LINGGEN_RELEASE_BASE:-}"
RELEASE_BASE="${RELEASE_BASE%/}"

LOCAL_PATH=""
VERSION_ARG=""

usage() {
  cat <<EOF
Usage: $(basename "$0") [--version <ver>] [--local-path <file://...tar.gz>]

Options:
  --version <ver>    Install a specific version. If omitted, installs latest.
  --local-path <url> Install from a local file URL or local path. Skips network fetch.

Environment:
  LING_MEM_VERSION   ling-mem version spec — exact tag or range (default: ${LING_MEM_PIN_DEFAULT}).
  LINGGEN_RELEASE_BASE  release mirror replacing GitHub (<url>/<owner>/<repo>/<asset>).

Installs the 'ling' CLI binary (Linggen AI coding agent) plus the
pinned 'ling-mem' memory backend, both fetched from GitHub releases
and checked against their published SHA-256.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION_ARG="$2"; shift 2 ;;
    --local-path)
      LOCAL_PATH="$2"; shift 2 ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "Unknown option: $1" >&2
      usage; exit 1 ;;
  esac
done

detect_slug() {
  local os arch
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$os" in
    darwin)
      case "$arch" in
        arm64|aarch64) echo "macos-aarch64" ;;
        x86_64|amd64)  echo "macos-x86_64" ;;
        *) echo "Unsupported architecture: $arch" >&2; exit 1 ;;
      esac
      ;;
    linux)
      case "$arch" in
        x86_64|amd64) echo "linux-x86_64" ;;
        arm64|aarch64) echo "linux-aarch64" ;;
        *) echo "Unsupported architecture: $arch" >&2; exit 1 ;;
      esac
      ;;
    *) echo "Unsupported OS: $os" >&2; exit 1 ;;
  esac
}

ensure_dir() {
  local dir="$1"
  if [ ! -d "$dir" ]; then
    mkdir -p "$dir" 2>/dev/null || return 1
  fi
  [ -w "$dir" ]
}

install_binary() {
  local tarball="$1" dest_dir="$2" binary_name="$3"
  local tmpdir binpath
  tmpdir="$(mktemp -d)"
  env -u TAR_OPTIONS tar -xzf "$tarball" -C "$tmpdir" >/dev/null
  binpath="$tmpdir/$binary_name"
  if [ ! -f "$binpath" ]; then
    echo "$binary_name binary not found in tarball" >&2
    rm -rf "$tmpdir"
    return 1
  fi

  # A fresh inode, renamed into place: `cp` over an existing (or running)
  # binary rewrites its inode, and macOS keeps the old code signature cached
  # for it — the new binary is then killed on launch, silently.
  local staged="$dest_dir/.$binary_name.new.$$"
  cp "$binpath" "$staged" && chmod +x "$staged" && mv -f "$staged" "$dest_dir/$binary_name" \
    || { rm -f "$staged"; rm -rf "$tmpdir"; echo "could not install $binary_name into $dest_dir" >&2; return 1; }
  rm -rf "$tmpdir"
}

# A running engine keeps the old binary until it restarts. Restart each engine
# daemon this install just replaced — the `ling --web …` that bare `ling`
# spawns, at this exact path — with its own arguments and working directory,
# the way bare `ling` launched it (no browser this time). An engine at any
# other path (Linggen.app carries its own) is left alone.
restart_engine_daemon() {
  local bin="$1" pid args cwd port i
  command -v pgrep >/dev/null 2>&1 || return 0
  for pid in $(pgrep -f -- "$bin --web" 2>/dev/null || true); do
    args="$(ps -o args= -p "$pid" 2>/dev/null || true)"
    case "$args" in "$bin --web"*) ;; *) continue ;; esac
    port="$(printf '%s' "$args" | sed -nE 's/.*--port[ =]([0-9]+).*/\1/p')"
    port="${port:-9527}"
    cwd="$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -n1 || true)"
    echo "Restarting the running Linggen engine (PID $pid, port $port) on the new binary..."
    kill -TERM "$pid" 2>/dev/null || true
    for i in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
    kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null || true
    mkdir -p "$HOME/.linggen"
    # shellcheck disable=SC2086 # the daemon's own argv, split as ps printed it
    (cd "${cwd:-$HOME}" 2>/dev/null || cd "$HOME"; nohup $args >"$HOME/.linggen/ling.log" 2>&1 </dev/null &)
    for i in $(seq 1 30); do
      curl -fsS -m 2 "http://127.0.0.1:${port}/api/health" >/dev/null 2>&1 && { echo "   Engine is back on port $port."; break; }
      sleep 1
    done
  done
  return 0
}

# GitHub is blocked or flaky in some regions (notably China); linggen.dev
# (Cloudflare) mirrors our release traffic at /dl/*. Rewrite a GitHub URL to
# its mirror form — empty output means the URL has no mirror equivalent.
mirror_url() {
  local url="$1"
  case "$url" in
    https://api.github.com/repos/*/releases/latest)
      echo "https://linggen.dev/dl/api/$(echo "$url" | sed -E 's|.*/repos/([^/]+/[^/]+)/.*|\1|')/releases/latest" ;;
    https://github.com/*/releases/download/*)
      echo "$url" | sed -E 's|https://github.com/([^/]+/[^/]+)/releases/download/|https://linggen.dev/dl/release/\1/|' ;;
    https://raw.githubusercontent.com/*/install-bin.sh)
      echo "https://linggen.dev/dl/install-bin.sh" ;;
    *) echo "" ;;
  esac
}

# curl with mirror fallback: try GitHub, then the linggen.dev mirror.
fetch_url() {
  local url="$1" dest="$2" mirror
  if [ -n "$RELEASE_BASE" ]; then
    curl -fsSL "$url" -o "$dest"
    return
  fi
  curl -fsSL "$url" -o "$dest" && return 0
  mirror="$(mirror_url "$url")"
  if [ -n "$mirror" ]; then
    echo "   GitHub unreachable — trying mirror $mirror" >&2
    curl -fsSL "$mirror" -o "$dest" && return 0
  fi
  return 1
}

download_tarball() {
  local url="$1" dest="$2"
  if [[ "$url" == file://* ]]; then
    cp "${url#file://}" "$dest"
  else
    if ! fetch_url "$url" "$dest"; then
      echo "Failed to download from $url (and its mirror)" >&2
      echo "   This may be a temporary CDN issue. Please try again." >&2
      return 1
    fi
  fi
}

# The sha256 a manifest.json (on stdin) lists for an asset (e.g.
# ling-macos-aarch64). Plain text tools only — no jq on a fresh Mac.
manifest_asset_sha256() {
  local asset="$1"
  tr -d ' \n\r\t' | tr '}' '\n' \
    | grep "\"name\":\"${asset}\"" \
    | grep -oE '"sha256":"[0-9a-fA-F]{64}"' \
    | cut -d'"' -f4 | head -n1 || true
}

# The sha256 GitHub records for a release asset — the "digest" beside its
# "name" in release JSON on stdin (e.g. ling-macos-aarch64.tar.gz).
release_json_sha256() {
  local file="$1"
  tr -d ' \n\r\t' | tr ',{' '\n\n' \
    | awk -v want="\"name\":\"${file}\"" '
        /^"name":/ { hit = ($0 == want); next }
        hit && /^"digest":"sha256:/ { print; exit }' \
    | grep -oE '[0-9a-fA-F]{64}' | head -n1 || true
}

# ling's published sha256 on the default path: the release's manifest.json,
# else GitHub's per-asset digest (manifests before 1.8.3 carry no sha256).
# An empty tag means the /latest/ release. Either may come via the mirror —
# the hash is checked against the tarball all the same.
ling_release_sha256() {
  local tag="$1" slug="$2" manifest_url api_url tmp sha=""
  if [ -n "$tag" ]; then
    manifest_url="https://github.com/linggen/linggen/releases/download/${tag}/manifest.json"
    api_url="https://api.github.com/repos/linggen/linggen/releases/tags/${tag}"
  else
    manifest_url="https://github.com/linggen/linggen/releases/latest/download/manifest.json"
    api_url="https://api.github.com/repos/linggen/linggen/releases/latest"
  fi
  tmp="$(mktemp)"
  if fetch_url "$manifest_url" "$tmp" 2>/dev/null; then
    sha="$(manifest_asset_sha256 "ling-${slug}" < "$tmp")"
  fi
  if [ -z "$sha" ] && fetch_url "$api_url" "$tmp" 2>/dev/null; then
    sha="$(release_json_sha256 "ling-${slug}.tar.gz" < "$tmp")"
  fi
  rm -f "$tmp"
  echo "$sha"
}

verify_sha256() {
  local file="$1" expected="$2" label="${3:-$(basename "$1")}" actual
  if [ -z "$expected" ]; then
    echo "No published sha256 for ${label} — refusing to install." >&2
    return 1
  fi
  if command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    actual="$(sha256sum "$file" | awk '{print $1}')"
  fi
  if [ "$(echo "$actual" | tr 'A-F' 'a-f')" != "$(echo "$expected" | tr 'A-F' 'a-f')" ]; then
    echo "SHA-256 mismatch for ${label}: expected $expected, got $actual — refusing to install." >&2
    return 1
  fi
  echo "Verified SHA-256"
}

resolve_latest_tag() {
  local repo="$1"
  local tag body
  body=$(curl -fsS "https://api.github.com/repos/${repo}/releases/latest" 2>/dev/null \
    || curl -fsS "https://linggen.dev/dl/api/${repo}/releases/latest" 2>/dev/null \
    || echo "")
  tag=$(echo "$body" | grep -o '"tag_name": "[^"]*' | cut -d'"' -f4 || echo "")
  echo "$tag"
}

# An empty tag means the /latest/download/ release.
resolve_download_url() {
  local repo="$1" binary="$2" slug="$3" tag="$4"
  if [ -n "$tag" ]; then
    echo "https://github.com/${repo}/releases/download/${tag}/${binary}-${slug}.tar.gz"
  else
    echo "https://github.com/${repo}/releases/latest/download/${binary}-${slug}.tar.gz"
  fi
}

check_path_conflicts() {
  local binary="$1" dest="$2" installed_version="$3"

  local current_in_path
  current_in_path=$(command -v "$binary" || echo "")
  if [ -n "$current_in_path" ]; then
    local path_version
    path_version=$("$binary" --version | awk '{print $2}' || echo "unknown")
    if [ "$current_in_path" != "$dest/$binary" ]; then
      echo ""
      echo "Warning: A different $binary binary was found in your PATH at $current_in_path"
      echo "   It reports version $path_version, while the new version is $installed_version at $dest/$binary"
      echo "   To use the new version, you may need to remove the old one or adjust your PATH."
    elif [ "$path_version" != "$installed_version" ]; then
      echo ""
      echo "Note: Your shell may have cached the old '$binary' binary location."
      echo "   Run 'hash -r' (bash) or 'rehash' (zsh) to refresh it."
    fi
  fi
}

remove_old_binary() {
  local binary_path="$1"
  if [ ! -f "$binary_path" ]; then
    return
  fi
  echo "   Removing old binary: $binary_path"
  rm -f "$binary_path" 2>/dev/null || true
}

cleanup_legacy_install() {
  local dest_dir="$1"
  local found=false

  # Old binary names from pre-consolidation Linggen
  local old_binaries=("linggen" "linggen-server")
  local search_dirs=("$dest_dir" "/usr/local/bin" "$HOME/.local/bin")

  for dir in "${search_dirs[@]}"; do
    for old in "${old_binaries[@]}"; do
      if [ -f "$dir/$old" ]; then
        if [ "$found" = "false" ]; then
          echo ""
          echo "Found legacy Linggen binaries (renamed: linggen -> ling, linggen-server -> ling-mem):"
          found=true
        fi
        remove_old_binary "$dir/$old"
      fi
    done
  done

  # Clean up old PID files
  local old_pids=("$HOME/.linggen/linggen-server.pid" "$HOME/.linggen/linggen-agent.pid")
  for pidfile in "${old_pids[@]}"; do
    if [ -f "$pidfile" ]; then
      # Try to stop the old process gracefully
      local pid
      pid=$(cat "$pidfile" 2>/dev/null || echo "")
      if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "   Stopping old process (PID $pid) from $pidfile"
        kill "$pid" 2>/dev/null || true
        sleep 1
      fi
      rm -f "$pidfile"
    fi
  done

  if [ "$found" = "true" ]; then
    echo "   Legacy cleanup complete."
    echo ""
  fi
}

install_ling_mem() {
  local pin="${LING_MEM_VERSION:-$LING_MEM_PIN_DEFAULT}"

  echo ""
  echo "Installing ling-mem (memory backend, ${pin})..."

  # Delegate to the canonical installer — it resolves the range to the
  # highest matching release, installs a real binary to ~/.local/bin,
  # and refuses to downgrade a newer shared copy. One source of truth,
  # shared with the engine runtime + shared-memory skill.
  # LING_MEM_SOURCE is exported at the top of this script (inherited from
  # LINGGEN_SOURCE), and install-bin.sh writes the marker itself — it is the
  # canonical ling-mem installer, so every path that installs the binary
  # records provenance, not just this wrapper. Prefer a LOCAL install-bin.sh
  # shipped alongside this script — skill and plugin artifacts vendor both,
  # so an installed bundle never executes a remotely fetched script. The
  # curl path remains for the linggen.dev one-liner, where this script
  # arrives alone through a pipe (BASH_SOURCE is unset there, so the local
  # probe resolves to nothing and falls through). Mirror fallback: raw.
  # githubusercontent is blocked in some regions; /dl/install-bin.sh serves
  # the same file through linggen.dev.
  local local_bin=""
  if [ -n "${BASH_SOURCE[0]:-}" ] && [ -f "${BASH_SOURCE[0]}" ]; then
    local_bin="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/install-bin.sh"
  fi
  if [ -n "$local_bin" ] && [ -f "$local_bin" ]; then
    if ! bash "$local_bin" --version "$pin"; then
      echo "" >&2
      echo "Warning: couldn't install ling-mem (${pin})." >&2
      echo "         Linggen will install it automatically the first time a" >&2
      echo "         memory feature is used, so this is non-fatal." >&2
      return 0
    fi
  elif [ "${LINGGEN_NO_REMOTE_SCRIPT:-0}" = "1" ]; then
    # A vendored caller (skill/plugin bootstrap) forbids remote script
    # execution outright. It installs ling-mem itself from its own bundled
    # installer before running this script, so skipping here loses nothing.
    echo "Skipping ling-mem install: no local installer beside this script" >&2
    echo "and LINGGEN_NO_REMOTE_SCRIPT=1 forbids fetching one." >&2
    return 0
  elif ! { { [ -n "$RELEASE_BASE" ] && curl -fsSL "$RELEASE_BASE/install-bin.sh"; } \
         || curl -fsSL "$LING_MEM_INSTALL_BIN_URL" \
         || curl -fsSL "https://linggen.dev/dl/install-bin.sh"; } | bash -s -- --version "$pin"; then
    echo "" >&2
    echo "Warning: couldn't install ling-mem (${pin})." >&2
    echo "         Linggen will install it automatically the first time a" >&2
    echo "         memory feature is used, so this is non-fatal." >&2
    return 0
  fi

  local bin lm_version
  bin="$(command -v ling-mem 2>/dev/null || echo "$HOME/.local/bin/ling-mem")"
  lm_version=$("$bin" --version 2>/dev/null | awk '{print $2}' || echo "unknown")
  echo "Installed ling-mem v${lm_version}"
}

install_bun() {
  # Bundled JS runtime for skill action scripts (skills declare tools whose
  # cmd runs JS via <skill>/scripts/run-js.sh — it prefers this binary, then
  # bun/node on PATH). One self-contained file; no npm, no packages. Skip if
  # already present; soft-fail — machines with node still work without it.
  local bin_dir="$HOME/.linggen/bin"
  if [ -x "$bin_dir/bun" ]; then
    echo "bun already bundled at $bin_dir/bun"
    return 0
  fi

  local os arch bun_slug
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$os-$arch" in
    darwin-arm64|darwin-aarch64) bun_slug="bun-darwin-aarch64" ;;
    darwin-x86_64)               bun_slug="bun-darwin-x64" ;;
    linux-x86_64|linux-amd64)    bun_slug="bun-linux-x64" ;;
    linux-arm64|linux-aarch64)   bun_slug="bun-linux-aarch64" ;;
    *) echo "No bun build for $os/$arch — skill scripts will use node if present."; return 0 ;;
  esac

  echo ""
  echo "Bundling bun (JS runtime for skill actions)..."
  # Checked against bun's own SHASUMS256.txt; any failure skips bun.
  local zipfile sums tmpdir base="https://github.com/oven-sh/bun/releases/latest/download"
  zipfile="$(mktemp)"
  sums="$(mktemp)"
  tmpdir="$(mktemp -d)"
  if curl -fsSL "$base/SHASUMS256.txt" -o "$sums" \
      && curl -fsSL "$base/${bun_slug}.zip" -o "$zipfile" \
      && verify_sha256 "$zipfile" "$(awk -v f="${bun_slug}.zip" '$2 == f { print $1; exit }' "$sums")" "${bun_slug}.zip" \
      && unzip -q -o "$zipfile" -d "$tmpdir"; then
    mkdir -p "$bin_dir"
    cp "$tmpdir/${bun_slug}/bun" "$bin_dir/bun"
    chmod +x "$bin_dir/bun"
    echo "Bundled bun $("$bin_dir/bun" --version 2>/dev/null || echo '?') to $bin_dir/bun"
  else
    echo "Warning: couldn't fetch bun — skill scripts will fall back to node." >&2
  fi
  rm -rf "$zipfile" "$sums" "$tmpdir"
  return 0
}

main() {
  local slug dest="$INSTALL_DIR_DEFAULT"
  local version="${VERSION_ARG:-latest}"

  slug="$(detect_slug)"

  # Determine install directory
  if ! ensure_dir "$dest"; then
    echo "Using fallback install dir: $FALLBACK_DIR"
    dest="$FALLBACK_DIR"
    mkdir -p "$dest"
  fi

  # --- Install ling ---
  echo "Installing ling (Linggen)..."

  local ling_url ling_tarball ling_tag="" expected=""
  if [ -n "$LOCAL_PATH" ]; then
    if [[ "$LOCAL_PATH" == file://* ]]; then
      ling_url="$LOCAL_PATH"
    else
      ling_url="file://$LOCAL_PATH"
    fi
  elif [ -n "$RELEASE_BASE" ]; then
    echo "Release source: $RELEASE_BASE (LINGGEN_RELEASE_BASE)" >&2
    ling_url="$RELEASE_BASE/linggen/linggen/ling-${slug}.tar.gz"
  else
    if [ "$version" = "latest" ]; then
      echo "Fetching latest release info from GitHub..." >&2
      ling_tag="$(resolve_latest_tag "linggen/linggen")"
      if [ -n "$ling_tag" ]; then
        echo "   Latest ling version: ${ling_tag}" >&2
      else
        echo "   Falling back to /latest/download/ for ling" >&2
      fi
    else
      ling_tag="$version"
    fi
    ling_url=$(resolve_download_url "linggen/linggen" "ling" "$slug" "$ling_tag")
  fi

  echo "Downloading $ling_url"
  ling_tarball="$(mktemp)"
  download_tarball "$ling_url" "$ling_tarball"
  # Every download is checked against its published sha256 before anything
  # installed is touched; only --local-path (a file already on this machine)
  # skips it.
  if [ -z "$LOCAL_PATH" ]; then
    if [ -n "$RELEASE_BASE" ]; then
      expected="$(curl -fsSL "$RELEASE_BASE/linggen/linggen/manifest.json" 2>/dev/null \
        | manifest_asset_sha256 "ling-${slug}" || true)"
    else
      expected="$(ling_release_sha256 "$ling_tag" "$slug")"
    fi
    verify_sha256 "$ling_tarball" "$expected" "ling-${slug}.tar.gz" || { rm -f "$ling_tarball"; exit 1; }
  fi

  # Clean up old linggen/linggen-server binaries
  cleanup_legacy_install "$dest"

  install_binary "$ling_tarball" "$dest" "ling"
  rm -f "$ling_tarball"

  local ling_version
  ling_version=$("$dest/ling" --version | awk '{print $2}' || echo "unknown")
  echo "Installed ling v${ling_version} to $dest/ling"
  check_path_conflicts "ling" "$dest" "$ling_version"
  restart_engine_daemon "$dest/ling"

  # Telemetry source marker — read by the engine on its first launch (or
  # after a version change) to record how this machine was reached. Writes
  # to ~/.linggen; LINGGEN_SOURCE is resolved at the top of this script,
  # LINGGEN_AGENT names the host agent when the caller knows it (the skill
  # bootstrap and plugin hook set it; a pasted one-liner has none).
  mkdir -p "$HOME/.linggen"
  {
    printf 'via=%s\n' "$LINGGEN_SOURCE"
    if [ -n "${LINGGEN_AGENT:-}" ]; then printf 'agent=%s\n' "$LINGGEN_AGENT"; fi
    printf 'installer_version=%s\n' "$ling_version"
    printf 'installed_at=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > "$HOME/.linggen/.linggen-install-source"

  # --- Install ling-mem (pinned binary, direct from linggen-memory releases) ---
  # ling-mem is mandatory: the engine's encoder, dream mission, core
  # memory inject, and Memory_* tools all need it. We pull the binary
  # tarball directly so the Linggen install owns its pinned version
  # without depending on the install-shared-memory.sh wrapper's release
  # cadence. The wrapper still ships the full shared-memory skill
  # bundle (recall hook, dashboard, references) for users who want
  # cross-host bridging — that install path remains separate. Soft-
  # fails: a network blip shouldn't take down the whole install —
  # re-runnable separately.
  install_ling_mem

  # --- Bundle bun (JS runtime for skill action scripts) ---
  install_bun

  # --- Post-install ---
  echo ""
  if [[ ":$PATH:" != *":$dest:"* ]]; then
    echo "Add to PATH if needed:"
    echo "    export PATH=\"$dest:\$PATH\""
    echo ""
  fi

  echo "Get started:"
  echo "    ling init         # Set up default config & download skills"
  echo "    ling              # Start agent (opens browser)"
  echo "    ling --web        # Start server (foreground)"
  echo "    ling status       # Show status"
  echo ""
  echo "Note: Linggen sends anonymous usage pings (install, engine.start, skill.open, …)"
  echo "      to help improve it. No content, no identity. Disable any time:"
  echo "        touch ~/.linggen/no-telemetry"
}

main "$@"
