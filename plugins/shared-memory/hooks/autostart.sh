#!/usr/bin/env bash
#
# SessionStart hook for shared-memory.
#
# 1. Bootstrap the ling-mem binary on first run (or after a plugin update
#    that bumped VERSION) into ${CLAUDE_PLUGIN_DATA}/bin/. The symlink at
#    ~/.local/bin/ling-mem makes it discoverable to the agent's later
#    Bash subshells.
# 2. Start the daemon idempotently.
# 3. Emit core memory as `hookSpecificOutput.additionalContext` so the
#    host injects always-on identity facts (name, role, location, family,
#    standing-instruction preferences) into the agent's system prompt.
#    CC honors the field natively; Codex ignores unknown JSON and just
#    keeps the side-effect (daemon start). Either way the session never
#    breaks.
#
# Bails silently on any failure — never blocks the session.

set -u

PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-${PLUGIN_ROOT:-}}"
DATA_DIR="${CLAUDE_PLUGIN_DATA:-$HOME/.linggen/plugin-data}"

# If neither host injects a plugin-root env var (e.g. Codex's hook runner
# doesn't expand ${PLUGIN_ROOT} when launching shell hooks), we have no
# stable place to read VERSION or scripts/install-bin.sh from. Bail
# silently — the daemon may still be reachable on whatever the user has
# already installed; nothing here can do useful work blind.
[ -z "$PLUGIN_ROOT" ] && exit 0

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

if [ -x "$BIN" ]; then
  mkdir -p "$HOME/.local/bin" 2>/dev/null
  ln -sf "$BIN" "$HOME/.local/bin/ling-mem" 2>/dev/null || true
fi

# Use $BIN directly — relying on `command -v ling-mem` after just
# creating the symlink would hit the shell's stale PATH cache, and on
# systems where ~/.local/bin is not on PATH it returns false even though
# the binary exists.
[ -x "$BIN" ] && "$BIN" start >/dev/null 2>&1 || true

# ── Inject core memory into the session's system prompt ─────────────────────
#
# Only when jq is available and the daemon answered our query. Empty store
# (fresh install) emits nothing — host gets a normal SessionStart with no
# additionalContext.

command -v jq >/dev/null 2>&1 || exit 0
[ -x "$BIN" ] || exit 0

core_rows="$("$BIN" list --tier core --limit 100 --format json --quiet 2>/dev/null || true)"
[ -z "$core_rows" ] && exit 0

core_block="$(printf '%s' "$core_rows" | jq -sr '
  map(. // empty)
  | flatten
  | map(select(.content))
  | if length == 0 then empty
    else
      "## Core memory — always-on user identity\n\n"
      + (map("- \(.content) (id=\(.id))") | join("\n"))
    end
' 2>/dev/null || true)"

[ -z "$core_block" ] && exit 0

jq -nc --arg ctx "$core_block" '{
  hookSpecificOutput: {
    hookEventName: "SessionStart",
    additionalContext: $ctx
  }
}'
