#!/usr/bin/env bash
#
# Privacy + intent (for human readers and static scanners):
# Walks LOCAL session-file directories and prints metadata (paths, byte
# size, message counts) to stdout. Reads filenames + a few JSONL entries
# per file to count messages. No network calls. No file content leaves
# the machine. Used by the agent to decide which sessions are worth
# extracting durable preferences from.
#
# collect_sessions.sh — Scan host session stores for a date.
#
# Hosts covered (per design doc §4):
#   - Claude Code: ~/.claude/projects/<enc-cwd>/<uuid>.jsonl
#   - Codex:       ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl (+ archived_sessions/)
#   - OpenClaw:    ~/.openclaw/logs/*.jsonl
#   - Linggen:     ~/.linggen/sessions/<id>/messages.jsonl  (legacy host install)
#
# Emits NDJSON (one object per session) to stdout:
#   {"filepath":"...","source":"CC"|"Codex"|"OpenClaw"|"Linggen","label":"...",
#    "date":"...","bytes":N,"user_turns":N}
#
# user_turns counts real user messages (skips tool_result-only "user" entries in
# CC / Codex). Callers can skip empty/greeting-only sessions before judging.
#
# Per-source watermark (--watermark <file>): if provided, emit only files whose
# mtime is strictly newer than the per-source ISO-8601 timestamp recorded in
# the watermark file. The file is JSON of the form:
#   { "CC": "<ISO>", "Codex": "<ISO>", "OpenClaw": "<ISO>", "Linggen": "<ISO>" }
# Missing keys = no filter for that source. Updating the watermark is the
# caller's job (Phase 4 of the dream flow).
#
# Usage: ./collect_sessions.sh [YYYY-MM-DD] [--watermark <path>]
#
# Requires: jq

set -uo pipefail

TARGET_DATE=""
WATERMARK_FILE=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --watermark) WATERMARK_FILE="${2:-}"; shift 2 ;;
    --watermark=*) WATERMARK_FILE="${1#--watermark=}"; shift ;;
    *) if [ -z "$TARGET_DATE" ]; then TARGET_DATE="$1"; fi; shift ;;
  esac
done

TARGET_DATE="${TARGET_DATE:-$(date +%Y-%m-%d)}"

HOME_DIR="${HOME:-$(eval echo ~)}"
CC_DIR="${HOME_DIR}/.claude/projects"
CODEX_DIR="${HOME_DIR}/.codex/sessions"
CODEX_ARCHIVE="${HOME_DIR}/.codex/archived_sessions"
OPENCLAW_DIR="${HOME_DIR}/.openclaw/agents"
LING_DIR="${HOME_DIR}/.linggen/sessions"

if ! command -v jq &>/dev/null; then
  echo "Error: jq is required but not installed." >&2
  exit 1
fi

# ── watermark helpers ────────────────────────────────────────────────
watermark_for() {
  # Echo the ISO-8601 timestamp recorded for <source>, or empty.
  local src="$1"
  [ -n "$WATERMARK_FILE" ] && [ -f "$WATERMARK_FILE" ] || return 0
  jq -r --arg s "$src" '.[$s] // ""' "$WATERMARK_FILE" 2>/dev/null
}

file_mtime_epoch() {
  if [[ "$(uname)" == "Darwin" ]]; then
    stat -f "%m" "$1" 2>/dev/null || echo 0
  else
    stat -c "%Y" "$1" 2>/dev/null || echo 0
  fi
}

iso_to_epoch() {
  # ISO-8601 (UTC, with or without trailing Z) → seconds since epoch.
  local iso="$1"
  [ -n "$iso" ] || { echo 0; return; }
  if [[ "$(uname)" == "Darwin" ]]; then
    date -u -j -f "%Y-%m-%dT%H:%M:%SZ" "${iso%Z}Z" "+%s" 2>/dev/null \
      || date -u -j -f "%Y-%m-%dT%H:%M:%S" "${iso%Z}" "+%s" 2>/dev/null \
      || echo 0
  else
    date -u -d "$iso" "+%s" 2>/dev/null || echo 0
  fi
}

