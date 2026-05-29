#!/usr/bin/env bash
#
# Privacy + intent (for human readers and static scanners):
# Walks LOCAL session-file directories, extracts a flattened, secret-
# filtered transcript per session via extract_session.sh, and writes
# the result as NDJSON to ~/.linggen/memory/.scan-output.jsonl. No
# network calls. No file content leaves the machine. Output is a
# candidates file the agent reads during the `hippocampus` action to
# decide what's memory-worthy.
#
# scan.sh — script-only memory scan (Phase 1+2 of the dream flow).
#
# Does NOT involve the LLM. Just shells out to:
#   1. collect_sessions.sh    — find session files for the chosen window
#   2. extract_session.sh     — per-session flatten + denoise + secret-filter
#
# Output file (overwritten on every run):
#   ~/.linggen/memory/.scan-output.jsonl
#
#   Line 1 — header object:
#     {"_meta": true,
#      "started_at": "<ISO>", "finished_at": "<ISO>",
#      "window": "24h|7d|30d|14d|YYYY-MM-DD",
#      "scanned_from": "<ISO date>", "scanned_to": "<ISO date>",
#      "sessions_found": N, "sessions_scanned": N, "skipped_empty": N,
#      "transcript_bytes": N, "duration_ms": N}
#
#   Lines 2..N — one per non-empty session:
#     {"filepath": "...", "source": "CC|Codex|OpenClaw|Linggen",
#      "date": "YYYY-MM-DD", "user_turns": N, "bytes": N,
#      "transcript": "[user]: ...\n[assistant]: ..."}
#
# Empty-session filter: skip when `user_turns < 2 AND bytes < 2000`
# (matches dream-flow.md). Counted under `skipped_empty`.
#
# Usage:  ./scan.sh [WINDOW]
#         WINDOW is one of:
#           today | 24h            -> last 1 day   (default)
#           week                   -> last 7 days
#           month                  -> last 30 days
#           <n>d | <n>w | <n>m | <n>y   e.g. 14d, 3w, 2m, 1y
#           YYYY-MM-DD             -> scan exactly that one day
#         (default: 24h)
#
# Requires: jq

set -uo pipefail

# Parse a flexible window into DAYS_BACK (calendar days to walk back)
# and a normalized WINDOW label. Aliases map to durations; <n><unit>
# accepts d(ay) / w(eek) / m(onth=30d) / y(ear=365d).
WINDOW="${1:-24h}"
SCAN_DATE=""   # set when WINDOW is a specific YYYY-MM-DD (dream one day)
case "$WINDOW" in
  today|24h)  DAYS_BACK=1  ; WINDOW=24h ;;
  week)       DAYS_BACK=7  ; WINDOW=7d  ;;
  month)      DAYS_BACK=30 ; WINDOW=30d ;;
  *)
    if [[ "$WINDOW" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
      SCAN_DATE="$WINDOW"; DAYS_BACK=1   # scan exactly this calendar day
    elif [[ "$WINDOW" =~ ^([0-9]+)([dwmy])$ ]]; then
      n="${BASH_REMATCH[1]}"
      case "${BASH_REMATCH[2]}" in
        d) DAYS_BACK=$n ;;
        w) DAYS_BACK=$(( n * 7 )) ;;
        m) DAYS_BACK=$(( n * 30 )) ;;
        y) DAYS_BACK=$(( n * 365 )) ;;
      esac
    else
      echo "Usage: $0 [today|24h|week|month|<n>d|<n>w|<n>m|<n>y|YYYY-MM-DD]  (e.g. 14d, 2m, 2026-05-20)" >&2
      exit 1
    fi
    ;;
esac
if (( DAYS_BACK < 1 )); then
  echo "window must resolve to >= 1 day (got '$WINDOW')" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COLLECT="$SCRIPT_DIR/collect_sessions.sh"
EXTRACT="$SCRIPT_DIR/extract_session.sh"

# Fall back to the installed skill location if either tool isn't
# alongside this script (e.g. dev-mode invocation from a checkout).
for var in COLLECT EXTRACT; do
  path="${!var}"
  if [[ ! -f "$path" ]]; then
    base="$(basename "$path")"
    eval "$var=\"\$HOME/.linggen/skills/shared-memory/scripts/$base\""
  fi
  if [[ ! -f "${!var}" ]]; then
    echo "$(basename "${!var}") not found" >&2
    exit 1
  fi
done

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required" >&2
  exit 1
fi

OUT_DIR="$HOME/.linggen/memory"
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/.scan-output.jsonl"
TMP_MANIFEST="$(mktemp)"
TMP_ERR="$(mktemp)"
trap 'rm -f "$TMP_MANIFEST" "$TMP_ERR"' EXIT

STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
START_EPOCH="$(date +%s)"

# Track the actual calendar-day range walked (lexicographic min/max on
# ISO dates). The dream records this so the heatmap greens every day a
# run covered, not just the run day.
SCANNED_FROM=""
SCANNED_TO=""
record_date() {
  [[ -z "$SCANNED_TO"   || "$1" > "$SCANNED_TO"   ]] && SCANNED_TO="$1"
  [[ -z "$SCANNED_FROM" || "$1" < "$SCANNED_FROM" ]] && SCANNED_FROM="$1"
}

if [[ -n "$SCAN_DATE" ]]; then
  # Single specific day — scan only that date.
  record_date "$SCAN_DATE"
  bash "$COLLECT" "$SCAN_DATE" >> "$TMP_MANIFEST" 2>> "$TMP_ERR" || true
