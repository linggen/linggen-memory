#!/bin/bash
set -euo pipefail
#
# Release orchestrator — builds ling-mem locally, tars it, uploads to a
# GitHub release. Patterned on linggen/linggen/scripts/release.sh but
# trimmed for our single-crate layout.
#
# Usage:
#   ./scripts/release.sh <version> [--draft] [--skip-build] [--no-upload]
#
#   <version>     Tag name, with or without leading 'v' (e.g. v0.1.0 or 0.1.0).
#   --draft       Leave the GitHub release as a draft; don't publish.
#   --skip-build  Reuse existing dist/ artifacts instead of rebuilding.
#   --no-upload   Build + package locally only; skip GitHub upload entirely.
#
# Requires: cargo, tar, gh (GitHub CLI, authenticated).
# Optional: minisign (for signing the tarball).

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-common.sh"

REPO="linggen/linggen-memory"
VERSION=""
KEEP_DRAFT=false
SKIP_BUILD=false
NO_UPLOAD=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --draft)      KEEP_DRAFT=true;  shift ;;
    --skip-build) SKIP_BUILD=true;  shift ;;
    --no-upload)  NO_UPLOAD=true;   shift ;;
    -h|--help)    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)
      if [ -z "$VERSION" ]; then VERSION="$1"; fi
      shift ;;
  esac
done

if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version> [--draft] [--skip-build] [--no-upload]" >&2
  exit 1
fi

# Normalize version: accept `v0.1.0` or `0.1.0`.
TAG="${VERSION#v}"
TAG="v${TAG}"
VERSION_NUM="${TAG#v}"
DIST_DIR="$ROOT_DIR/dist"
SLUG=$(detect_platform)
TARBALL_NAME="ling-mem-${SLUG}.tar.gz"
TARBALL="$DIST_DIR/$TARBALL_NAME"

echo "─────────────────────────────────────────────────────────"
echo "  linggen-memory release: $TAG  ($SLUG)"
echo "─────────────────────────────────────────────────────────"

# ── Step 1: sync + build ─────────────────────────────────────────────────────

if [ "$SKIP_BUILD" = "true" ]; then
  echo ""
  echo "Step 1/4: Skipping build (--skip-build); reusing $DIST_DIR"
  [ -f "$TARBALL" ] || { echo "Expected $TARBALL but it's missing." >&2; exit 1; }
else
  echo ""
  echo "Step 1/4: Syncing Cargo.toml version to $VERSION_NUM"
  sync_cargo_version "$VERSION_NUM" "$ROOT_DIR/Cargo.toml"

  echo ""
  echo "Step 2/4: cargo build --release"
  rm -rf "$DIST_DIR"
  mkdir -p "$DIST_DIR"
  (cd "$ROOT_DIR" && cargo build --release --locked --bin ling-mem)

  # Verify the built binary reports the version we asked for.
  BUILT_VER=$("$ROOT_DIR/target/release/ling-mem" --version 2>/dev/null | awk '{print $2}' || true)
  if [ "$BUILT_VER" != "$VERSION_NUM" ]; then
    echo "Error: built binary reports '$BUILT_VER', expected '$VERSION_NUM'" >&2
    exit 1
  fi

  # Package: binary + README + LICENSE.
  STAGING="$(mktemp -d)"
  cp "$ROOT_DIR/target/release/ling-mem" "$STAGING/"
  cp "$ROOT_DIR/README.md" "$ROOT_DIR/LICENSE" "$STAGING/"
  (cd "$STAGING" && tar czf "$TARBALL" .)
  rm -rf "$STAGING"

  # Checksum (for the release notes page).
  (cd "$DIST_DIR" && shasum -a 256 "$TARBALL_NAME" > "${TARBALL_NAME}.sha256")

  # Optional signing.
  SIG=$(sign_file "$TARBALL" "$ROOT_DIR" 2>/dev/null) || true
  if [ -n "${SIG:-}" ]; then
    echo "$SIG" > "${TARBALL}.sig.txt"
    echo "  tarball signed → ${TARBALL_NAME}.sig.txt"
  fi

  echo "  built → $TARBALL ($(du -h "$TARBALL" | awk '{print $1}'))"
fi

# ── Step 3: GitHub release ───────────────────────────────────────────────────

if [ "$NO_UPLOAD" = "true" ]; then
  echo ""
  echo "Step 3/4: Skipping GitHub upload (--no-upload)."
  echo ""
  echo "Artifacts in $DIST_DIR:"
  ls -la "$DIST_DIR"
  exit 0
fi

echo ""
echo "Step 3/4: Ensuring GitHub release $TAG exists"
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

echo ""
echo "Step 4/4: Uploading $TARBALL_NAME"
UPLOAD_ARGS=("$TARBALL" "${TARBALL}.sha256")
[ -f "${TARBALL}.sig.txt" ] && UPLOAD_ARGS+=("${TARBALL}.sig.txt")
gh release upload "$TAG" "${UPLOAD_ARGS[@]}" --repo "$REPO" --clobber

# ── Finalize ─────────────────────────────────────────────────────────────────

if [ "$KEEP_DRAFT" = "true" ]; then
  echo ""
  echo "Done. Draft release $TAG created (not published)."
  echo "  Publish with: gh release edit $TAG --draft=false --latest --repo $REPO"
else
  gh release edit "$TAG" --draft=false --latest --repo "$REPO" >/dev/null
  echo ""
  echo "Done. Release $TAG published."
fi

echo "  https://github.com/${REPO}/releases/tag/${TAG}"
