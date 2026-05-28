#!/usr/bin/env bash
#
# SessionStart hook: ensure ling-mem binary + daemon are up.
#
# - On first run (or plugin upgrade with a bumped VERSION), downloads the
#   binary into ${CLAUDE_PLUGIN_DATA}/bin/ (or ${PLUGIN_ROOT}/data/bin/
#   on Codex if CLAUDE_PLUGIN_DATA is unset).
# - Symlinks into ~/.local/bin/ling-mem so the agent's later `Bash` calls
#   find `ling-mem` on PATH.
# - Starts the daemon idempotently.
#
# Exits 0 unconditionally — never blocks the session.

set -u

PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-${PLUGIN_ROOT:-}}"
DATA_DIR="${CLAUDE_PLUGIN_DATA:-$HOME/.linggen/plugin-data}"
mkdir -p "$DATA_DIR/bin"

VERSION="$(cat "$PLUGIN_ROOT/VERSION" 2>/dev/null || echo "v0.7.1")"
BIN="$DATA_DIR/bin/ling-mem"
EXPECTED="${VERSION#v}"
HAVE="$("$BIN" --version 2>/dev/null | awk '{print $2}' || echo none)"

if [ "$HAVE" != "$EXPECTED" ]; then
  bash "$PLUGIN_ROOT/scripts/install-bin.sh" \
    --version "$VERSION" \
    --dest "$DATA_DIR/bin" \
    --quiet >/dev/null 2>&1 || exit 0
fi

# Make the binary discoverable for the agent's Bash subshells.
if [ -x "$BIN" ]; then
  mkdir -p "$HOME/.local/bin" 2>/dev/null
  ln -sf "$BIN" "$HOME/.local/bin/ling-mem" 2>/dev/null || true
fi

command -v ling-mem >/dev/null 2>&1 && ling-mem start >/dev/null 2>&1 || true
exit 0
