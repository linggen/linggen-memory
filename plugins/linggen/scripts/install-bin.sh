#!/usr/bin/env bash
#
# install-bin.sh — download and install the `ling-mem` binary.
#
# Called from the SessionStart hook (autostart.sh) to bootstrap the binary
# on first plugin use, and standalone via the linggen.dev wrapper for hosts
# without a plugin model (OpenClaw, Linggen native).
#
#   --version <spec>   release to install (default: $VERSION env or v1.0.0)
#                      <spec> is an exact tag or a semver range:
#                        vX.Y.Z   exact   — no network (default behavior)
#                        ~X.Y     patch   — highest vX.Y.*
#                        ^X | X   minor   — highest vX.*
#                        latest           — highest stable release
#                      Ranges resolve via the GitHub releases API, cached 24h
#                      (LING_MEM_RESOLVE_TTL); offline falls back to the
#                      installed binary.
#   --dest <dir>       install dir for the binary (default: ~/.local/bin)
#   --quiet            suppress informational output
#   --force            re-download even if version matches
#
# Mandatory SHA-256 verification (override with LING_MEM_SKIP_CHECKSUM=1).
# Source: https://github.com/linggen/linggen-memory/releases
#
set -euo pipefail

REPO="linggen/linggen-memory"
VERSION="${VERSION:-v1.0.0}"
DEST="$HOME/.local/bin"
QUIET=0
FORCE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --dest)    DEST="$2"; shift 2 ;;
    --quiet)   QUIET=1; shift ;;
    --force)   FORCE=1; shift ;;
    -h|--help)
      sed -n '3,23p' "$0"; exit 0 ;;
    *) echo "install-bin: unknown arg: $1" >&2; exit 2 ;;
  esac
done

say() { [ "$QUIET" = "1" ] || echo "install-bin: $*"; }

# ── Version-spec resolution ─────────────────────────────────────────────────
# Resolve a --version spec to a concrete tag. Exact tags (vX.Y.Z) pass through
# with no network call; semver ranges resolve to the highest matching release
# via the GitHub API, cached for 24h so the SessionStart hook isn't a per-call
# API hit. API failure falls back to a stale cache (handled by the caller).
RESOLVE_TTL="${LING_MEM_RESOLVE_TTL:-86400}"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/ling-mem"

is_exact_tag() { case "$1" in v[0-9]*.[0-9]*.[0-9]*) return 0 ;; *) return 1 ;; esac; }

# GitHub is blocked or flaky in some regions (notably China); linggen.dev
# (Cloudflare) mirrors our release traffic at /dl/*. Every network fetch
# below tries GitHub first, then the mirror.
MIRROR="https://linggen.dev/dl"

list_release_tags() {
  { curl -fsSL --retry 3 --retry-delay 2 \
         -H 'Accept: application/vnd.github+json' \
         "https://api.github.com/repos/${REPO}/releases?per_page=100" \
    || curl -fsSL "${MIRROR}/api/${REPO}/releases"; } \
    | grep -oE '"tag_name"[[:space:]]*:[[:space:]]*"[^"]+"' \
    | sed -E 's/.*"([^"]+)"$/\1/'
}

# Highest release tag satisfying a range spec, pre-releases excluded.
highest_matching() {
  local spec="$1" re
  case "$spec" in
    latest)           re='^v[0-9]+\.[0-9]+\.[0-9]+$' ;;
    "~"[0-9]*.[0-9]*) re="^v${spec#\~}\.[0-9]+$" ;;
    "^"[0-9]*)        re="^v${spec#^}\.[0-9]+\.[0-9]+$" ;;
    [0-9]*.x)         re="^v${spec%.x}\.[0-9]+\.[0-9]+$" ;;
    [0-9]*)           re="^v${spec}\.[0-9]+\.[0-9]+$" ;;
    *) return 1 ;;
  esac
  list_release_tags | grep -E "$re" | grep -vE -- '-' | sort -V | tail -n1
}

# Resolve, with a 24h cache. Prints the concrete tag, or nothing on failure.
resolve_version() {
  local spec="$1"
  is_exact_tag "$spec" && { printf '%s\n' "$spec"; return 0; }

  local key cache
  key="$(printf '%s' "$spec" | tr -c 'A-Za-z0-9._-' '_')"
  cache="$CACHE_DIR/resolved-$key"

  if [ -f "$cache" ]; then
    local mtime now
    mtime="$(stat -f %m "$cache" 2>/dev/null || stat -c %Y "$cache" 2>/dev/null || echo 0)"
    now="$(date +%s 2>/dev/null || echo 0)"
    if [ "$now" -gt 0 ] && [ "$(( now - mtime ))" -lt "$RESOLVE_TTL" ]; then
      cat "$cache"; return 0
    fi
  fi

  local tag
  tag="$(highest_matching "$spec" 2>/dev/null || true)"
  if [ -n "$tag" ]; then
    mkdir -p "$CACHE_DIR" 2>/dev/null || true
    printf '%s\n' "$tag" > "$cache" 2>/dev/null || true
    printf '%s\n' "$tag"; return 0
  fi

  # API failed — fall back to a stale cache if present.
  [ -f "$cache" ] && { cat "$cache"; return 0; }
  return 1
}

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$OS-$ARCH" in
  darwin-arm64|darwin-aarch64) TARGET="macos-aarch64" ;;
  darwin-x86_64|darwin-amd64)  TARGET="macos-x86_64" ;;
  linux-x86_64|linux-amd64)    TARGET="linux-x86_64" ;;
  linux-aarch64|linux-arm64)   TARGET="linux-aarch64" ;;
  *) echo "install-bin: unsupported platform $OS-$ARCH" >&2; exit 1 ;;
