#!/usr/bin/env bash
#
# Privacy + intent (for human readers and static scanners):
# Reads ONE session JSONL file from the LOCAL filesystem and prints a
# flattened, secret-filtered transcript to stdout. No network calls.
# No data leaves the machine. The agent reads stdout in-process to
# extract durable user preferences, which it writes back to the LOCAL
# ling-mem database.
#
# extract_session.sh — Print a flattened [role]: text transcript for one session.
#
# Hosts covered (per design doc §4):
#   - CC       — Claude Code JSONL
#   - Codex    — Codex rollout JSONL
#   - OpenClaw — OpenClaw logs JSONL
#   - Linggen  — Linggen messages JSONL (legacy host install)
#
# Strips tool_use / tool_result blocks, keeps user + assistant text only.
# Strips noise blocks (<system-reminder>, <command-*>, ``` fences) and
# applies a defence-in-depth secret filter (API keys, bearer tokens,
# password-like assignments). Messages capped at 2000 chars; total
# output capped by MAX_CHARS bytes.
#
# Usage:  ./extract_session.sh <filepath> <source> [YYYY-MM-DD] [MAX_CHARS]
#   source:    CC | Codex | OpenClaw | Linggen
#   date:      filter messages to this date (default: today)
#   max_chars: total-output cap in bytes. Default 200000 (~50k tokens).
#              Set to 0 for unlimited.
#
# Requires: jq

set -uo pipefail

if [ "$#" -lt 2 ]; then
  echo "Usage: $0 <filepath> <source> [YYYY-MM-DD] [MAX_CHARS]" >&2
  echo "  source: CC | Codex | OpenClaw | Linggen" >&2
  exit 1
fi

FILEPATH="$1"
SOURCE="$2"
TARGET_DATE="${3:-$(date +%Y-%m-%d)}"
MAX_CHARS="${4:-200000}"

if [ ! -f "$FILEPATH" ]; then
  echo "Error: file not found: $FILEPATH" >&2
  exit 1
fi

if ! command -v jq &>/dev/null; then
  echo "Error: jq is required but not installed." >&2
  exit 1
fi

if [[ "$(uname)" == "Darwin" ]]; then
  DAY_START=$(date -j -f "%Y-%m-%d %H:%M:%S" "${TARGET_DATE} 00:00:00" "+%s" 2>/dev/null || echo "0")
else
  DAY_START=$(date -d "${TARGET_DATE} 00:00:00" "+%s" 2>/dev/null || echo "0")
fi
DAY_END=$((DAY_START + 86400))

# Strip injected noise that isn't user prose:
#   - <system-reminder>...</system-reminder>  — CC auto-memory / tool reminders
#   - <command-name>...</command-name>        — CC slash-command markers
#   - <command-message>...</command-message>  — CC slash-command args
#   - ``` fenced code blocks                  — mockups, file contents, ascii art
strip_noise() {
  perl -0777 -pe '
    s{<system-reminder>.*?</system-reminder>}{}gs;
    s{<command-(?:name|message|args)>.*?</command-(?:name|message|args)>}{}gs;
    s{```[\s\S]*?```}{}g;
    s{\n{3,}}{\n\n}g;
  '
}

# Defence-in-depth secret filter (memory-spec §3 r6).  Drops entire
# *lines* matching known credential / API-key / bearer-token patterns
# BEFORE the host LLM ever sees them.  Per-line filtering keeps the
# rest of the transcript intact while preventing accidental secret
# capture even if the user pasted one mid-conversation.
secret_filter() {
  perl -ne '
    # Skip lines with obvious credentials.
    next if /\b(?:api[_-]?key|secret|token|password|passwd|bearer|authorization)\s*[:=]\s*\S{6,}/i;
    # Skip lines with high-entropy tokens (>= 24 chars of [A-Za-z0-9_\-]).
    next if /\b[A-Za-z0-9_\-]{24,}\b/ && /\b(?:key|token|secret|auth|sk-|ghp_|github_pat_|xoxp-|xoxb-|AKIA)/i;
    # Skip "Authorization: Bearer ..." headers.
    next if /^\s*Authorization:\s*Bearer\s+\S+/i;
    # Skip Set-Cookie/Cookie lines.
    next if /^\s*(?:Set-)?Cookie:/i;
    print;
  '
}

# Cap total output by byte count. Keep the head — durable facts surface
# in the first N messages anyway.
apply_max() {
  if [ "$MAX_CHARS" -le 0 ]; then
    cat
  else
    awk -v max="$MAX_CHARS" '
      BEGIN { total = 0 }
      {
        line_len = length($0) + 1   # +1 for the newline
        if (total + line_len <= max) {
          print
          total += line_len
        } else if (total == 0) {
          # Single line larger than max — emit it truncated so we always
          # produce something useful.
          print substr($0, 1, max - 40)
          print "[TRUNCATED: transcript exceeded MAX_CHARS=" max "]"
          exit
        } else {
          print "[TRUNCATED: transcript exceeded MAX_CHARS=" max "]"
          exit
        }
      }
    '
  fi
}

