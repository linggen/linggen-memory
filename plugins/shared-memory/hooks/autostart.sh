#!/usr/bin/env bash
#
# SessionStart hook for shared-memory.
#
# 1. Bootstrap the ling-mem binary on first run (or after a plugin update
#    that bumped VERSION) into the ONE canonical, cross-host location
#    ~/.local/bin/ling-mem — a real file shared by every host/channel, not
#    a per-plugin copy. install-bin resolves the (range) pin, won't
#    downgrade a newer shared binary, and replaces any legacy symlink.
# 2. Start the daemon idempotently (restart only if it's older than the
#    on-disk binary — never downgrade a daemon another host started).
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

# If neither host injects a plugin-root env var (e.g. Codex's hook runner
# doesn't expand ${PLUGIN_ROOT} when launching shell hooks), we have no
# stable place to read VERSION or scripts/install-bin.sh from. Bail
# silently — the daemon may still be reachable on whatever the user has
# already installed; nothing here can do useful work blind.
[ -z "$PLUGIN_ROOT" ] && exit 0

# ONE canonical, cross-host binary path. Every channel (CC/Codex plugin,
# OpenClaw, skills.sh, Linggen engine) installs/upgrades this same real file —
# never a per-plugin copy or symlink — so there's one binary, one daemon, one
# store. ~/.local/bin (not /usr/local) because plugins/skills install without
# sudo. install-bin.sh defaults to this dir too.
DEST="$HOME/.local/bin"
BIN="$DEST/ling-mem"
VERSION="$(cat "$PLUGIN_ROOT/VERSION" 2>/dev/null || echo "v0.7.3")"
mkdir -p "$DEST" 2>/dev/null || true

# Install/upgrade the shared binary. install-bin resolves a range pin, verifies
# SHA-256, won't downgrade a newer shared binary, and replaces a legacy symlink
# with a real file. Idempotent and cheap when already satisfied.
bash "$PLUGIN_ROOT/scripts/install-bin.sh" \
  --version "$VERSION" --dest "$DEST" --quiet >/dev/null 2>&1 || true

[ -x "$BIN" ] || exit 0
HAVE="$("$BIN" --version 2>/dev/null | awk '{print $2}')"

# Reconcile the RUNNING daemon with the on-disk binary: start if down; restart
# only if the daemon is OLDER than the binary on disk (so a stale-pinned host
# never downgrades a daemon another host already started — we only move
# forward). $BIN is used by absolute path to dodge stale PATH caching.
_st="$("$BIN" status --format json 2>/dev/null)"
_state="$(printf '%s' "$_st" | jq -r '.state // empty' 2>/dev/null)"
_running="$(printf '%s' "$_st" | jq -r '.version // empty' 2>/dev/null)"
if [ "$_state" != "running" ]; then
  "$BIN" start >/dev/null 2>&1 || true
elif [ -n "$HAVE" ] && [ -n "$_running" ] && [ "$_running" != "$HAVE" ] && \
     [ "$(printf '%s\n%s\n' "$_running" "$HAVE" | sort -V | head -n1)" = "$_running" ]; then
  "$BIN" restart >/dev/null 2>&1 || true
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
