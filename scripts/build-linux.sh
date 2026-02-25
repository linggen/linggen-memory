#!/bin/bash
# Build ling-mem for Linux (x86_64 and arm64) using Docker Buildx

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

echo "Building Linux packages using Docker Buildx..."
echo "   Target architectures: amd64 (x86_64), arm64 (aarch64)"
echo "   Artifact: ling-mem (Linggen Memory with embedded Web UI)"
echo ""

# Ensure buildx is available and set up
if ! docker buildx version > /dev/null 2>&1; then
    echo "Error: Docker Buildx is not installed or enabled."
    exit 1
fi

# Create and use a new builder if needed (supports multi-arch)
BUILDER_NAME="linggen-builder"
if ! docker buildx inspect "$BUILDER_NAME" > /dev/null 2>&1; then
    echo "Creating new Buildx builder: $BUILDER_NAME"
    docker buildx create --name "$BUILDER_NAME" --use
else
    docker buildx use "$BUILDER_NAME"
fi

# Ensure output directory exists
mkdir -p dist/linux

# Run the build
VERSION="${1:-0.0.0}"
VERSION_NUM="${VERSION#v}"

echo "Starting build process for version ${VERSION_NUM}..."
docker buildx build \
    --platform linux/amd64,linux/arm64 \
    --build-arg "BUILD_VERSION=${VERSION_NUM}" \
    --target artifacts \
    --output type=local,dest=./dist/linux \
    -f scripts/Dockerfile.linux \
    .

# Flatten the multi-arch output from buildx
echo "Flattening multi-arch artifacts..."
if [ -d "dist/linux/linux_amd64" ]; then
    mv dist/linux/linux_amd64/*.tar.gz dist/linux/ 2>/dev/null || true
    rmdir dist/linux/linux_amd64 2>/dev/null || true
fi
if [ -d "dist/linux/linux_arm64" ]; then
    mv dist/linux/linux_arm64/*.tar.gz dist/linux/ 2>/dev/null || true
    rmdir dist/linux/linux_arm64 2>/dev/null || true
fi

echo ""
echo "Linux build complete! Packages are in dist/linux/"
echo ""
echo "Files found:"
ls -lh dist/linux/*.tar.gz 2>/dev/null || ls -lh dist/linux/

echo ""
echo "To install: ling install --memory  (or: ling-mem install)"
