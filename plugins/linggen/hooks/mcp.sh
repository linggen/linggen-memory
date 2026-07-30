#!/usr/bin/env bash
# Shared by the plugin's hooks: how to reach ling-mem, and how to call one of
# its tools. Sourced, never executed.
#
# One copy on purpose. The address used to be written independently in each
# place that needed it, and they drifted — the 2026-07 port migration flipped
# `.mcp.json` and left a hook behind, so every session started a second daemon
# on a port nothing served. These are the SAME env vars `.mcp.json` expands,
# with the SAME defaults, written here and there and nowhere else.

LING_MEM_HOST="${LING_MEM_HOST:-127.0.0.1}"
LING_MEM_PORT="${LING_MEM_PORT:-9528}"
LING_MEM_URL="http://${LING_MEM_HOST}:${LING_MEM_PORT}/mcp"

# Is the store on THIS machine? Installing a binary or starting a daemon only
# makes sense if it is: pointed at another host, a local daemon on the same
# port would answer from a DIFFERENT store and silently fork the user's memory
# in two — the one outcome this whole arrangement exists to prevent.
case "$LING_MEM_HOST" in
  127.0.0.1|localhost|::1|"") LING_MEM_LOCAL=1 ;;
  *) LING_MEM_LOCAL=0 ;;
esac

# Off-machine, ling-mem's LAN gate wants a paired device's token; loopback
# needs none, so a normal single-machine install sets nothing.
_ling_mem_auth=()
[ -n "${LING_MEM_TOKEN:-}" ] && _ling_mem_auth=(-H "x-linggen-device: ${LING_MEM_TOKEN}")

# One MCP tool call. $1 = tool name, $2 = arguments as JSON, $3 = timeout secs
# (default 3). Prints the tool's payload — MCP double-encodes it, as a JSON
# string inside a text content block, so this unwraps one layer and the caller
# parses the rest.
#
# Empty output on ANY failure: unreachable daemon, gate refusal, malformed
# reply. Every caller is a hook that must never block a session, so a failure
# here is silence, not an error.
#
# `${_ling_mem_auth[@]+…}` because bash 3.2 (stock macOS) treats an empty array
# as unbound under `set -u`.
mcp_call() {
  local name="$1" args="$2" to="${3:-3}" body
  command -v curl >/dev/null 2>&1 || return 0
  command -v jq   >/dev/null 2>&1 || return 0
  body="$(jq -nc --arg n "$name" --argjson a "$args" \
    '{jsonrpc:"2.0", id:1, method:"tools/call", params:{name:$n, arguments:$a}}' \
    2>/dev/null)" || return 0
  curl -fsS --max-time "$to" --connect-timeout 1 \
    -H 'Content-Type: application/json' \
    ${_ling_mem_auth[@]+"${_ling_mem_auth[@]}"} \
    -d "$body" "$LING_MEM_URL" 2>/dev/null \
    | jq -r '.result.content[0].text // empty' 2>/dev/null || true
}
