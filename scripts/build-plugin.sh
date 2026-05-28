#!/usr/bin/env bash
#
# Stamp the current Cargo.toml version into both plugin manifests in place.
#
# After the May 2026 plugin-tree refactor, plugins/shared-memory/ is a single
# unified tree that both Claude Code and Codex load directly. There is no
# longer a per-host `cc/` or `codex/` build output — the tree IS the artifact.
# This script just keeps the two manifest versions in sync with the Rust crate.
#
# Manifests stamped:
#   plugins/shared-memory/.claude-plugin/plugin.json   (Claude Code)
#   plugins/shared-memory/plugin.json                  (Codex)
#
# Run from the linggen-memory repo root, or via the path-relative invocation.

set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)"/\1/')"
ROOT="plugins/shared-memory"
CC_MANIFEST="$ROOT/.claude-plugin/plugin.json"
CX_MANIFEST="$ROOT/plugin.json"

[ -f "$CC_MANIFEST" ] || { echo "missing $CC_MANIFEST" >&2; exit 1; }
[ -f "$CX_MANIFEST" ] || { echo "missing $CX_MANIFEST" >&2; exit 1; }

stamp() {
  local file="$1"
  local tmp="$file.tmp.$$"
  sed -E 's/("version"[[:space:]]*:[[:space:]]*")[^"]+(")/\1'"$VERSION"'\2/' \
    "$file" > "$tmp"
  mv "$tmp" "$file"
}

stamp "$CC_MANIFEST"
stamp "$CX_MANIFEST"

# VERSION file drives the SessionStart binary bootstrap (autostart.sh).
printf 'v%s\n' "$VERSION" > "$ROOT/VERSION"

echo "build-plugin: stamped version=$VERSION into both manifests + VERSION"
