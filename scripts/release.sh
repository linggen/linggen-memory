#!/bin/bash
set -euo pipefail

# Release orchestrator script for Linggen Memory
# Usage: ./scripts/release.sh <version> [--draft] [--skip-linux]

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-common.sh"

REPO="linggen/linggen-memory"
VERSION=""
KEEP_DRAFT=false
PASS_ARGS=()

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --draft)
      KEEP_DRAFT=true
      shift ;;
    --skip-linux)
      PASS_ARGS+=("--skip-linux")
      shift ;;
    *)
      if [ -z "$VERSION" ]; then
        VERSION="$1"
      fi
      shift ;;
  esac
done

if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version> [--draft] [--skip-linux]" >&2
  exit 1
fi

VERSION_NUM="${VERSION#v}"
DIST_DIR="$ROOT_DIR/dist"

# Step 1: Build everything
echo "Step 1: Building all artifacts..."
"$ROOT_DIR/scripts/build.sh" "$VERSION" ${PASS_ARGS[@]+"${PASS_ARGS[@]}"}

SLUG=$(detect_platform)

# Step 2: Create GitHub Release
echo ""
echo "Step 2: Creating GitHub Release..."
if gh release view "$VERSION" --repo "$REPO" &>/dev/null; then
  echo "Release ${VERSION} already exists"
else
  gh release create "$VERSION" \
    --repo "$REPO" \
    --title "Linggen Memory ${VERSION}" \
    --notes "Release ${VERSION}" \
    --draft
  echo "Created draft release ${VERSION}"
fi

# Step 3: Upload Artifacts
echo ""
echo "Step 3: Uploading artifacts..."

delete_asset() {
  local name="$1"
  gh release delete-asset "$VERSION" "$name" --repo "$REPO" --yes 2>/dev/null || true
}

# ling-mem binary tarball (local platform)
TARBALL="$DIST_DIR/ling-mem-${SLUG}.tar.gz"
if [ -f "$TARBALL" ]; then
  echo "  Uploading: $(basename "$TARBALL")"
  delete_asset "$(basename "$TARBALL")"
  gh release upload "$VERSION" "$TARBALL" --repo "$REPO"
fi

# Linux Artifacts (multi-arch from Docker)
if [ -d "$DIST_DIR/linux" ]; then
  echo "  Uploading Linux artifacts..."
  for file in "$DIST_DIR/linux"/*; do
    if [ -f "$file" ]; then
      echo "    Uploading: $(basename "$file")"
      delete_asset "$(basename "$file")"
      gh release upload "$VERSION" "$file" --repo "$REPO"
    fi
  done
fi

# Step 4: Generate and Upload Manifest
echo ""
echo "Step 4: Generating and uploading manifest..."
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

# Build assets array for the manifest
ASSETS="[]"

# Add current host artifact
if [ -f "$DIST_DIR/ling-mem-${SLUG}.tar.gz" ]; then
  ASSETS=$(echo "$ASSETS" | jq \
    --arg name "ling-mem-${SLUG}" \
    --arg url "${BASE_URL}/ling-mem-${SLUG}.tar.gz" \
    '. + [{"name": $name, "url": $url}]')
fi

# Add Linux artifacts if they exist
if [ -d "$DIST_DIR/linux" ]; then
  for arch in x86_64 aarch64; do
    TAR="ling-mem-linux-${arch}.tar.gz"
    if [ -f "$DIST_DIR/linux/$TAR" ]; then
      ASSETS=$(echo "$ASSETS" | jq \
        --arg name "ling-mem-linux-${arch}" \
        --arg url "${BASE_URL}/$TAR" \
        '. + [{"name": $name, "url": $url}]')
    fi
  done
fi

MANIFEST_JSON=$(jq -n \
  --arg version "${VERSION_NUM}" \
  --argjson assets "$ASSETS" \
  '{version: $version, assets: $assets}')

echo "$MANIFEST_JSON" > "$DIST_DIR/manifest.json"

delete_asset "manifest.json"
gh release upload "$VERSION" "$DIST_DIR/manifest.json" --repo "$REPO"

# Step 5: Finalize
if [ "$KEEP_DRAFT" = "true" ]; then
  echo "Draft release ${VERSION} created."
else
  echo "Publishing release..."
  gh release edit "$VERSION" --draft=false --latest --repo "$REPO"
  echo "Release ${VERSION} published!"
  echo "To install: ling install --memory  (or: ling-mem install)"
fi
