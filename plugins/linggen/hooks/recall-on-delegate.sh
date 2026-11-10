#!/usr/bin/env bash
# PreToolUse recall for delegated briefs: mid-turn agents otherwise miss memory
# and stale claims in their briefs go uncorrected. Never block the host.
# LING_MEM_DELEGATE=0 disables; LING_MEM_DELEGATE_REWRITE=0 supplies context
# only; LING_MEM_DELEGATE_DECISION=allow opts in to permission approval.
#
# One script, two hosts, chosen by LING_MEM_HOST (the hook entry sets it;
# default claude-code). The host table below is the only difference:
#   claude-code  rewrites the brief, keeps the normal permission flow
#   codex        rewrites the brief; Codex honours `updatedInput` only together
#                with permissionDecision "allow" (verified on 0.167.0), so the
#                delegation is auto-approved there.
# Codex's built-in `spawn_agent` is NOT covered, on purpose (verified on
# 0.167.0): its `message` is ciphertext (`"encrypted": true` in the tool schema)
# that only Codex can read — a hook can neither search on it nor rewrite it
# (a plaintext replacement makes the child fail with invalid_encrypted_content),
# and the child thread fires no SessionStart/UserPromptSubmit of its own. Only
# plaintext briefs (`agent_run` over MCP) are served; the guard below also lets
# ciphertext (no whitespace) fall through silently.
set -u
[ "${LING_MEM_DELEGATE:-1}" = 0 ] && exit 0
command -v jq >/dev/null 2>&1 || exit 0
input="$(cat)"
# Host table: does a rewrite need an explicit permissionDecision "allow"?
case "${LING_MEM_HOST:-claude-code}" in
  codex) host_decision=allow ;;
  *) host_decision=none ;;
esac
heading='From memory (ling-mem) — recalled for this task; newer rows win over what the brief says:'
# Validate the shape and inspect the original string in jq (including newlines).
query="$(printf '%s' "$input" | jq -er --arg h "$heading" '
  .tool_input.prompt | select(type == "string" and length >= 40 and test("\\s") and (contains($h) | not)) | .[:2000]
' 2>/dev/null)" || exit 0
cwd="$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)" || exit 0
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" || exit 0
. "$here/scope.sh" 2>/dev/null || exit 0
scope=""
[ -n "$cwd" ] && scope="$(session_root "$cwd")"
args="$(jq -nc --arg q "$query" --argjson l "${LING_MEM_DELEGATE_LIMIT:-3}" --arg c "$scope" '
  {query:$q, limit:$l} + (if ($c | length) > 0 then {scope_root:$c} else {} end)
' 2>/dev/null)" || exit 0
if [ -n "${LING_MEM_SEARCH_CMD:-}" ]; then
  out="$("$LING_MEM_SEARCH_CMD" memory_search "$args" "${LING_MEM_RECALL_TIMEOUT:-3}" 2>/dev/null)" || exit 0
else
  command -v curl >/dev/null 2>&1 || exit 0
  . "$here/mcp.sh" 2>/dev/null || exit 0
  out="$(mcp_call memory_search "$args" "${LING_MEM_RECALL_TIMEOUT:-3}")" || exit 0
fi
[ -n "$out" ] || exit 0
lines="$(printf '%s' "$out" | jq -r --argjson input "$input" '
  if type != "array" then empty else .[] |
  select((.id | type) == "string" and (.id | length) > 0) |
  select(.id as $id | ($input.tool_input.prompt | contains($id) | not)) |
  "- (\(.type), \((.created_at // "")[0:10]), id=\(.id)) \((.content // "")[0:400] | gsub("[\r\n]+"; " "))"
  end
' 2>/dev/null)" || exit 0
[ -n "$lines" ] || exit 0
block="$(printf '%s\n%s' "$heading" "$lines")"
printf '%s' "$input" | jq -c --arg b "$block" --arg rewrite "${LING_MEM_DELEGATE_REWRITE:-1}" --arg decision "${LING_MEM_DELEGATE_DECISION:-}" --arg hostdec "$host_decision" '
  ($rewrite != "0") as $rw |
  (if $decision != "" then $decision elif $rw then $hostdec else "none" end) as $dec |
  {hookSpecificOutput: ({hookEventName:"PreToolUse", additionalContext:("Recalled for the task you just delegated: " + $b)}
    + (if $rw then {updatedInput:(.tool_input | .prompt += "\n\n" + $b)} else {} end)
    + (if $dec == "allow" then {permissionDecision:"allow"} else {} end))}
' 2>/dev/null || exit 0
exit 0
