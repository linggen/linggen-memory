#!/bin/bash
set -euo pipefail

# Build script for macOS — builds the 'ling-mem' binary with embedded Web UI
# Usage: ./scripts/build-mac.sh <version>

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-common.sh"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version>" >&2
  exit 1
fi

VERSION_NUM="${VERSION#v}"
DIST_DIR="$ROOT_DIR/dist"
mkdir -p "$DIST_DIR"

SLUG=$(detect_platform)
ARCH="$(uname -m)"

echo "Building ling-mem ${VERSION} for macOS (${ARCH})"
echo "=========================================="

# Step 1: Build Frontend
echo "1. Building Frontend..."
cd "$ROOT_DIR/frontend"
if [ -f "package-lock.json" ]; then npm ci; else npm install; fi
npm run build
echo "   Frontend built"

# Step 2: Build ling-mem binary (with embedded UI via rust-embed)
echo ""
echo "2. Building ling-mem binary..."
cd "$ROOT_DIR/backend"
cargo clean -p api
cargo build --release --bin ling-mem
BUILT_VER=$(target/release/ling-mem --version | awk '{print $2}')
if [ "$BUILT_VER" != "$VERSION_NUM" ]; then
  echo "Error: Built version ($BUILT_VER) does not match target version ($VERSION_NUM)" >&2
  exit 1
fi
tar -C target/release -czf "$DIST_DIR/ling-mem-${SLUG}.tar.gz" ling-mem
echo "   ling-mem built: dist/ling-mem-${SLUG}.tar.gz"

# Step 3: Signing
echo ""
echo "3. Signing Artifacts..."

TARBALL="$DIST_DIR/ling-mem-${SLUG}.tar.gz"
SIG=$(sign_file "$TARBALL" "$ROOT_DIR") || true
if [ -n "$SIG" ]; then
  echo "$SIG" > "${TARBALL}.sig.txt"
  echo "   Tarball signed"
fi

echo ""
echo "macOS build complete! Artifacts are in $DIST_DIR"
