#!/usr/bin/env bash
# Where this host goes to find Linggen, and how it calls a tool there.
# Sourced by the plugin's hooks, never executed.
#
# **This is the CLIENT declaration.** It says where to *connect*, which on a
# second machine is somewhere else entirely — and it is deliberately NOT the
# engine's `[server].url`. That file says where the daemon on THAT machine
# binds; reading it to find a daemon on another host is how the two facts get
# confused. A client that has no local daemon still needs an address.
#
# Authored in ONE file, `~/.linggen/client.json`, written by
# `scripts/config.sh` (`/linggen:config`):
#
#   { "ling": "http://…:9527", "ling_mem": "http://…:9528", "token": "…" }
#
# Precedence is env > file > default. Env first because `.mcp.json` can only
# ever be given an environment variable — Claude Code expands it at startup,
# before any hook runs — so an env override has to mean the same thing here as
# it does there.

# ── Where ───────────────────────────────────────────────────────────────────

_ling_client_file="${LINGGEN_DATA_DIR:-$HOME/.linggen}/client.json"

# One key out of the client config, or empty. Absent file, bad JSON and missing
# key are all "nothing configured" — this runs in a hook that must never fail.
_ling_client_get() {
    [ -r "$_ling_client_file" ] || return 0
    command -v jq >/dev/null 2>&1 || return 0
    jq -r --arg k "$1" '.[$k] // empty' "$_ling_client_file" 2>/dev/null || true
}

# Split `http://host:port` into host and port, tolerating a missing scheme.
_ling_host_of() { local u="${1#*://}"; printf '%s' "${u%%:*}"; }
_ling_port_of() { local u="${1#*://}"; u="${u##*:}"; printf '%s' "${u%%/*}"; }

_ling_resolve() { # $1 host-env $2 port-env $3 client-key $4 default-host $5 default-port
    local h="$1" p="$2" from_file
    from_file="$(_ling_client_get "$3")"
    if [ -z "$h" ] && [ -n "$from_file" ]; then h="$(_ling_host_of "$from_file")"; fi
    if [ -z "$p" ] && [ -n "$from_file" ]; then p="$(_ling_port_of "$from_file")"; fi
    printf '%s %s' "${h:-$4}" "${p:-$5}"
}

# shellcheck disable=SC2046
read -r LINGGEN_HOST LINGGEN_PORT <<<"$(_ling_resolve "${LINGGEN_HOST:-}" "${LINGGEN_PORT:-}" ling 127.0.0.1 9527)"
read -r LING_MEM_HOST LING_MEM_PORT <<<"$(_ling_resolve "${LING_MEM_HOST:-}" "${LING_MEM_PORT:-}" ling_mem 127.0.0.1 9528)"

LINGGEN_URL="http://${LINGGEN_HOST}:${LINGGEN_PORT}"
LING_MEM_URL="http://${LING_MEM_HOST}:${LING_MEM_PORT}/mcp"

# Is each daemon on THIS machine? Installing a binary or starting a daemon only
# makes sense if it is: pointed at another host, a local daemon on the same port
# would answer from a DIFFERENT store and silently fork the user's memory in two
# — the one outcome this whole arrangement exists to prevent.
_ling_is_local() {
    case "$1" in
        127.0.0.1|localhost|::1|"") return 0 ;;
        *) return 1 ;;
    esac
}
_ling_is_local "$LINGGEN_HOST"  && LINGGEN_LOCAL=1  || LINGGEN_LOCAL=0
_ling_is_local "$LING_MEM_HOST" && LING_MEM_LOCAL=1 || LING_MEM_LOCAL=0

# Off-machine, ling-mem's LAN gate wants a paired device's token; loopback needs
# none, so a normal single-machine install sets nothing.
LING_MEM_TOKEN="${LING_MEM_TOKEN:-$(_ling_client_get token)}"
_ling_mem_auth=()
[ -n "$LING_MEM_TOKEN" ] && _ling_mem_auth=(-H "x-linggen-device: ${LING_MEM_TOKEN}")

# ── Calling ─────────────────────────────────────────────────────────────────

# One MCP tool call against ling-mem. $1 = tool name, $2 = arguments as JSON,
# $3 = timeout secs (default 3). Prints the tool's payload — MCP double-encodes
# it, as a JSON string inside a text content block, so this unwraps one layer
# and the caller parses the rest.
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
