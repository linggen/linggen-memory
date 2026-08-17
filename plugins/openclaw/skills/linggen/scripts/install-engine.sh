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

Installs the 'ling' CLI binary (Linggen AI coding agent) plus the
pinned 'ling-mem' memory backend, both fetched from GitHub releases.
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

  cp "$binpath" "$dest_dir/"
  chmod +x "$dest_dir/$binary_name"
  rm -rf "$tmpdir"
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

resolve_latest_tag() {
  local repo="$1"
  local tag body
  body=$(curl -fsS "https://api.github.com/repos/${repo}/releases/latest" 2>/dev/null \
    || curl -fsS "https://linggen.dev/dl/api/${repo}/releases/latest" 2>/dev/null \
    || echo "")
  tag=$(echo "$body" | grep -o '"tag_name": "[^"]*' | cut -d'"' -f4 || echo "")
  echo "$tag"
}

resolve_download_url() {
  local repo="$1" binary="$2" slug="$3" version="$4"

  if [ "$version" = "latest" ]; then
    local tag
    tag=$(resolve_latest_tag "$repo")
    if [ -n "$tag" ]; then
      echo "   Latest ${binary} version: ${tag}" >&2
      echo "https://github.com/${repo}/releases/download/${tag}/${binary}-${slug}.tar.gz"
    else
      echo "   Falling back to /latest/download/ for ${binary}" >&2
      echo "https://github.com/${repo}/releases/latest/download/${binary}-${slug}.tar.gz"
    fi
  else
    echo "https://github.com/${repo}/releases/download/${version}/${binary}-${slug}.tar.gz"
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
  elif ! { curl -fsSL "$LING_MEM_INSTALL_BIN_URL" \
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
  local zipfile tmpdir
  zipfile="$(mktemp)"
  tmpdir="$(mktemp -d)"
  if curl -fsSL "https://github.com/oven-sh/bun/releases/latest/download/${bun_slug}.zip" -o "$zipfile" \
      && unzip -q -o "$zipfile" -d "$tmpdir"; then
    mkdir -p "$bin_dir"
    cp "$tmpdir/${bun_slug}/bun" "$bin_dir/bun"
    chmod +x "$bin_dir/bun"
    echo "Bundled bun $("$bin_dir/bun" --version 2>/dev/null || echo '?') to $bin_dir/bun"
  else
    echo "Warning: couldn't fetch bun — skill scripts will fall back to node." >&2
  fi
  rm -rf "$zipfile" "$tmpdir"
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

  # Clean up old linggen/linggen-server binaries
  cleanup_legacy_install "$dest"

  # --- Install ling ---
  echo "Installing ling (Linggen)..."

  local ling_url ling_tarball
  if [ -n "$LOCAL_PATH" ]; then
    if [[ "$LOCAL_PATH" == file://* ]]; then
      ling_url="$LOCAL_PATH"
    else
      ling_url="file://$LOCAL_PATH"
    fi
  else
    echo "Fetching latest release info from GitHub..." >&2
    ling_url=$(resolve_download_url "linggen/linggen" "ling" "$slug" "$version")
  fi

  echo "Downloading $ling_url"
  ling_tarball="$(mktemp)"
  download_tarball "$ling_url" "$ling_tarball"
  install_binary "$ling_tarball" "$dest" "ling"
  rm -f "$ling_tarball"

  local ling_version
  ling_version=$("$dest/ling" --version | awk '{print $2}' || echo "unknown")
  echo "Installed ling v${ling_version} to $dest/ling"
  check_path_conflicts "ling" "$dest" "$ling_version"

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
