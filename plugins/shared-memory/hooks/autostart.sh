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

VERSION="$(cat "$PLUGIN_ROOT/VERSION" 2>/dev/null || echo "v0.7.2")"
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

# Reconcile the RUNNING daemon with the pinned binary. A plugin/skill update
# bumps VERSION and swaps the binary on disk above, but a `start` is a no-op
# while a daemon is already bound to the port — so the old in-memory version
# keeps serving until something restarts it. Decide from the live daemon:
#   not running      -> start
#   running, older    -> restart onto the freshly-pinned binary
#   running, >= pin   -> leave it (don't let a stale-pinned host downgrade a
#                        newer daemon that another channel already started)
# $BIN is used directly: a freshly-created ~/.local/bin symlink would miss the
# shell's stale PATH cache, and where ~/.local/bin isn't on PATH a
# `command -v ling-mem` check returns false even though the binary exists.
if [ -x "$BIN" ]; then
  _st="$("$BIN" status --format json 2>/dev/null)"
  _state="$(printf '%s' "$_st" | jq -r '.state // empty' 2>/dev/null)"
  _running_ver="$(printf '%s' "$_st" | jq -r '.version // empty' 2>/dev/null)"
  if [ "$_state" != "running" ]; then
    "$BIN" start >/dev/null 2>&1 || true
  elif [ -n "$_running_ver" ] && [ "$_running_ver" != "$EXPECTED" ] && \
       [ "$(printf '%s\n%s\n' "$_running_ver" "$EXPECTED" | sort -V | head -n1)" = "$_running_ver" ]; then
    "$BIN" restart >/dev/null 2>&1 || true
  fi
fi

# ── Inject core memory into the session's system prompt ─────────────────────
#
# Only when jq is available and the daemon answered our query. Empty store
# (fresh install) emits nothing — host gets a normal SessionStart with no
# additionalContext.

command -v jq >/dev/null 2>&1 || exit 0
[ -x "$BIN" ] || exit 0

core_rows="$("$BIN" list --tier core --limit 100 --format json --quiet 2>/dev/null || true)"
[ -z "$core_rows" ] && exit 0

# Defensive guard: if `ling-mem list --quiet` somehow leaked non-JSON
# to stdout (daemon bug, mixed warning, partial response), the jq
# pipeline below would fail silently and the session would start with
# no core context and no log of why. Validate parse first; on failure,
# log to stderr (CC shows hook stderr in the transcript) and bail
# cleanly without emitting hookSpecificOutput.
if ! printf '%s' "$core_rows" | jq -es '.' >/dev/null 2>&1; then
  printf 'shared-memory autostart: ling-mem list --tier core returned non-JSON; skipping core injection\n' >&2
  exit 0
fi

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
