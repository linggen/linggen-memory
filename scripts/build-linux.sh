#!/bin/bash
# Build ling-mem for Linux (x86_64 and aarch64) using Docker Buildx.
# Output tarballs land in dist/linux/.
#
# Patterned on linggen/linggen/scripts/build-linux.sh.
#
# Usage: ./scripts/build-linux.sh <version>
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

VERSION="${1:-0.0.0}"
VERSION_NUM="${VERSION#v}"

echo "🐳 Building ling-mem for Linux via Docker Buildx (amd64 + arm64)"
echo "   Version: ${VERSION_NUM}"
echo ""

if ! docker buildx version > /dev/null 2>&1; then
    echo "❌ Error: Docker Buildx is not installed or enabled." >&2
    exit 1
fi

# Reuse the same builder name as linggen so users don't accumulate one
# per project — both build similar Rust artifacts.
BUILDER_NAME="linggen-builder"
if ! docker buildx inspect "$BUILDER_NAME" > /dev/null 2>&1; then
    echo "🔧 Creating buildx builder: $BUILDER_NAME"
    docker buildx create --name "$BUILDER_NAME" --use
else
    docker buildx use "$BUILDER_NAME"
fi

mkdir -p dist/linux

echo "🚀 Building ${VERSION_NUM}..."
docker buildx build \
    --platform linux/amd64,linux/arm64 \
    --build-arg "BUILD_VERSION=${VERSION_NUM}" \
    --target artifacts \
    --output type=local,dest=./dist/linux \
    -f scripts/Dockerfile.linux \
    .

# Flatten the per-platform subdirs that buildx creates.
echo "📂 Flattening multi-arch output..."
for platform in linux_amd64 linux_arm64; do
    if [ -d "dist/linux/$platform" ]; then
        mv dist/linux/$platform/*.tar.gz dist/linux/ 2>/dev/null || true
        rmdir "dist/linux/$platform" 2>/dev/null || true
    fi
done

echo ""
echo "✅ Linux build complete."
ls -lh dist/linux/*.tar.gz 2>/dev/null || ls -lh dist/linux/
