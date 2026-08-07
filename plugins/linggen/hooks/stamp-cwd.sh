#!/usr/bin/env bash
# PreToolUse hook installed by the linggen plugin. Stamps the working
# directory onto every memory write, and scopes every memory search to it.
#
# WHY A HOOK AND NOT THE MODEL. `cwd` is a fact about the session, not a
# judgment about the content: the host knows it exactly, the model would be
# copying it by hand, and a field copied by hand is a field that is empty most
# of the time. The store's own history is the argument — when writes moved from
# the CLI (which had `--cwd`) to MCP (which did not), `cwd` fell to 7 rows out
# of 1142, and `source_session`, which IS asked of the model every single turn
# by recall.sh, sits at 44%. Anything mechanical belongs here.
#
# WHAT IT BUYS. Recall stops crossing projects. Without a scope, a question
# about "perception" asked in one repo answers out of another repo that happens
# to use the same word. `cwd_scope` is applied in the daemon BEFORE ranking, so
# the unrelated project cannot crowd the wanted rows out of the top N — a
# filter applied after the fact can only shrink an already-wrong list.
#
# Rows with no `cwd` stay visible from everywhere by design: identity,
# preferences and cross-project gotchas are about the person, not the project.
#
# Bails silently on anything unexpected — a memory write must never fail
# because attribution could not be worked out.

set -u
[ "${LING_MEM_STAMP_CWD_DISABLE:-0}" = "1" ] && exit 0
command -v jq >/dev/null 2>&1 || exit 0

input="$(cat)"

tool="$(printf '%s' "$input" | jq -r '.tool_name // empty' 2>/dev/null || true)"
cwd="$(printf '%s' "$input"  | jq -r '.cwd // empty'       2>/dev/null || true)"
[ -n "$cwd" ] || exit 0

# Which field this tool wants. A write records where it came from; a read asks
# what is in scope. Same value, opposite direction — see the matching pair in
# linggen/src/engine/tools/memory_mcp.rs, which does this for Linggen's own
# agents. Suffix match, because the tool arrives namespaced by server
# (`mcp__plugin_linggen_ling-mem__memory_add`) and that prefix is the user's to
# choose.
case "$tool" in
  *memory_add)    field="cwd" ;;
  *memory_search) field="cwd_scope" ;;
  *) exit 0 ;;
esac

# Never overwrite a value the caller set deliberately. The one legitimate case
# is a promote pass carrying the ORIGINAL row's origin forward — the dream
# knows where a memory came from and this hook does not.
existing="$(printf '%s' "$input" | jq -r --arg f "$field" '.tool_input[$f] // empty' 2>/dev/null || true)"
[ -n "$existing" ] && exit 0

updated="$(printf '%s' "$input" \
  | jq -c --arg f "$field" --arg v "$cwd" '.tool_input + {($f): $v}' 2>/dev/null || true)"
[ -n "$updated" ] || exit 0

jq -nc --argjson i "$updated" '{
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    permissionDecision: "allow",
    updatedInput: $i
  }
}'
