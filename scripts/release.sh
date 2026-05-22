#!/bin/bash
set -euo pipefail
#
# Release orchestrator for linggen-memory (`ling-mem` binary).
#
# Builds, packages, and publishes a single-arch or multi-arch release. The
# --platform flag is one value combining OS + arch; each value names a complete
# release target.
#
# Usage:
#   ./scripts/release.sh <version> [--platform <target>]
#                                  [--draft] [--skip-build] [--no-upload]
#
#   <version>     Tag name, e.g. v0.4.0 or 0.4.0.
#   --platform    Target, one of:
#                   mac_arm   — macOS aarch64 (cargo, native or rustup-cross)
#                   mac_x64   — macOS x86_64  (cargo, native or rustup-cross)
#                   linux_x64 — Linux x86_64  (native cargo if host=x86_64,
#                               else Docker buildx single-arch)
#                   linux_arm — Linux aarch64 (native cargo if host=aarch64,
#                               else Docker buildx single-arch)
#                   linux_all — Linux multi-arch (Docker buildx amd64+arm64)
#                 Default = the host's platform (always native, fastest).
#   --draft       Leave the GitHub release as a draft; don't publish.
#   --skip-build  Reuse existing dist/ artifacts instead of rebuilding.
#   --no-upload   Build + package locally only; skip GitHub upload entirely.
#
# Requires: cargo, tar, gh (GitHub CLI, authenticated).
# Optional: docker buildx (for off-host linux targets and linux_all).
# Optional: rustup target add (for cross-compiling mac_x64 ↔ mac_arm).

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
    -h|--help)     sed -n '3,29p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)
      if [ -z "$VERSION" ]; then VERSION="$1"; fi
      shift ;;
  esac
done

if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version> [--platform mac_arm|mac_x64|linux_x64|linux_arm|linux_all] [--draft] [--skip-build] [--no-upload]" >&2
  exit 1
fi

# ── Resolve host + chosen platform ───────────────────────────────────────────

OS_LOWER="$(uname -s | tr '[:upper:]' '[:lower:]')"
HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
  arm64|aarch64) HOST_ARCH_NORM="arm" ;;
  x86_64|amd64)  HOST_ARCH_NORM="x64" ;;
  *) echo "Unsupported host arch: $HOST_ARCH" >&2; exit 1 ;;
esac
case "$OS_LOWER" in
  darwin) HOST_OS="mac" ;;
  linux)  HOST_OS="linux" ;;
  *) echo "Unsupported host OS: $OS_LOWER" >&2; exit 1 ;;
esac
HOST_PLATFORM="${HOST_OS}_${HOST_ARCH_NORM}"
PLATFORM="${PLATFORM:-$HOST_PLATFORM}"

case "$PLATFORM" in
  mac_arm|mac_x64|linux_x64|linux_arm|linux_all) ;;
  *)
    echo "Error: --platform must be one of: mac_arm, mac_x64, linux_x64, linux_arm, linux_all (got '$PLATFORM')" >&2
    exit 1 ;;
esac

# Map --platform → target triple, tarball name, output dir.
case "$PLATFORM" in
  mac_arm)   TARGET_TRIPLE="aarch64-apple-darwin";       TARBALL_BASENAME="ling-mem-macos-aarch64.tar.gz";  PLATFORM_OS="mac";   IS_MULTI=false ;;
  mac_x64)   TARGET_TRIPLE="x86_64-apple-darwin";        TARBALL_BASENAME="ling-mem-macos-x86_64.tar.gz";   PLATFORM_OS="mac";   IS_MULTI=false ;;
  linux_arm) TARGET_TRIPLE="aarch64-unknown-linux-gnu";  TARBALL_BASENAME="ling-mem-linux-aarch64.tar.gz";  PLATFORM_OS="linux"; IS_MULTI=false ;;
  linux_x64) TARGET_TRIPLE="x86_64-unknown-linux-gnu";   TARBALL_BASENAME="ling-mem-linux-x86_64.tar.gz";   PLATFORM_OS="linux"; IS_MULTI=false ;;
  linux_all) TARGET_TRIPLE="";                           TARBALL_BASENAME="";                              PLATFORM_OS="linux"; IS_MULTI=true  ;;
esac

# Decide whether this platform builds natively or needs Docker.
USES_NATIVE=false
if [ "$IS_MULTI" = "false" ] && [ "$PLATFORM" = "$HOST_PLATFORM" ]; then
  USES_NATIVE=true
fi
# Mac targets always use cargo (native or rustup-cross). Off-host requires
# the rustup target installed.
if [ "$PLATFORM_OS" = "mac" ]; then
  if [ "$OS_LOWER" != "darwin" ]; then
    echo "Error: --platform $PLATFORM requires a macOS host" >&2; exit 1
  fi
  USES_NATIVE=true
