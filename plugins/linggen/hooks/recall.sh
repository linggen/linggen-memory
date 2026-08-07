#!/usr/bin/env bash
# UserPromptSubmit hook installed by the linggen plugin. Surfaces relevant
# memories for each turn. Bails silently on any failure — never blocks
# the user.
#
# Talks to ling-mem over **MCP**, not the CLI. A hook can't *be* a model's
# tool call, but it can *make* one: `tools/call` is a JSON-RPC POST, and
# curl can post JSON. What that buys is the whole point — this hook now needs
# a URL and (off-machine) a token, and **no `ling-mem` binary at all**. The
# CLI resolves the daemon through `daemon.json`, which can only ever describe
# a *local* daemon, so a second machine on CLI recall would need its own
# binary, its own daemon and its own store: memory forked in two.
#
# Cost, accepted: the daemon must be up. The CLI could fall back to opening
# the store directly; a URL cannot. The SessionStart hook starts it, and a
# quiet turn is the correct failure — recall is an enhancement, never a gate.

set -u
[ "${LING_MEM_RECALL_DISABLE:-0}" = "1" ] && exit 0

command -v jq   >/dev/null 2>&1 || exit 0
command -v curl >/dev/null 2>&1 || exit 0

input="$(cat)"
prompt="$(printf '%s' "$input" | jq -r '.prompt // empty' 2>/dev/null || true)"
cwd="$(printf '%s' "$input"   | jq -r '.cwd    // empty' 2>/dev/null || true)"
sid="$(printf '%s' "$input"   | jq -r '.session_id // empty' 2>/dev/null || true)"

[ "${#prompt}" -lt 8 ] && exit 0

topk="${LING_MEM_RECALL_TOPK:-3}"
limit="${LING_MEM_RECALL_LIMIT:-8}"
to="${LING_MEM_RECALL_TIMEOUT:-3}"
# No hardcoded floor: with no `min_score` the daemon applies its store-wide
# `recall_min_score` (one selectivity shared by all hosts). Set
# LING_MEM_RECALL_MIN_SCORE to tighten it per host — applied client-side,
# because `min_score` is deliberately absent from the MCP schema (a *model*
# guessing a threshold narrows recall to zero). The rows carry their scores,
# so filtering here costs nothing and needs no schema change.
min_score="${LING_MEM_RECALL_MIN_SCORE:-0}"

# Address + `mcp_call`, shared with autostart.sh. Located from this script's
# own path rather than a plugin-root env var — Codex's hook runner doesn't set
# one, and recall has to keep working there.
# shellcheck source=./mcp.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/mcp.sh" 2>/dev/null || exit 0

# Scope the search to the work being done here — rows written under this
# path, plus every row that belongs to no project (identity, preferences,
# cross-project gotchas). Sent to the daemon rather than applied to its
# answer: the scope has to shape the ranking, because a filter applied
# afterwards can only shrink a list that was already the wrong N. This
# replaces a client-side `project/<name>` context filter that matched 17 rows
# out of 1142 and therefore passed everything through the "untagged is
# global" branch — a filter reading a namespace nothing wrote.
#
# `$HOME` is not a project: a session started there means "no particular
# work", and scoping to it would claim every repo underneath.
search_args="$(jq -nc --arg q "$prompt" --argjson l "$limit" --arg c "$cwd" --arg home "$HOME" '
  {query:$q, limit:$l}
  + (if ($c | length) > 0 and $c != $home then {cwd_scope: $c} else {} end)
')"
out="$(mcp_call memory_search "$search_args" "$to")"

[ -z "$out" ] && exit 0

hits="$(printf '%s' "$out" | jq -r --argjson k "$topk" --argjson min "$min_score" '
  (if type == "array" then . else [] end)
  | map(select((.hybrid_score // .score // 0) >= $min))
  | .[:$k]
  | .[]
  | "From memory (\(.type), \(.host // "unknown"), \((.created_at // "")[0:10]), score=\((.hybrid_score // .score // 0) * 100 | floor / 100), id=\(.id)): \(.content)"
' 2>/dev/null || true)"

hit_count="$(printf '%s\n' "$hits" | grep -c .)"
[ -n "$hits" ] && printf '%s\n' "$hits"

# Memory-upkeep nudge — undreamed days + open review items, from ONE
# `memory_days` call. Cached (default 30 min) so the recall path stays
# fast; upkeep state moves slowly. Thresholds: ≥2 undreamed days (the
# engine's own 3am cron usually covers yesterday; nagging about one day
# is noise) and ≥1 open review item.
upkeep_ttl_min="${LING_MEM_UPKEEP_CACHE_MIN:-30}"
upkeep_cache="${TMPDIR:-/tmp}/ling-mem-upkeep-$(id -u)"
upkeep=""
if [ -f "$upkeep_cache" ] && [ -n "$(find "$upkeep_cache" -mmin "-$upkeep_ttl_min" 2>/dev/null)" ]; then
  upkeep="$(cat "$upkeep_cache" 2>/dev/null || true)"
else
  days_out="$(mcp_call memory_days '{"undreamed_only":true}' "$to")"
  if [ -n "$days_out" ]; then
    upkeep="$(printf '%s' "$days_out" | jq -r '
      (.days // [] | length) as $p |
      (.open_issues // 0) as $i |
      ((.days // [])[0].date // "") as $o |
      [ (if $p >= 2 then "memory upkeep: \($p) days undreamed (oldest \($o)) — offer to run the dream: /linggen:dream runs it with this session'\''s model; memory_dream_run offloads it to the Linggen engine" else empty end),
        (if $i >= 1 then "memory upkeep: \($i) item(s) awaiting review — offer /linggen:solve (memory_issues has the facts)" else empty end) ]
      | join("\n")
    ' 2>/dev/null || true)"
    printf '%s' "$upkeep" > "$upkeep_cache" 2>/dev/null || true
  fi
fi
[ -n "$upkeep" ] && printf '\n%s\n' "$upkeep"

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

if [ "$hit_count" -ge 1 ]; then
  # Mirrors linggen/src/engine/prompt/core_block.rs:RECONCILE_FOOTER.
  # Fires on ANY hit: a single recalled row can still conflict with the
  # user's current turn, and the daemon's user-voice guard needs the
  # resolved write to carry user_directed:true.
  cat <<'NOTE'

Note: If a recalled row above duplicates or conflicts with another row or with what the user just said AND the user's current turn is unrelated to memory itself (incidental recall hit), resolve it on the side — merge authority follows voice: memory_delete for exact dups; rows that are all your own notes (from=derived — built/fixed/tried/learned) merge freely into one current-truth row via memory_add with replace_ids listing the losers (atomic insert + delete), no ask; if any row is in the user's voice (from=user — preference/decision/identity), ask first via the host's ask-user primitive (AskUserQuestion on Claude Code; plain chat elsewhere), then write the winner via memory_add with replace_ids AND user_directed:true — the daemon BLOCKS user-voice replaces without that flag, and a hedged reflection ("X feels about right to me") never justifies it (never separate add + delete). If the user IS explicitly steering memory ("clean up", "remember X", "what's in memory", "ignore the hits"), follow their instruction and do NOT side-quest into dedup. Either way, keep memory in good shape.
NOTE
fi
