#!/bin/bash
set -euo pipefail
#
# Release orchestrator for linggen-memory (`ling-mem` binary).
#
# Builds the chosen platform's artifacts, packages them, then uploads to a
# GitHub release. Patterned on linggen/linggen/scripts/release.sh — same
# `--platform mac|linux` flag, same draft/publish flow.
#
# Usage:
#   ./scripts/release.sh <version> [--draft] [--platform mac|linux]
#                                  [--skip-build] [--no-upload]
#
#   <version>      Tag name, with or without leading 'v' (e.g. v0.4.0 or 0.4.0).
#   --draft        Leave the GitHub release as a draft; don't publish.
#   --platform     mac (host must be macOS) | linux (Docker buildx required).
#                  Default = current host (no cross-build).
#   --skip-build   Reuse existing dist/ artifacts instead of rebuilding.
#   --no-upload    Build + package locally only; skip GitHub upload entirely.
#
# Requires: cargo, tar, gh (GitHub CLI, authenticated).
# Optional: docker buildx (for --platform linux).

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-common.sh"

REPO="linggen/linggen-memory"
VERSION=""
KEEP_DRAFT=false
SKIP_BUILD=false
NO_UPLOAD=false
PLATFORM=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --draft)       KEEP_DRAFT=true;  shift ;;
    --skip-build)  SKIP_BUILD=true;  shift ;;
    --no-upload)   NO_UPLOAD=true;   shift ;;
    --platform)    PLATFORM="${2:-}"; shift 2 ;;
    --platform=*)  PLATFORM="${1#--platform=}"; shift ;;
    -h|--help)     sed -n '3,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)
      if [ -z "$VERSION" ]; then VERSION="$1"; fi
      shift ;;
  esac
done

if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version> [--draft] [--platform mac|linux] [--skip-build] [--no-upload]" >&2
  exit 1
fi

OS_LOWER="$(uname -s | tr '[:upper:]' '[:lower:]')"
HOST_PLATFORM="$([ "$OS_LOWER" = "darwin" ] && echo mac || echo linux)"
PLATFORM="${PLATFORM:-$HOST_PLATFORM}"

case "$PLATFORM" in
  mac|linux) ;;
  *)
    echo "Error: --platform must be 'mac' or 'linux' (got '$PLATFORM')" >&2
    exit 1 ;;
esac

if [ "$PLATFORM" = "mac" ] && [ "$OS_LOWER" != "darwin" ]; then
  echo "Error: --platform mac requires a macOS host" >&2
  exit 1
fi
if [ "$PLATFORM" = "linux" ] && ! (command -v docker >/dev/null && docker buildx version >/dev/null 2>&1); then
  echo "Error: --platform linux requires Docker + buildx" >&2
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
echo "  platform: $PLATFORM"
echo "─────────────────────────────────────────────────────────"

# ── Step 1: build ────────────────────────────────────────────────────────────

if [ "$SKIP_BUILD" = "true" ]; then
  echo ""
  echo "Step 1/4: Skipping build (--skip-build); reusing $DIST_DIR"
  if [ "$PLATFORM" = "mac" ]; then
    [ -f "$HOST_TARBALL" ] || { echo "Expected $HOST_TARBALL but it's missing." >&2; exit 1; }
  else
    [ -d "$DIST_DIR/linux" ] || { echo "Expected $DIST_DIR/linux/ but it's missing." >&2; exit 1; }
  fi
elif [ "$PLATFORM" = "mac" ]; then
  echo ""
  echo "Step 1a/4: Syncing Cargo.toml version to $VERSION_NUM"
  sync_cargo_version "$VERSION_NUM" "$ROOT_DIR/Cargo.toml"
  # Refresh Cargo.lock so the build reflects the bumped Cargo.toml.
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
  echo "  built → $HOST_TARBALL ($(du -h "$HOST_TARBALL" | awk '{print $1}'))"
else
  # PLATFORM = linux
  echo ""
  echo "Step 1a/4: Syncing Cargo.toml version to $VERSION_NUM"
  sync_cargo_version "$VERSION_NUM" "$ROOT_DIR/Cargo.toml"
  (cd "$ROOT_DIR" && cargo update --workspace --offline >/dev/null 2>&1) || true

  echo ""
  echo "Step 1b/4: Linux multi-arch via Docker Buildx (amd64 + arm64)"
  rm -rf "$DIST_DIR/linux"
  mkdir -p "$DIST_DIR/linux"
  "$ROOT_DIR/scripts/build-linux.sh" "$TAG"

  for tarball in "$DIST_DIR"/linux/ling-mem-linux-*.tar.gz; do
    [ -f "$tarball" ] || continue
    base="$(basename "$tarball")"
    (cd "$DIST_DIR/linux" && shasum -a 256 "$base" > "${base}.sha256")
  done
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

# ── Step 3: upload artifacts for the chosen platform ─────────────────────────

echo ""
echo "Step 3/4: Uploading artifacts"

upload_one() {
  local file="$1"
  [ -f "$file" ] || return 0
  echo "  → $(basename "$file")"
  gh release upload "$TAG" "$file" --repo "$REPO" --clobber
}

HAS_MAC_TARBALL=false
HAS_LINUX_DIR=false
[ "$PLATFORM" = "mac" ] && [ -f "$HOST_TARBALL" ] && HAS_MAC_TARBALL=true
[ -d "$DIST_DIR/linux" ] && HAS_LINUX_DIR=true

if [ "$HAS_MAC_TARBALL" = "false" ] && [ "$HAS_LINUX_DIR" = "false" ]; then
  echo "Error: no artifacts to upload — did the build step produce anything?" >&2
  exit 1
fi

if [ "$HAS_MAC_TARBALL" = "true" ]; then
  upload_one "$HOST_TARBALL"
  upload_one "${HOST_TARBALL}.sha256"
fi

if [ "$HAS_LINUX_DIR" = "true" ]; then
  for arch in x86_64 aarch64; do
    base="ling-mem-linux-${arch}.tar.gz"
    upload_one "$DIST_DIR/linux/$base"
    upload_one "$DIST_DIR/linux/${base}.sha256"
  done
fi

# ── Step 4: finalize ─────────────────────────────────────────────────────────

echo ""
echo "Step 4/4: Finalize"
if [ "$KEEP_DRAFT" = "true" ]; then
  echo "  Done. Draft release $TAG (not published)."
  echo "  Publish with: gh release edit $TAG --draft=false --latest --repo $REPO"
else
  gh release edit "$TAG" --draft=false --latest --repo "$REPO" >/dev/null
  echo "  Done. Release $TAG published."
fi
echo "  https://github.com/${REPO}/releases/tag/${TAG}"
