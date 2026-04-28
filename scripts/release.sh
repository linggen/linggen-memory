#!/bin/bash
set -euo pipefail
#
# Release orchestrator — builds ling-mem locally (host platform) and via
# Docker Buildx for Linux x86_64 + aarch64, packages the artifacts, then
# uploads them to a GitHub release.
#
# Patterned on linggen/linggen/scripts/release.sh + build.sh, collapsed
# into a single orchestrator since this crate has no UI build step.
#
# Usage:
#   ./scripts/release.sh <version> [--draft] [--skip-build] [--skip-linux] [--no-upload]
#
#   <version>     Tag name, with or without leading 'v' (e.g. v0.2.1 or 0.2.1).
#   --draft       Leave the GitHub release as a draft; don't publish.
#   --skip-build  Reuse existing dist/ artifacts instead of rebuilding.
#   --skip-linux  Skip the multi-arch Docker Linux build (host only).
#   --no-upload   Build + package locally only; skip GitHub upload entirely.
#
# Requires: cargo, tar, gh (GitHub CLI, authenticated).
# Optional: minisign (for signing tarballs), docker buildx (for Linux).

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-common.sh"

REPO="linggen/linggen-memory"
VERSION=""
KEEP_DRAFT=false
SKIP_BUILD=false
SKIP_LINUX=false
NO_UPLOAD=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --draft)      KEEP_DRAFT=true;  shift ;;
    --skip-build) SKIP_BUILD=true;  shift ;;
    --skip-linux) SKIP_LINUX=true;  shift ;;
    --no-upload)  NO_UPLOAD=true;   shift ;;
    -h|--help)    sed -n '3,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)
      if [ -z "$VERSION" ]; then VERSION="$1"; fi
      shift ;;
  esac
done

if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version> [--draft] [--skip-build] [--skip-linux] [--no-upload]" >&2
  exit 1
fi

# Normalize version: accept `v0.1.0` or `0.1.0`.
TAG="${VERSION#v}"
TAG="v${TAG}"
VERSION_NUM="${TAG#v}"
DIST_DIR="$ROOT_DIR/dist"
HOST_SLUG=$(detect_platform)
HOST_TARBALL_NAME="ling-mem-${HOST_SLUG}.tar.gz"
HOST_TARBALL="$DIST_DIR/$HOST_TARBALL_NAME"

echo "─────────────────────────────────────────────────────────"
echo "  linggen-memory release: $TAG"
echo "  host: $HOST_SLUG · linux: $([ "$SKIP_LINUX" = "true" ] && echo "skipped" || echo "amd64+arm64")"
echo "─────────────────────────────────────────────────────────"

# ── Step 1: build (host + optional Linux) ────────────────────────────────────

if [ "$SKIP_BUILD" = "true" ]; then
  echo ""
  echo "Step 1/4: Skipping build (--skip-build); reusing $DIST_DIR"
  [ -f "$HOST_TARBALL" ] || { echo "Expected $HOST_TARBALL but it's missing." >&2; exit 1; }
