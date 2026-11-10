#!/usr/bin/env bash
# PreToolUse hook installed by the linggen plugin. Stamps where the session
# stands onto every memory call — the facts a host knows and a model would
# only copy by hand (ling-mem doc/scope-index-spec.md):
#
#   memory_add           cwd (the row's default scope), root (what a model's
#                        `scope` resolves against), source_session, host
#   memory_search        scope_root = root (recall: rows under the root, at
#                        its parents, and about the person)
#   memory_session_start cwd, root
#
# The daemon owns the rules — which dirs can hold rows, which a root sees —
# so this hook only hands it paths. `root` is the SESSION's root (scope.sh
# session_root), not the shell cwd's: the candidates were shown against it. The model picks a row's `scope` from the
# candidates the SessionStart hook showed; the daemon checks it against
# `root` and falls back to `cwd`.
#
# WHY A HOOK AND NOT THE MODEL. These are facts about the session, not
# judgments about the content. The store's own history is the argument — when
# writes moved from the CLI (which had `--cwd`) to MCP (which did not), `cwd`
# fell to 7 rows out of 1142, and `source_session`, which used to be asked of
# the model every turn, sat at 44%. Anything mechanical belongs here.
#
# BOTH HOSTS. Claude Code registers it in hooks.json; Codex in codex.hooks.json
# with LING_MEM_HOST=codex, which only changes the `host` stamp. Verified on
# codex-cli 0.167.0 (2026-11-10): Codex fires PreToolUse for MCP tools and
# applies `updatedInput` together with permissionDecision "allow" — the
# rewritten arguments reach the tool. (The old note here, citing
# openai/codex#18491, was stale.) The tool name there is normalised, e.g.
# `mcp__ling_mem__memory_search`, which the suffix match below covers. Note
# "allow" does not skip Codex's own MCP approval gate: a call that needed
# approval still needs it.
#
# Bails silently on anything unexpected — a memory write must never fail
# because attribution could not be worked out.

set -u
[ "${LING_MEM_STAMP_CWD_DISABLE:-0}" = "1" ] && exit 0
command -v jq >/dev/null 2>&1 || exit 0

# shellcheck source=./scope.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/scope.sh" 2>/dev/null || exit 0

input="$(cat)"

tool="$(printf '%s' "$input" | jq -r '.tool_name // empty' 2>/dev/null || true)"
cwd="$(printf '%s' "$input"  | jq -r '.cwd // empty'       2>/dev/null || true)"
sid="$(printf '%s' "$input"  | jq -r '.session_id // empty' 2>/dev/null || true)"

# Suffix match, because the tool arrives namespaced by server
# (`mcp__plugin_linggen_ling-mem__memory_add`) and that prefix is the user's
# to choose.
case "$tool" in
  *memory_add)           verb="add" ;;
  *memory_search)        verb="search" ;;
  *memory_session_start) verb="session_start" ;;
  *) exit 0 ;;
esac

root=""
if [ -n "$cwd" ]; then root="$(session_root "$cwd")"; fi

# A field the caller set wins — the one legitimate case is a promote pass
# carrying the ORIGINAL row's session, scope (sent as `cwd`) and host forward.
has() { [ -n "$(printf '%s' "$input" | jq -r --arg f "$1" '.tool_input[$f] // empty' 2>/dev/null)" ]; }

stamp="{}"
add_field() {
  has "$1" && return 0
  [ -n "$2" ] || return 0
  stamp="$(printf '%s' "$stamp" | jq -c --arg f "$1" --arg v "$2" '. + {($f): $v}')"
}

case "$verb" in
  add)
    # A write that names ANOTHER session's row is not this session's
    # authorship: the dream's promote and the scan's backfill carry the
    # original row's source_session — and its scope as `cwd`, when it had one. This
    # session's paths stamped over the gap would rescope someone else's
    # memory to wherever the dream happened to run.
    src="$(printf '%s' "$input" | jq -r '.tool_input.source_session // empty' 2>/dev/null || true)"
    if [ -z "$src" ] || [ "$src" = "$sid" ]; then
      add_field source_session "$sid"
      add_field cwd "$cwd"
      add_field root "$root"
    fi
    add_field host "${LING_MEM_HOST:-claude-code}"
    ;;
  search)
    # A search the model makes itself is scoped only inside a project. At
    # $HOME / ~/.linggen it is a deliberate whole-store lookup (the dream runs
    # there); the per-turn recall (recall.sh) is the one that narrows those
    # sessions to rows about the person.
    if is_project_dir "$root"; then add_field scope_root "$root"; fi
    ;;
  session_start)
    add_field cwd "$cwd"
    add_field root "$root"
    ;;
esac

[ "$stamp" = "{}" ] && exit 0

updated="$(printf '%s' "$input" \
  | jq -c --argjson s "$stamp" '.tool_input + $s' 2>/dev/null || true)"
[ -n "$updated" ] || exit 0

jq -nc --argjson i "$updated" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    permissionDecision: "allow",
    updatedInput: $i
  }
}'