fi

# Linux off-host targets need Docker buildx.
if [ "$PLATFORM_OS" = "linux" ] && [ "$USES_NATIVE" = "false" ]; then
  if ! (command -v docker >/dev/null && docker buildx version >/dev/null 2>&1); then
    echo "Error: --platform $PLATFORM on this host requires Docker + buildx" >&2; exit 1
  fi
fi

# Normalize version: accept `v0.1.0` or `0.1.0`.
TAG="${VERSION#v}"
TAG="v${TAG}"
VERSION_NUM="${TAG#v}"
DIST_DIR="$ROOT_DIR/dist"
LINUX_DIR="$DIST_DIR/linux"

echo "─────────────────────────────────────────────────────────"
echo "  linggen-memory release: $TAG"
echo "  platform: $PLATFORM (host: $HOST_PLATFORM)"
if [ "$IS_MULTI" = "true" ]; then
  echo "  build: Docker buildx (amd64 + arm64)"
elif [ "$USES_NATIVE" = "true" ]; then
  echo "  build: cargo (native)"
else
  echo "  build: Docker buildx (single-arch cross)"
fi
echo "─────────────────────────────────────────────────────────"

# ── Step 1: build ────────────────────────────────────────────────────────────

build_native_cargo() {
  # Build with `cargo build --release [--target $TARGET_TRIPLE]`. Uses --target
  # only when cross-compiling; the binary path differs accordingly.
  local need_target=false
  if [ "$PLATFORM_OS" = "mac" ] && [ "$PLATFORM" != "$HOST_PLATFORM" ]; then
    need_target=true
    if ! rustup target list --installed 2>/dev/null | grep -q "^${TARGET_TRIPLE}$"; then
      echo "  installing rustup target: $TARGET_TRIPLE"
      rustup target add "$TARGET_TRIPLE"
    fi
  fi

  if [ "$need_target" = "true" ]; then
    (cd "$ROOT_DIR" && cargo build --release --bin ling-mem --target "$TARGET_TRIPLE")
    BIN_PATH="$ROOT_DIR/target/$TARGET_TRIPLE/release/ling-mem"
  else
    (cd "$ROOT_DIR" && cargo build --release --bin ling-mem)
    BIN_PATH="$ROOT_DIR/target/release/ling-mem"
  fi

  # Re-sign mac binaries adhoc. cargo emits linker-signed binaries whose
  # signature macOS 26.x (Sequoia+) rejects with `SIGKILL Code Signature
  # Invalid` on first launch from /usr/local/bin or any copy-target. A
  # fresh adhoc signature via `codesign -s -` is enough — no Developer ID
  # needed for ling-mem's local-install distribution model.
  if [ "$PLATFORM_OS" = "mac" ]; then
    codesign --force --sign - "$BIN_PATH"
  fi

  # Verify the binary reports the expected version (skip on cross-compile —
  # binary may not be runnable on the build host).
  if [ "$need_target" = "false" ]; then
    local built_ver
    built_ver=$("$BIN_PATH" --version 2>/dev/null | awk '{print $2}' || true)
    if [ "$built_ver" != "$VERSION_NUM" ]; then
      echo "Error: built binary reports '$built_ver', expected '$VERSION_NUM'" >&2
      exit 1
    fi
  fi
}

package_tarball() {
  # Package $BIN_PATH + README + LICENSE → $1 (full path).
  local out_path="$1"
  local out_dir
  out_dir="$(dirname "$out_path")"
  local out_base
  out_base="$(basename "$out_path")"
  mkdir -p "$out_dir"

  local staging
  staging="$(mktemp -d)"
  cp "$BIN_PATH" "$staging/"
  cp "$ROOT_DIR/README.md" "$ROOT_DIR/LICENSE" "$staging/"
  (cd "$staging" && tar czf "$out_path" .)
  rm -rf "$staging"
  (cd "$out_dir" && shasum -a 256 "$out_base" > "${out_base}.sha256")
  echo "  built → $out_path ($(du -h "$out_path" | awk '{print $1}'))"
}

# Determine output dir + tarball path for single-arch targets.
OUT_DIR="$DIST_DIR"
if [ "$PLATFORM_OS" = "linux" ]; then
  OUT_DIR="$LINUX_DIR"
fi
OUT_TARBALL="$OUT_DIR/$TARBALL_BASENAME"

if [ "$SKIP_BUILD" = "true" ]; then
  echo ""
  echo "Step 1/4: Skipping build (--skip-build); reusing $DIST_DIR"
  if [ "$IS_MULTI" = "true" ]; then
    [ -d "$LINUX_DIR" ] || { echo "Expected $LINUX_DIR/ but it's missing." >&2; exit 1; }
  else
    [ -f "$OUT_TARBALL" ] || { echo "Expected $OUT_TARBALL but it's missing." >&2; exit 1; }
  fi