# Emit a one-line [SESSION_CWD] header so the agent can pass --cwd
# through to ling-mem writes. Source-specific extraction.
emit_cwd_header_cc() {
  local session_cwd
  session_cwd=$(jq -r 'select(.cwd != null) | .cwd' "$FILEPATH" 2>/dev/null \
    | sort | uniq -c | sort -rn | awk 'NR==1 { $1=""; sub(/^ +/,""); print }')
  if [ -n "$session_cwd" ]; then
    echo "[SESSION_CWD]: $session_cwd"
    echo ""
  fi
}

emit_cwd_header_codex() {
  # Codex rollout: `cwd` lives on `type == "session_meta"` rows
  # (under .payload.cwd).
  local session_cwd
  session_cwd=$(jq -r 'select(.type == "session_meta") | .payload.cwd // empty' "$FILEPATH" 2>/dev/null \
    | head -1)
  if [ -n "$session_cwd" ]; then
    echo "[SESSION_CWD]: $session_cwd"
    echo ""
  fi
}

emit_cwd_header_openclaw() {
  # OpenClaw: `cwd` lives on the opening `type=="session"` line.
  local session_cwd
  session_cwd=$(jq -r 'select(.type == "session") | .cwd // empty' "$FILEPATH" 2>/dev/null | head -1)
  if [ -n "$session_cwd" ]; then
    echo "[SESSION_CWD]: $session_cwd"
    echo ""
  fi
}

case "$SOURCE" in
  CC)
    emit_cwd_header_cc
    jq -r --arg date "$TARGET_DATE" '
      select((.type == "user" or .type == "assistant") and ((.timestamp // "") | startswith($date)))
      | (.message.role // .type) as $role
      | (.message.content // "") as $content
      | if ($content | type) == "string" then
          $content
        elif ($content | type) == "array" then
          [.message.content[] | select(.type == "text") | .text] | join("\n")
        else
          ""
        end
      | if . == "" then empty
        else "[\($role)]: \(.[0:2000])"
        end
    ' "$FILEPATH" 2>/dev/null | strip_noise | secret_filter | apply_max
    ;;
  Codex)
    emit_cwd_header_codex
    # Codex stores user/assistant turns as `type == "response_item"` rows;
    # `payload.role` carries the speaker, `payload.content` is an array of
    # parts where `input_text` is user prose and `output_text` is the model's.
    jq -r --arg date "$TARGET_DATE" '
      select(.type == "response_item" and ((.timestamp // "") | startswith($date)))
      | .payload as $p
      | ($p.role // "") as $role
      | ($p.content // []) as $content
      | ($content
          | map(select((.type // "") | test("^(input_text|output_text)$")))
          | map(.text // "")
          | join("\n")
        ) as $text
      | if $text == "" then empty
        else "[\($role)]: \($text[0:2000])"
        end
    ' "$FILEPATH" 2>/dev/null | strip_noise | secret_filter | apply_max
    ;;
  OpenClaw)
    emit_cwd_header_openclaw
    # OpenClaw chat lives on `type=="message"` rows with .message.role
    # and .message.content (the latter is an array of {type, text} parts).
    jq -r --arg date "$TARGET_DATE" '
      select(.type == "message"
             and ((.message.role // "") | test("^(user|assistant)$"))
             and (((.timestamp // "") | tostring) | startswith($date)))
      | (.message.role) as $role
      | (.message.content // "") as $content
      | if ($content | type) == "string" then
          $content
        elif ($content | type) == "array" then
          [$content[] | (.text // "")] | join("\n")
        else
          ""
        end
      | if . == "" then empty
        else "[\($role)]: \(.[0:2000])"
        end
    ' "$FILEPATH" 2>/dev/null | strip_noise | secret_filter | apply_max
    ;;
  Linggen)
    emit_cwd_header_cc   # Linggen messages also expose .cwd; same heuristic
    jq -r --argjson start "$DAY_START" --argjson end "$DAY_END" '
      select(
        (.from_id == "user" or .from_id == "ling")
        and (.is_observation == false or .is_observation == null)
        and (.timestamp >= $start and .timestamp < $end)
      )
      | (.from_id | if . == "ling" then "assistant" else . end) as $role
      | .content // ""
      | if . == "" then empty
        else "[\($role)]: \(.[0:2000])"
        end
    ' "$FILEPATH" 2>/dev/null | strip_noise | secret_filter | apply_max
    ;;
  *)
    echo "Error: unknown source '$SOURCE' (expected CC | Codex | OpenClaw | Linggen)" >&2
    exit 1
    ;;
esac
