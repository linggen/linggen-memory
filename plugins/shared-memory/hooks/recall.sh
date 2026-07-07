#!/usr/bin/env bash
# UserPromptSubmit hook installed by shared-memory. Surfaces relevant
# memories for each turn. Bails silently on any failure — never blocks
# the user.

set -u
[ "${LING_MEM_RECALL_DISABLE:-0}" = "1" ] && exit 0

command -v jq       >/dev/null 2>&1 || exit 0
command -v ling-mem >/dev/null 2>&1 || exit 0

input="$(cat)"
prompt="$(printf '%s' "$input" | jq -r '.prompt // empty' 2>/dev/null || true)"
cwd="$(printf '%s' "$input"   | jq -r '.cwd    // empty' 2>/dev/null || true)"
sid="$(printf '%s' "$input"   | jq -r '.session_id // empty' 2>/dev/null || true)"

[ "${#prompt}" -lt 8 ] && exit 0

topk="${LING_MEM_RECALL_TOPK:-3}"
limit="${LING_MEM_RECALL_LIMIT:-8}"
to="${LING_MEM_RECALL_TIMEOUT:-3}"
# No hardcoded floor: omit --min-score so the daemon applies its store-wide
# `recall_min_score` (one selectivity shared by all hosts). Set
# LING_MEM_RECALL_MIN_SCORE to override per-host.
min_score="${LING_MEM_RECALL_MIN_SCORE:-}"

proj=""
if [ -n "$cwd" ] && [ "$cwd" != "$HOME" ]; then
  proj="$(basename "$cwd")"
fi

TIMEOUT_BIN=""
if   command -v timeout  >/dev/null 2>&1; then TIMEOUT_BIN="timeout"
elif command -v gtimeout >/dev/null 2>&1; then TIMEOUT_BIN="gtimeout"
fi

# Portable timeout. Stock macOS ships neither `timeout` nor `gtimeout`
# (those are GNU coreutils, opt-in via Homebrew). Without a fallback, a
# hung daemon — slow LanceDB scan, OOM, lock contention — would block
# the user's prompt indefinitely. The background+kill pattern works on
# any POSIX shell.
run_with_timeout() {
  local seconds="$1"; shift
  if [ -n "$TIMEOUT_BIN" ]; then
    "$TIMEOUT_BIN" "$seconds" "$@" 2>/dev/null
    return
  fi
  "$@" 2>/dev/null &
  local pid=$!
  (
    sleep "$seconds"
    kill -TERM "$pid" 2>/dev/null
    sleep 1
    kill -KILL "$pid" 2>/dev/null
  ) &
  local watcher=$!
  wait "$pid" 2>/dev/null
  kill -KILL "$watcher" 2>/dev/null
  wait "$watcher" 2>/dev/null
}

score_arg=""
[ -n "$min_score" ] && score_arg="--min-score $min_score"
# shellcheck disable=SC2086
out="$(run_with_timeout "$to" ling-mem search "$prompt" \
    --limit "$limit" $score_arg \
    --format json --quiet || true)"

[ -z "$out" ] && exit 0

hits="$(printf '%s' "$out" | jq -sr --arg proj "$proj" --argjson k "$topk" '
  map(select(
    ((.contexts // []) | map(select(startswith("project/"))))
    | (length == 0 or any(. == ("project/" + $proj)))
  ))
  | .[:$k]
  | .[]
  | "From memory (\(.type), \(.host // "unknown"), \((.created_at // "")[0:10]), score=\((.score // 0) * 100 | floor / 100), id=\(.id)): \(.content)"
' 2>/dev/null || true)"

hit_count="$(printf '%s\n' "$hits" | grep -c .)"
[ -n "$hits" ] && printf '%s\n' "$hits"

# Always-on capture nudge — fires EVERY turn, including zero-hit turns
# (often the very turns that produce new memory). Tier definitions / routing
# live in the session-start MCP instructions; this is only the per-turn reminder.
cat <<'CAPTURE'

Memory capture: before finishing this turn, recognize anything worth remembering and write it at the right tier per the memory protocol (core/semantic = search-first; episodic = incidental); anchor relative time to absolute dates ("last month" → "2026-06"). Nothing worth keeping? Skip silently.
CAPTURE
# Session stamp: pass source_session on every add so a later scan of this
# day's logs skips sessions that already contributed (idempotent backfill).
if [ -n "$sid" ]; then
  printf 'On every memory_add, pass source_session:"%s" (this session).\n' "$sid"
fi

if [ "$hit_count" -gt 1 ]; then
  # Mirrors linggen/src/engine/prompt/core_block.rs:RECONCILE_FOOTER.
  # Adapted to the ling-mem MCP verbs (memory_add / memory_delete;
  # replace_ids is in the MCP memory_add schema — one atomic call).
  cat <<'NOTE'

Note: If duplicates or conflicting rows appear above AND the user's current turn is unrelated to memory itself (incidental recall hit), resolve them on the side — merge authority follows voice: memory_delete for exact dups; rows that are all your own notes (from=derived — built/fixed/tried/learned) merge freely into one current-truth row via memory_add with replace_ids listing the losers (atomic insert + delete), no AskUser; if any row is in the user's voice (from=user — preference/decision/identity), AskUser first, then the same memory_add with replace_ids (never separate add + delete). If the user IS explicitly steering memory ("clean up", "remember X", "what's in memory", "ignore the hits"), follow their instruction and do NOT side-quest into dedup. Either way, keep memory in good shape.
NOTE
fi