# Returns 0 if file mtime > watermark for source; 1 otherwise.
past_watermark() {
  local src="$1" file="$2"
  local wm; wm="$(watermark_for "$src")"
  [ -n "$wm" ] || return 0
  local wm_epoch; wm_epoch="$(iso_to_epoch "$wm")"
  local f_epoch;  f_epoch="$(file_mtime_epoch "$file")"
  [ "$f_epoch" -gt "$wm_epoch" ]
}

# ── byte / turn counting ─────────────────────────────────────────────
count_user_turns_cc() {
  # CC: type=="user" with a text content block (tool_result entries don't count)
  jq -s '[.[] | select(.type == "user") | .message.content
         | if type == "array"
           then map(select(.type == "text" and (.text // "" | length > 0))) | length
           else (if . != null and . != "" then 1 else 0 end) end]
         | add // 0' "$1" 2>/dev/null || echo 0
}

count_user_turns_codex() {
  # Codex rollout: `type == "response_item"` with payload.role=="user" and a
  # text content block; tool-result entries don't count.
  jq -s '[.[] | select(.type == "response_item"
                       and (.payload.role // "") == "user")
              | .payload.content
              | if type == "array"
                then map(select((.type // "") == "input_text"
                                 and (.text // "" | length > 0))) | length
                else 0 end]
         | add // 0' "$1" 2>/dev/null || echo 0
}

count_user_turns_openclaw() {
  # OpenClaw: lines with `type=="message"` where `.message.role == "user"`.
  # (Older script looked for top-level `.role`, which never matched the
  # `agents/<name>/sessions/<uuid>.jsonl` schema — every session returned 0.)
  jq -s '[.[] | select(.type == "message"
                       and (.message.role // "") == "user")] | length' "$1" 2>/dev/null || echo 0
}

count_user_turns_linggen() {
  jq -s '[.[] | select(.from_id == "user")] | length' "$1" 2>/dev/null || echo 0
}

file_bytes() {
  if [[ "$(uname)" == "Darwin" ]]; then
    stat -f "%z" "$1" 2>/dev/null || echo 0
  else
    stat -c "%s" "$1" 2>/dev/null || echo 0
  fi
}

emit_manifest() {
  # emit_manifest <filepath> <source> <label> <bytes> <user_turns>
  jq -cn --arg filepath "$1" \
         --arg source "$2" \
         --arg label "$3" \
         --arg date "$TARGET_DATE" \
         --argjson bytes "$4" \
         --argjson user_turns "$5" \
         '{filepath:$filepath, source:$source, label:$label, date:$date,
           bytes:$bytes, user_turns:$user_turns}'
}

file_date_str() {
  if [[ "$(uname)" == "Darwin" ]]; then
    stat -f "%Sm" -t "%Y-%m-%d" "$1" 2>/dev/null || echo ""
  else
    date -r "$1" +%Y-%m-%d 2>/dev/null || echo ""
  fi
}

# ── Claude Code ──────────────────────────────────────────────────────
if [ -d "$CC_DIR" ]; then
  for project_dir in "$CC_DIR"/*/; do
    [ -d "$project_dir" ] || continue
    for jsonl_file in "$project_dir"*.jsonl; do
      [ -f "$jsonl_file" ] || continue

      file_date="$(file_date_str "$jsonl_file")"
      [[ "$file_date" == "$TARGET_DATE" ]] || continue
      past_watermark "CC" "$jsonl_file" || continue

      project_name=$(basename "$project_dir")
      session_name=$(basename "$jsonl_file" .jsonl)
      label="${project_name}/${session_name}"
      bytes=$(file_bytes "$jsonl_file")
      user_turns=$(count_user_turns_cc "$jsonl_file")
      emit_manifest "$jsonl_file" "CC" "$label" "${bytes:-0}" "${user_turns:-0}"
    done
  done
fi

# ── Codex (rollouts + archived sessions) ─────────────────────────────
collect_codex_dir() {
  local root="$1"
  [ -d "$root" ] || return 0
  # Codex partitions by YYYY/MM/DD; walk all rollout-*.jsonl files under root.
  while IFS= read -r jsonl_file; do
    [ -f "$jsonl_file" ] || continue
    file_date="$(file_date_str "$jsonl_file")"
    [[ "$file_date" == "$TARGET_DATE" ]] || continue
    past_watermark "Codex" "$jsonl_file" || continue

    rel_path="${jsonl_file#$root/}"
    label="${rel_path}"
    bytes=$(file_bytes "$jsonl_file")
    user_turns=$(count_user_turns_codex "$jsonl_file")
    emit_manifest "$jsonl_file" "Codex" "$label" "${bytes:-0}" "${user_turns:-0}"
  done < <(find "$root" -type f -name 'rollout-*.jsonl' 2>/dev/null)
}
collect_codex_dir "$CODEX_DIR"
collect_codex_dir "$CODEX_ARCHIVE"

# ── OpenClaw ─────────────────────────────────────────────────────────
# Session JSONLs live at ~/.openclaw/agents/<agent>/sessions/<uuid>.jsonl.
# Skip the companion `<uuid>.trajectory.jsonl` files — those are structured
# trace events, not chat turns, and have a different schema.
if [ -d "$OPENCLAW_DIR" ]; then
  while IFS= read -r jsonl_file; do
    [ -f "$jsonl_file" ] || continue
    file_date="$(file_date_str "$jsonl_file")"
    [[ "$file_date" == "$TARGET_DATE" ]] || continue
    past_watermark "OpenClaw" "$jsonl_file" || continue

    label="$(basename "$jsonl_file" .jsonl)"
    bytes=$(file_bytes "$jsonl_file")
    user_turns=$(count_user_turns_openclaw "$jsonl_file")
    emit_manifest "$jsonl_file" "OpenClaw" "$label" "${bytes:-0}" "${user_turns:-0}"
  done < <(find "$OPENCLAW_DIR" -type f -name '*.jsonl' ! -name '*.trajectory.jsonl' 2>/dev/null)
fi

# ── Linggen (legacy host install) ────────────────────────────────────
if [ -d "$LING_DIR" ]; then
  for session_dir in "$LING_DIR"/*/; do
    [ -d "$session_dir" ] || continue
    jsonl_file="${session_dir}messages.jsonl"
    [ -f "$jsonl_file" ] || continue

    file_date="$(file_date_str "$jsonl_file")"
    [[ "$file_date" == "$TARGET_DATE" ]] || continue
    past_watermark "Linggen" "$jsonl_file" || continue

    session_name=$(basename "$session_dir")

    # Only ingest user sessions — skip mission/skill/unknown creators.
    [ -f "${session_dir}session.yaml" ] || continue
    creator=$(grep -m1 '^creator:' "${session_dir}session.yaml" 2>/dev/null \
      | sed -E 's/^creator:[[:space:]]*//; s/[[:space:]]*#.*$//; s/^"(.*)"$/\1/; s/^'"'"'(.*)'"'"'$/\1/')
    [ "$creator" = "user" ] || continue

    session_title=""
    if [ -f "${session_dir}session.yaml" ]; then
      session_title=$(grep '^title:' "${session_dir}session.yaml" 2>/dev/null | sed 's/^title: *//' | head -1)
    fi
    label="${session_title:-$session_name}"
    bytes=$(file_bytes "$jsonl_file")
    user_turns=$(count_user_turns_linggen "$jsonl_file")
    emit_manifest "$jsonl_file" "Linggen" "$label" "${bytes:-0}" "${user_turns:-0}"
  done
fi