else
  echo ""
  echo "Step 1a/4: Syncing Cargo.toml version to $VERSION_NUM"
  sync_cargo_version "$VERSION_NUM" "$ROOT_DIR/Cargo.toml"
  # Refresh Cargo.lock so the host build (and the Docker COPY of Cargo.lock)
  # match the bumped Cargo.toml. Use --offline to avoid needing the network.
  (cd "$ROOT_DIR" && cargo update --workspace --offline >/dev/null 2>&1) || true

  echo ""
  echo "Step 1b/4: cargo build --release (host: $HOST_SLUG)"
  rm -rf "$DIST_DIR"
  mkdir -p "$DIST_DIR"
  (cd "$ROOT_DIR" && cargo build --release --bin ling-mem)

  BUILT_VER=$("$ROOT_DIR/target/release/ling-mem" --version 2>/dev/null | awk '{print $2}' || true)
  if [ "$BUILT_VER" != "$VERSION_NUM" ]; then
    echo "Error: built binary reports '$BUILT_VER', expected '$VERSION_NUM'" >&2
    exit 1
  fi

  STAGING="$(mktemp -d)"
  cp "$ROOT_DIR/target/release/ling-mem" "$STAGING/"
  cp "$ROOT_DIR/README.md" "$ROOT_DIR/LICENSE" "$STAGING/"
  (cd "$STAGING" && tar czf "$HOST_TARBALL" .)
  rm -rf "$STAGING"

  (cd "$DIST_DIR" && shasum -a 256 "$HOST_TARBALL_NAME" > "${HOST_TARBALL_NAME}.sha256")

  HOST_SIG=$(sign_file "$HOST_TARBALL" "$ROOT_DIR" 2>/dev/null) || true
  if [ -n "${HOST_SIG:-}" ]; then
    echo "$HOST_SIG" > "${HOST_TARBALL}.sig.txt"
    echo "  tarball signed → ${HOST_TARBALL_NAME}.sig.txt"
  fi
  echo "  built → $HOST_TARBALL ($(du -h "$HOST_TARBALL" | awk '{print $1}'))"

  echo ""
  if [ "$SKIP_LINUX" = "true" ]; then
    echo "Step 1c/4: Skipping Linux multi-arch build (--skip-linux)"
  elif command -v docker >/dev/null && docker buildx version >/dev/null 2>&1; then
    echo "Step 1c/4: Linux multi-arch via Docker Buildx (amd64 + arm64)"
    "$ROOT_DIR/scripts/build-linux.sh" "$TAG"

    # Sign + checksum each Linux tarball that landed.
    for tarball in "$DIST_DIR"/linux/ling-mem-linux-*.tar.gz; do
      [ -f "$tarball" ] || continue
      base="$(basename "$tarball")"
      (cd "$DIST_DIR/linux" && shasum -a 256 "$base" > "${base}.sha256")
      sig=$(sign_file "$tarball" "$ROOT_DIR" 2>/dev/null) || true
      if [ -n "${sig:-}" ]; then
        echo "$sig" > "${tarball}.sig.txt"
        echo "  linux tarball signed → ${base}.sig.txt"
      fi
    done
  else
    echo "Step 1c/4: ⚠️  Docker Buildx unavailable — skipping Linux build."
    echo "         Pass --skip-linux to silence this warning, or install Docker."
  fi
fi

# ── Step 2: GitHub release ───────────────────────────────────────────────────

if [ "$NO_UPLOAD" = "true" ]; then
  echo ""
  echo "Step 2/4: Skipping GitHub upload (--no-upload)."
  echo ""
  echo "Artifacts in $DIST_DIR:"
  find "$DIST_DIR" -type f
  exit 0
fi

echo ""
echo "Step 2/4: Ensuring GitHub release $TAG exists"
if gh release view "$TAG" --repo "$REPO" &>/dev/null; then
  echo "  release $TAG already exists — will replace assets"
else
  gh release create "$TAG" \
    --repo "$REPO" \
    --title "linggen-memory $TAG" \
    --notes "Release $TAG" \
    --draft
  echo "  created draft release $TAG"
fi

# ── Step 3: upload all artifacts ─────────────────────────────────────────────

echo ""
echo "Step 3/4: Uploading artifacts"

upload_one() {
  local file="$1"
  [ -f "$file" ] || return 0
  echo "  → $(basename "$file")"
  gh release upload "$TAG" "$file" --repo "$REPO" --clobber
}

# Host (mac arm/x86 or local linux)
upload_one "$HOST_TARBALL"
upload_one "${HOST_TARBALL}.sha256"
upload_one "${HOST_TARBALL}.sig.txt"

# Linux multi-arch
if [ -d "$DIST_DIR/linux" ]; then
  for arch in x86_64 aarch64; do
    base="ling-mem-linux-${arch}.tar.gz"
    upload_one "$DIST_DIR/linux/$base"
    upload_one "$DIST_DIR/linux/${base}.sha256"
    upload_one "$DIST_DIR/linux/${base}.sig.txt"
  done
fi

# ── Step 4: finalize ─────────────────────────────────────────────────────────

echo ""
echo "Step 4/4: Finalize"
if [ "$KEEP_DRAFT" = "true" ]; then
  echo "  Done. Draft release $TAG created (not published)."
  echo "  Publish with: gh release edit $TAG --draft=false --latest --repo $REPO"
else
  gh release edit "$TAG" --draft=false --latest --repo "$REPO" >/dev/null
  echo "  Done. Release $TAG published."
fi
echo "  https://github.com/${REPO}/releases/tag/${TAG}"
