#!/bin/bash
set -euo pipefail

# Sync version to all project files
# Usage: ./scripts/sync-version.sh <version>
#        Version should be without 'v' prefix (e.g., "0.7.0")

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "Usage: $0 <version>" >&2
  echo "Example: $0 0.7.0" >&2
  exit 1
fi

# Remove 'v' prefix if present
VERSION="${VERSION#v}"

echo "Syncing version $VERSION to all project files..."

# Update backend/Cargo.toml (workspace version)
if [ -f "$ROOT_DIR/backend/Cargo.toml" ]; then
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "s/^version = \"[^\"]*\"/version = \"$VERSION\"/" "$ROOT_DIR/backend/Cargo.toml"
  else
    sed -i "s/^version = \"[^\"]*\"/version = \"$VERSION\"/" "$ROOT_DIR/backend/Cargo.toml"
  fi
  echo "  Updated backend/Cargo.toml"

  # Update workspace Cargo.lock
  (cd "$ROOT_DIR/backend" && cargo fetch 2>/dev/null || true)
  echo "  Updated backend/Cargo.lock"
fi

# Update frontend/package.json
if [ -f "$ROOT_DIR/frontend/package.json" ]; then
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "s/\"version\": \"[^\"]*\"/\"version\": \"$VERSION\"/" "$ROOT_DIR/frontend/package.json"
  else
    sed -i "s/\"version\": \"[^\"]*\"/\"version\": \"$VERSION\"/" "$ROOT_DIR/frontend/package.json"
  fi
  echo "  Updated frontend/package.json"
fi

echo "Version sync complete!"
