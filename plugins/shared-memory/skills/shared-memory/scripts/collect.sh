#!/usr/bin/env bash
#
# Privacy + intent (for human readers and static scanners):
# Reads session JSONL files from the LOCAL machine (~/.claude/projects/,
# ~/.linggen/sessions/) and writes a manifest to a LOCAL output dir.
# No network calls. No data leaves the machine. Output is consumed by
# the agent in-process to extract durable preferences, which are then
# written back to the LOCAL ling-mem database.
#
# collect.sh — dream mission entry script.
#
# Collects the last 24 hours of Claude Code + Linggen session files and
# writes an NDJSON manifest to $MISSION_OUTPUT_DIR/manifest.ndjson for the
# agent to consume.
#
# Invoked by the dream mission scheduler. Env vars provided:
#   MISSION_ID, MISSION_DIR, MISSION_CWD, MISSION_OUTPUT_DIR,
#   MISSION_LAST_RUN_AT, MISSION_RUN_ID

set -uo pipefail

: "${MISSION_OUTPUT_DIR:?MISSION_OUTPUT_DIR not set}"
: "${MISSION_DIR:?MISSION_DIR not set}"

# The collect_sessions.sh script lives next to this one in the installed
# skill directory (~/.linggen/skills/shared-memory/scripts/). Find it
# relative to this script's own location so it works whether the dream
# mission was copied standalone or lives inside the skill dir.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COLLECT="$SCRIPT_DIR/collect_sessions.sh"

if [[ ! -x "$COLLECT" && -f "$COLLECT" ]]; then
  chmod +x "$COLLECT" || true
fi
if [[ ! -f "$COLLECT" ]]; then
  # Fall back to the installed skill location.
  COLLECT="$HOME/.linggen/skills/shared-memory/scripts/collect_sessions.sh"
fi
if [[ ! -f "$COLLECT" ]]; then
  echo "collect_sessions.sh not found" >&2
  exit 1
fi

mkdir -p "$MISSION_OUTPUT_DIR"
MANIFEST="$MISSION_OUTPUT_DIR/manifest.ndjson"
ERRLOG="$MISSION_OUTPUT_DIR/collect.err"
: > "$MANIFEST"
: > "$ERRLOG"

# Scan today + yesterday to cover the full 24-hour window. Match
# collect_sessions.sh's explicit `uname` branching: a fallback chain of
# `date -v ... || date -d ...` silently substitutes $TODAY for $YESTERDAY
# when both fail (minimal container, sandboxed PATH), which masks the
# problem instead of surfacing it.
TODAY=$(date +%Y-%m-%d)
if [[ "$(uname)" == "Darwin" ]]; then
  YESTERDAY=$(date -v-1d +%Y-%m-%d)
else
  YESTERDAY=$(date -d 'yesterday' +%Y-%m-%d)
fi
# If date is so broken that even the platform-correct invocation failed,
# scan today only — better than re-scanning the same day twice.
[ -z "$YESTERDAY" ] && YESTERDAY="$TODAY"

# stderr goes to a side log so the manifest stays valid NDJSON.
bash "$COLLECT" "$TODAY"     >> "$MANIFEST" 2>> "$ERRLOG" || true
if [[ "$YESTERDAY" != "$TODAY" ]]; then
  bash "$COLLECT" "$YESTERDAY" >> "$MANIFEST" 2>> "$ERRLOG" || true
fi

# Dedup by filepath — a session that spans midnight would appear twice.
if command -v jq >/dev/null 2>&1; then
  TMP="$MISSION_OUTPUT_DIR/.manifest.tmp"
  jq -s 'unique_by(.filepath) | .[]' -c "$MANIFEST" > "$TMP" 2>/dev/null || cp "$MANIFEST" "$TMP"
  mv "$TMP" "$MANIFEST"
fi

LINES=$(wc -l < "$MANIFEST" | tr -d ' ')
echo "dream: collected $LINES session(s) into $MANIFEST"
