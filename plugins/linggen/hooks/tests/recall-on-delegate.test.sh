#!/usr/bin/env bash
set -u
here="$(cd "$(dirname "$0")" && pwd)"
hook="$here/../recall-on-delegate.sh"
tmp="$(mktemp -d)" || exit 1
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/search" <<'FAKE'
#!/usr/bin/env bash
case "${FAKE_MODE:-hits}" in
  fail) exit 1 ;;
  empty) echo '[]' ;;
  *) printf '%s\n' '[{"type":"fact","created_at":"2026-11-10T12:00:00Z","id":"abc123","content":"A useful memory."}]' ;;
esac
FAKE
chmod +x "$tmp/search"
export LING_MEM_SEARCH_CMD="$tmp/search"
unset LING_MEM_DELEGATE LING_MEM_DELEGATE_REWRITE LING_MEM_DELEGATE_DECISION 2>/dev/null || true
prompt='Please investigate this substantial task and report back with a detailed result.'
input="$(jq -nc --arg p "$prompt" '{tool_name:"Agent",tool_input:{prompt:$p,description:"Investigate",subagent_type:"general",model:"test"},cwd:"/tmp",session_id:"s"}')"
run() { printf '%s' "$1" | bash "$hook"; }
pass() { printf 'ok %s\n' "$1"; }
fail() { printf 'FAIL %s\n' "$1" >&2; exit 1; }
out="$(run "$input")" || fail 1
printf '%s' "$out" | jq -e --arg p "$prompt" '.hookSpecificOutput | .hookEventName == "PreToolUse" and .updatedInput.description == "Investigate" and .updatedInput.subagent_type == "general" and .updatedInput.model == "test" and (.updatedInput.prompt == ($p + "\n\n" + (.additionalContext | ltrimstr("Recalled for the task you just delegated: ")))) and (has("permissionDecision") | not)' >/dev/null || fail 1
pass 1
out="$(FAKE_MODE=empty run "$input")"; [ -z "$out" ] || fail 2; pass 2
short="$(jq -nc '{tool_input:{prompt:"short"}}')"; out="$(run "$short")"; [ -z "$out" ] || fail 3; pass 3
resent="$(printf '%s' "$out" >/dev/null; printf '%s' "$input" | jq '.tool_input.prompt += " From memory (ling-mem) — recalled for this task; newer rows win over what the brief says:"')"; out="$(run "$resent")"; [ -z "$out" ] || fail 4; pass 4
out="$(LING_MEM_DELEGATE_REWRITE=0 run "$input")"; printf '%s' "$out" | jq -e '.hookSpecificOutput | (has("updatedInput") | not) and (.additionalContext | length > 0)' >/dev/null || fail 5; pass 5
out="$(LING_MEM_DELEGATE_DECISION=allow run "$input")"; printf '%s' "$out" | jq -e '.hookSpecificOutput.permissionDecision == "allow"' >/dev/null || fail 6; pass 6
agent="$(jq -nc --arg p "$prompt" '{tool_name:"mcp__plugin_linggen_linggen__agent_run",tool_input:{prompt:$p,write_dir:"/tmp/out",permission_mode:"default"}}')"; out="$(run "$agent")"; printf '%s' "$out" | jq -e '.hookSpecificOutput.updatedInput | .write_dir == "/tmp/out" and .permission_mode == "default"' >/dev/null || fail 7; pass 7
out="$(FAKE_MODE=fail run "$input")" || fail 8; [ -z "$out" ] || fail 8; pass 8
