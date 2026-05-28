#!/usr/bin/env bash
#
# install-bin.sh — download and install the `ling-mem` binary.
#
# Called from the SessionStart hook (autostart.sh) to bootstrap the binary
# on first plugin use, and standalone via the linggen.dev wrapper for hosts
# without a plugin model (OpenClaw, Linggen native).
#
#   --version vX.Y.Z   pin a specific release    (default: from $VERSION env or v0.7.1)
#   --dest <dir>       install dir for the binary (default: ~/.local/bin)
#   --quiet            suppress informational output
#   --force            re-download even if version matches
#
# Mandatory SHA-256 verification (override with LING_MEM_SKIP_CHECKSUM=1).
# Source: https://github.com/linggen/linggen-memory/releases
#
set -euo pipefail

REPO="linggen/linggen-memory"
VERSION="${VERSION:-v0.7.1}"
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
      sed -n '3,16p' "$0"; exit 0 ;;
    *) echo "install-bin: unknown arg: $1" >&2; exit 2 ;;
  esac
done

say() { [ "$QUIET" = "1" ] || echo "install-bin: $*"; }

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
EXPECTED="${VERSION#v}"
if [ "$FORCE" = "0" ] && [ -x "$BIN" ]; then
  HAVE="$("$BIN" --version 2>/dev/null | awk '{print $2}' || true)"
  if [ "$HAVE" = "$EXPECTED" ]; then
    say "already at $VERSION ($BIN)"
    exit 0
  fi
fi

ASSET="ling-mem-${TARGET}.tar.gz"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"
URL="${BASE}/${ASSET}"
SUM_URL="${BASE}/${ASSET}.sha256"

TMP="$(mktemp -d -t ling-mem-dl-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

say "downloading ling-mem $VERSION ($TARGET)"
curl -fsSL --retry 3 --retry-delay 2 "$URL" -o "$TMP/$ASSET"

if [ "${LING_MEM_SKIP_CHECKSUM:-0}" = "1" ]; then
  say "WARNING: SHA-256 verification skipped"
else
  curl -fsSL --retry 3 --retry-delay 2 "$SUM_URL" -o "$TMP/$ASSET.sha256"
  if command -v shasum >/dev/null 2>&1; then
    (cd "$TMP" && shasum -a 256 -c "$ASSET.sha256" >/dev/null)
  elif command -v sha256sum >/dev/null 2>&1; then
    (cd "$TMP" && sha256sum -c "$ASSET.sha256" >/dev/null)
  else
    echo "install-bin: no shasum / sha256sum available" >&2; exit 1
  fi
  say "verified SHA-256"
fi

tar -xzf "$TMP/$ASSET" -C "$DEST" ling-mem
chmod +x "$BIN"
say "installed $BIN"

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) say "note: $DEST is not on PATH; add it to your shell rc" ;;
esac