else
  # Walk DAYS_BACK calendar days back from today. The collect script
  # accepts a single date; de-duping happens in jq below — sessions
  # that span midnight only appear once.
  for ((i = 0; i < DAYS_BACK; i++)); do
    if [[ "$(uname)" == "Darwin" ]]; then
      DATE_ISO="$(date -v-${i}d +%Y-%m-%d 2>/dev/null || echo)"
    else
      DATE_ISO="$(date -d "$i days ago" +%Y-%m-%d 2>/dev/null || echo)"
    fi
    [[ -z "$DATE_ISO" ]] && continue
    record_date "$DATE_ISO"
    bash "$COLLECT" "$DATE_ISO" >> "$TMP_MANIFEST" 2>> "$TMP_ERR" || true
  done
fi

# Dedup manifest by filepath.
TMP_MANIFEST_DEDUP="$(mktemp)"
jq -s 'unique_by(.filepath) | .[]' -c "$TMP_MANIFEST" > "$TMP_MANIFEST_DEDUP" 2>/dev/null \
  || cp "$TMP_MANIFEST" "$TMP_MANIFEST_DEDUP"
mv "$TMP_MANIFEST_DEDUP" "$TMP_MANIFEST"

SESSIONS_FOUND="$(wc -l < "$TMP_MANIFEST" | tr -d ' ')"
SESSIONS_SCANNED=0
SKIPPED_EMPTY=0
# Total bytes of EXTRACTED transcripts (i.e. what the hippocampus
# LLM will read), not raw session-file bytes. The raw bytes are
# typically 50-200x larger because they include tool calls / system
# reminders / JSON envelopes that extract_session.sh strips.
TRANSCRIPT_BYTES=0

# Build the body (lines 2..N) into a temp file, then write the header
# + body atomically. Lets us compute counters before writing the header.
TMP_BODY="$(mktemp)"
trap 'rm -f "$TMP_MANIFEST" "$TMP_ERR" "$TMP_BODY"' EXIT

while IFS= read -r row; do
  [[ -z "$row" ]] && continue
  filepath="$(jq -r '.filepath' <<<"$row")"
  source="$(jq -r '.source'   <<<"$row")"
  date="$(jq -r '.date'       <<<"$row")"
  user_turns="$(jq -r '.user_turns // 0' <<<"$row")"
  bytes="$(jq -r '.bytes // 0' <<<"$row")"

  # Empty-session filter — same gate as dream-flow Phase 1.
  if [[ "$user_turns" -lt 2 && "$bytes" -lt 2000 ]]; then
    SKIPPED_EMPTY=$((SKIPPED_EMPTY + 1))
    continue
  fi
  if [[ ! -f "$filepath" ]]; then
    SKIPPED_EMPTY=$((SKIPPED_EMPTY + 1))
    continue
  fi

  transcript="$(bash "$EXTRACT" "$filepath" "$source" "$date" 2>>"$TMP_ERR" || true)"
  if [[ -z "$transcript" ]]; then
    SKIPPED_EMPTY=$((SKIPPED_EMPTY + 1))
    continue
  fi

  transcript_bytes="${#transcript}"
  jq -n -c \
    --arg filepath "$filepath" \
    --arg source "$source" \
    --arg date "$date" \
    --argjson user_turns "$user_turns" \
    --argjson bytes "$bytes" \
    --argjson transcript_bytes "$transcript_bytes" \
    --arg transcript "$transcript" \
    '{filepath: $filepath, source: $source, date: $date, user_turns: $user_turns, bytes: $bytes, transcript_bytes: $transcript_bytes, transcript: $transcript}' \
    >> "$TMP_BODY"

  SESSIONS_SCANNED=$((SESSIONS_SCANNED + 1))
  TRANSCRIPT_BYTES=$((TRANSCRIPT_BYTES + transcript_bytes))
done < "$TMP_MANIFEST"

FINISHED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
END_EPOCH="$(date +%s)"
DURATION_MS=$(( (END_EPOCH - START_EPOCH) * 1000 ))

# Atomic write: header line first, then the body.
jq -n -c \
  --arg started "$STARTED_AT" \
  --arg finished "$FINISHED_AT" \
  --arg window "$WINDOW" \
  --arg scanned_from "$SCANNED_FROM" \
  --arg scanned_to "$SCANNED_TO" \
  --argjson found "$SESSIONS_FOUND" \
  --argjson scanned "$SESSIONS_SCANNED" \
  --argjson skipped "$SKIPPED_EMPTY" \
  --argjson transcript_bytes "$TRANSCRIPT_BYTES" \
  --argjson duration_ms "$DURATION_MS" \
  '{_meta: true, started_at: $started, finished_at: $finished, window: $window,
    scanned_from: $scanned_from, scanned_to: $scanned_to,
    sessions_found: $found, sessions_scanned: $scanned, skipped_empty: $skipped,
    transcript_bytes: $transcript_bytes, duration_ms: $duration_ms}' \
  > "$OUT"
cat "$TMP_BODY" >> "$OUT"

# One-line summary on stdout — the skill's web app reads this directly
# and pipes it to the agent so the hippocampus prompt can quote real
# numbers without re-parsing the JSONL header.
echo "scan: window=$WINDOW found=$SESSIONS_FOUND scanned=$SESSIONS_SCANNED skipped=$SKIPPED_EMPTY transcript_bytes=$TRANSCRIPT_BYTES ms=$DURATION_MS out=$OUT"
