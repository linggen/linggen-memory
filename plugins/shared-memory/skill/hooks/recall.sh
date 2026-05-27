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

[ "${#prompt}" -lt 8 ] && exit 0

topk="${LING_MEM_RECALL_TOPK:-3}"
limit="${LING_MEM_RECALL_LIMIT:-8}"
to="${LING_MEM_RECALL_TIMEOUT:-3}"
min_score="${LING_MEM_RECALL_MIN_SCORE:-0.30}"

proj=""
if [ -n "$cwd" ] && [ "$cwd" != "$HOME" ]; then
  proj="$(basename "$cwd")"
fi

TIMEOUT_BIN=""
if   command -v timeout  >/dev/null 2>&1; then TIMEOUT_BIN="timeout"
elif command -v gtimeout >/dev/null 2>&1; then TIMEOUT_BIN="gtimeout"
fi

if [ -n "$TIMEOUT_BIN" ]; then
  out="$($TIMEOUT_BIN "$to" ling-mem search "$prompt" \
      --limit "$limit" --min-score "$min_score" \
      --format json --quiet 2>/dev/null || true)"
else
  out="$(ling-mem search "$prompt" \
      --limit "$limit" --format json --quiet 2>/dev/null || true)"
fi

[ -z "$out" ] && exit 0

hits="$(printf '%s' "$out" | jq -sr --arg proj "$proj" --argjson k "$topk" '
  map(select(
    ((.contexts // []) | map(select(startswith("project/"))))
    | (length == 0 or any(. == ("project/" + $proj)))
  ))
  | .[:$k]
  | .[]
  | "From memory (\(.type), \(.host // "unknown"), \((.created_at // "")[0:10]), id=\(.id)): \(.content)"
' 2>/dev/null || true)"

[ -z "$hits" ] && exit 0

printf '%s\n' "$hits"
hit_count="$(printf '%s\n' "$hits" | grep -c .)"
if [ "$hit_count" -gt 1 ]; then
  echo "Note: When you see duplicates, dedup via 'ling-mem delete <id>'. When you see conflicts, ask the user which is correct, then 'ling-mem edit <id>'. Please keep memory in good shape."
fi
