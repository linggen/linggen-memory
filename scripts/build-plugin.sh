#!/usr/bin/env bash
#
# Stamp the binary version (from Cargo.toml) into the VERSION file.
#
# After the May 2026 plugin-tree refactor, plugins/linggen/ is a single
# unified tree that both Claude Code and Codex load directly. There is no
# longer a per-host `cc/` or `codex/` build output — the tree IS the artifact.
#
# Two DIFFERENT versions live here; do not conflate them:
#   • Binary version (Cargo.toml) — the `ling-mem` crate/release the plugin
#     fetches. The VERSION file drives the SessionStart binary bootstrap
#     (autostart.sh / install-bin.sh), so it must track Cargo.
#   • Plugin/skill CONTENT version — the manifests
#     (`.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`) and the
#     ClawHub skill. This LEADS the binary version when skill/doc-only
#     changes ship over an unchanged binary (e.g. plugin 0.7.4 over binary
#     0.7.2). ClawHub versions are immutable, so doc fixes force a content
#     bump independent of the binary.
#
# Therefore this script stamps ONLY the VERSION file (binary pin). The plugin
# manifest `version` fields are content-versioned BY HAND and must NOT be
# overwritten from Cargo — doing so reverts intentional content bumps.
#
# Run from the linggen-memory repo root, or via the path-relative invocation.

set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)"/\1/')"
ROOT="plugins/linggen"

# VERSION file drives the SessionStart binary bootstrap (autostart.sh) — it
# tracks the BINARY (Cargo) version, not the plugin content version.
printf 'v%s\n' "$VERSION" > "$ROOT/VERSION"

echo "build-plugin: stamped binary VERSION=v$VERSION (manifest versions are content-versioned by hand, left untouched)"