elif [ "$IS_MULTI" = "true" ]; then
  echo ""
  echo "Step 1a/4: Syncing Cargo.toml version to $VERSION_NUM"
  sync_cargo_version "$VERSION_NUM" "$ROOT_DIR/Cargo.toml"
  (cd "$ROOT_DIR" && cargo update --workspace --offline >/dev/null 2>&1) || true

  echo ""
  echo "Step 1b/4: Linux multi-arch via Docker Buildx (amd64 + arm64)"
  rm -rf "$LINUX_DIR"
  mkdir -p "$LINUX_DIR"
  "$ROOT_DIR/scripts/build-linux.sh" "$TAG"

  for tarball in "$LINUX_DIR"/ling-mem-linux-*.tar.gz; do
    [ -f "$tarball" ] || continue
    base="$(basename "$tarball")"
    (cd "$LINUX_DIR" && shasum -a 256 "$base" > "${base}.sha256")
  done
elif [ "$USES_NATIVE" = "true" ]; then
  echo ""
  echo "Step 1a/4: Syncing Cargo.toml version to $VERSION_NUM"
  sync_cargo_version "$VERSION_NUM" "$ROOT_DIR/Cargo.toml"
  (cd "$ROOT_DIR" && cargo update --workspace --offline >/dev/null 2>&1) || true

  echo ""
  echo "Step 1b/4: cargo build --release ($PLATFORM)"
  rm -rf "$OUT_DIR"
  build_native_cargo
  package_tarball "$OUT_TARBALL"
else
  # Linux off-host single-arch: Docker buildx with one --platform.
  echo ""
  echo "Step 1a/4: Syncing Cargo.toml version to $VERSION_NUM"
  sync_cargo_version "$VERSION_NUM" "$ROOT_DIR/Cargo.toml"
  (cd "$ROOT_DIR" && cargo update --workspace --offline >/dev/null 2>&1) || true

  echo ""
  echo "Step 1b/4: Docker Buildx single-arch ($PLATFORM)"
  rm -rf "$LINUX_DIR"
  mkdir -p "$LINUX_DIR"

  case "$PLATFORM" in
    linux_x64) BUILDX_PLATFORM="linux/amd64" ;;
    linux_arm) BUILDX_PLATFORM="linux/arm64" ;;
  esac

  BUILDER_NAME="linggen-builder"
  if ! docker buildx inspect "$BUILDER_NAME" > /dev/null 2>&1; then
    docker buildx create --name "$BUILDER_NAME" --use
  else
    docker buildx use "$BUILDER_NAME"
  fi

  (cd "$ROOT_DIR" && docker buildx build \
    --platform "$BUILDX_PLATFORM" \
    --build-arg "BUILD_VERSION=${VERSION_NUM}" \
    --target artifacts \
    --output "type=local,dest=$LINUX_DIR" \
    -f scripts/Dockerfile.linux .)

  # Flatten the per-platform subdir buildx may create.
  for sub in linux_amd64 linux_arm64; do
    if [ -d "$LINUX_DIR/$sub" ]; then
      mv "$LINUX_DIR/$sub"/*.tar.gz "$LINUX_DIR/" 2>/dev/null || true
      rmdir "$LINUX_DIR/$sub" 2>/dev/null || true
    fi
  done

  for tarball in "$LINUX_DIR"/ling-mem-linux-*.tar.gz; do
    [ -f "$tarball" ] || continue
    base="$(basename "$tarball")"
    (cd "$LINUX_DIR" && shasum -a 256 "$base" > "${base}.sha256")
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

# ── Step 3: upload artifacts ─────────────────────────────────────────────────

echo ""
echo "Step 3/4: Uploading artifacts"

upload_one() {
  local file="$1"
  [ -f "$file" ] || return 0
  echo "  → $(basename "$file")"
  gh release upload "$TAG" "$file" --repo "$REPO" --clobber
}

# Upload every tarball + sha256 produced by this run, regardless of which
# platform was built. The build step only writes to the relevant dirs, so
# this naturally scopes to the chosen target.
shopt -s nullglob
ANY_UPLOADED=false
for f in "$DIST_DIR"/*.tar.gz "$DIST_DIR"/*.tar.gz.sha256 \
         "$LINUX_DIR"/*.tar.gz "$LINUX_DIR"/*.tar.gz.sha256; do
  upload_one "$f"
  ANY_UPLOADED=true
done

if [ "$ANY_UPLOADED" = "false" ]; then
  echo "Error: no artifacts to upload — did the build step produce anything?" >&2
  exit 1
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