esac

mkdir -p "$DEST"
[ -w "$DEST" ] || { echo "install-bin: $DEST not writable" >&2; exit 1; }

BIN="$DEST/ling-mem"

# Resolve a range spec to a concrete tag before comparing/downloading. Exact
# tags pass through untouched (no network). If resolution fails (offline,
# rate-limited, no match) keep an already-installed binary rather than
# breaking the session.
if ! VERSION="$(resolve_version "$VERSION")" || [ -z "$VERSION" ]; then
  if [ -x "$BIN" ]; then
    say "could not resolve version spec (offline?); keeping installed $("$BIN" --version 2>/dev/null | awk '{print $2}')"
    exit 0
  fi
  echo "install-bin: could not resolve a release for the requested version spec" >&2
  exit 1
fi

EXPECTED="${VERSION#v}"
# Skip unless the installed binary is strictly OLDER than the target — never
# downgrade. The shared canonical binary is touched by every host; a
# stale-pinned channel must not roll back a newer one. (`sort -V` lowest ==
# EXPECTED means installed is >= target.)
if [ "$FORCE" = "0" ] && [ -x "$BIN" ]; then
  HAVE="$("$BIN" --version 2>/dev/null | awk '{print $2}' || true)"
  if [ -n "$HAVE" ] && \
     [ "$(printf '%s\n%s\n' "$HAVE" "$EXPECTED" | sort -V | head -n1)" = "$EXPECTED" ]; then
    say "already at $HAVE (target $VERSION; not downgrading) ($BIN)"
    exit 0
  fi
fi

ASSET="ling-mem-${TARGET}.tar.gz"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"
MIRROR_BASE="${MIRROR}/release/${REPO}/${VERSION}"

# Download with mirror fallback. The SHA-256 check below runs on the result
# either way, so a compromised or truncated mirror response cannot install.
fetch_asset() {
  local name="$1"
  curl -fsSL --retry 3 --retry-delay 2 "${BASE}/${name}" -o "$TMP/$name" \
    || { say "GitHub unreachable — using mirror for $name"; \
         curl -fsSL "${MIRROR_BASE}/${name}" -o "$TMP/$name"; }
}

TMP="$(mktemp -d -t ling-mem-dl-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

say "downloading ling-mem $VERSION ($TARGET)"
fetch_asset "$ASSET"

if [ "${LING_MEM_SKIP_CHECKSUM:-0}" = "1" ]; then
  say "WARNING: SHA-256 verification skipped"
else
  fetch_asset "$ASSET.sha256"

  # Pull the expected hex digest and reject anything that doesn't look
  # like a real checksum line. Both `shasum -c` and `sha256sum -c` parse
  # the file as `<hex>  <filename>` — a malformed file (HTML error page,
  # truncated download, wrong path component) can pass `-c` silently on
  # some implementations when the filename doesn't match any file in cwd
  # ("no properly formatted SHA1 checksum lines found" → exit 1 on GNU
  # but inconsistent elsewhere). Pin both inputs explicitly: extract the
  # first 64-hex token, recompute, compare literal strings.
  expected="$(awk 'match($0, /^[0-9a-fA-F]{64}/) { print toupper(substr($0, RSTART, RLENGTH)); exit }' "$TMP/$ASSET.sha256")"
  if [ -z "$expected" ]; then
    echo "install-bin: malformed .sha256 for $ASSET" >&2
    sed -n '1,3p' "$TMP/$ASSET.sha256" >&2
    exit 1
  fi

  if command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$TMP/$ASSET" | awk '{print toupper($1)}')"
  elif command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$TMP/$ASSET" | awk '{print toupper($1)}')"
  else
    echo "install-bin: no shasum / sha256sum available" >&2; exit 1
  fi

  if [ "$actual" != "$expected" ]; then
    echo "install-bin: SHA-256 mismatch" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    exit 1
  fi
  say "verified SHA-256"
fi

# Replace any legacy symlink (older plugins symlinked ~/.local/bin/ling-mem to
# a per-plugin data-dir copy) with a real file, so tar doesn't write through it.
rm -f "$BIN"
tar -xzf "$TMP/$ASSET" -C "$DEST" ling-mem
chmod +x "$BIN"
say "installed $BIN"

# Install-source marker. `ling-mem` reads this on its first launch and reports
# `via` once, so we learn which door a machine came through. This script is the
# canonical ling-mem installer — the plugin hook, the skill bootstrap and
# linggen.dev's install.sh all funnel through it — so the marker is written
# here rather than in each caller, and every install path gets provenance
# instead of only the wrapper. The script itself never talks to the network
# about this: it writes a file, the binary decides whether to report it.
# Callers label their channel with LING_MEM_SOURCE (unset means direct) and
# optionally the host agent with LING_MEM_AGENT (cc/codex/openclaw/linggen).
mkdir -p "$HOME/.linggen" 2>/dev/null || true
{
  printf 'via=%s\n' "${LING_MEM_SOURCE:-install-bin}"
  [ -n "${LING_MEM_AGENT:-}" ] && printf 'agent=%s\n' "$LING_MEM_AGENT"
  printf 'installer_version=%s\n' "$("$BIN" --version 2>/dev/null | awk '{print $2}' || echo unknown)"
  printf 'installed_at=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$HOME/.linggen/.ling-mem-install-source" 2>/dev/null \
  || say "note: could not write install-source marker"

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) say "note: $DEST is not on PATH; add it to your shell rc" ;;
esac
