#!/usr/bin/env bash
# Where this host goes to find Linggen, and how it calls a memory tool there.
# Sourced by the plugin's hooks, never executed.
#
# **This is the CLIENT declaration.** It says where to *connect*, which on a
# machine with only the plugin is another machine entirely — and it is
# deliberately NOT the engine's `[server].url`. That file says where the engine
# on THAT machine binds; reading it to find an engine on another host is how
# the two facts get confused.
#
# One address: where Linggen lives (host + port), and, for another machine,
# that machine's token. Everything hangs off it:
#
#   $LINGGEN_URL/mcp          the engine's tools (browser, x, agents, dream)
#   $LINGGEN_URL/mcp/memory   Linggen's memory (the 17 memory_* tools)
#
# Authored in ONE file, `~/.linggen/client.json`, written by
# `scripts/config.sh` (`/linggen:config`):
#
#   { "ling": "http://…:9527", "token": "…", "device_id": "…" }
#
# Precedence is file > env > default (127.0.0.1:9527): client.json is the one
# authority; env (LINGGEN_HOST, LINGGEN_PORT, LINGGEN_TOKEN) is the fallback for
# a host with no file. `.mcp.json` can only be given environment variables —
# Claude Code expands it at startup, before any hook runs — so `config.sh`
# mirrors the file into Claude Code's settings env, and the SessionStart hook
# re-mirrors it when the two drift. Codex expands nothing in a URL: its
# declaration is the literal loopback `.codex-plugin/mcp.json`, and `config.sh`
# overrides it from config.toml when this host points elsewhere.
#
# An older file or environment may still name the retired memory daemon's
# address, the old memory keys (`ling_mem` in the file, LING_MEM_HOST/PORT/TOKEN in env). It is read
# as this one address when that is unambiguous — see _ling_pick — and
# `config.sh` writes the file without it the next time it runs.

# ── Where ───────────────────────────────────────────────────────────────────

_ling_client_file="${LINGGEN_DATA_DIR:-$HOME/.linggen}/client.json"

# One key out of the client config, or empty. Absent file, bad JSON and missing
# key are all "nothing configured" — this runs in a hook that must never fail.
_ling_client_get() {
    [ -r "$_ling_client_file" ] || return 0
    command -v jq >/dev/null 2>&1 || return 0
    jq -r --arg k "$1" '.[$k] // empty' "$_ling_client_file" 2>/dev/null || true
}

# `http://host:port` → host / port, tolerating a missing scheme or path.
_ling_host_of() { local u="${1#*://}"; u="${u%%/*}"; printf '%s' "${u%:*}"; }
_ling_port_of() { local u="${1#*://}"; u="${u%%/*}"; case "$u" in *:*) printf '%s' "${u##*:}" ;; esac; }

_ling_is_local() {
    case "$1" in
        127.0.0.1|localhost|::1|"") return 0 ;;
        *) return 1 ;;
    esac
}

# _ling_pick <ling-url> <old-memory-url> — the one address. The old memory
# address meant "the machine whose memory this host uses"; that machine's
# Linggen now serves memory on its engine port (9527).
#   only one given            → that one (the old one on port 9527)
#   both on the same host     → the engine address
#   engine local, memory not  → the memory machine: this host used its memory,
#                               and starting a local Linggen would open a
#                               second, empty store
#   two different remotes     → the engine address (ambiguous; config.sh shows it)
_ling_pick() {
    local ling="$1" old="$2" oh
    [ -n "$old" ] || { printf '%s' "$ling"; return; }
    oh="$(_ling_host_of "$old")"
    old="http://${oh:-127.0.0.1}:9527"
    [ -n "$ling" ] || { printf '%s' "$old"; return; }
    if [ "$(_ling_host_of "$ling")" = "$oh" ]; then printf '%s' "$ling"; return; fi
    if _ling_is_local "$(_ling_host_of "$ling")" && ! _ling_is_local "$oh"; then
        printf '%s' "$old"; return
    fi
    printf '%s' "$ling"
}

_ling_addr="$(_ling_pick "$(_ling_client_get ling)" "$(_ling_client_get ling_mem)")"
if [ -z "$_ling_addr" ]; then
    _ling_env=""
    if [ -n "${LINGGEN_HOST:-}${LINGGEN_PORT:-}" ]; then
        _ling_env="http://${LINGGEN_HOST:-127.0.0.1}:${LINGGEN_PORT:-9527}"
    fi
    _ling_old_env=""
    [ -n "${LING_MEM_HOST:-}" ] && _ling_old_env="http://${LING_MEM_HOST}"  # old memory address
    _ling_addr="$(_ling_pick "$_ling_env" "$_ling_old_env")"
fi

LINGGEN_HOST="$(_ling_host_of "$_ling_addr")"
LINGGEN_PORT="$(_ling_port_of "$_ling_addr")"
LINGGEN_HOST="${LINGGEN_HOST:-127.0.0.1}"
LINGGEN_PORT="${LINGGEN_PORT:-9527}"
LINGGEN_URL="http://${LINGGEN_HOST}:${LINGGEN_PORT}"
LINGGEN_MEMORY_URL="${LINGGEN_URL}/mcp/memory"

# Is Linggen on THIS machine? Installing or starting `ling` — or opening the
# store with `ling mem` — only makes sense if it is: pointed at another machine,
# a local Linggen would hold a DIFFERENT store and silently fork the user's
# memory in two, the one outcome this whole arrangement exists to prevent.
_ling_is_local "$LINGGEN_HOST" && LINGGEN_LOCAL=1 || LINGGEN_LOCAL=0

# Off-machine, the engine wants an approved machine's token; loopback needs
# none, so a normal single-machine install sets nothing.
_ling_file_token="$(_ling_client_get token)"
LINGGEN_TOKEN="${_ling_file_token:-${LINGGEN_TOKEN:-${LING_MEM_TOKEN:-}}}"  # last: the old memory token
_ling_auth=()
[ -n "$LINGGEN_TOKEN" ] && _ling_auth=(-H "x-linggen-device: ${LINGGEN_TOKEN}")

# ── Calling ─────────────────────────────────────────────────────────────────

# One MCP tool call against Linggen's memory. $1 = tool name, $2 = arguments as
# JSON, $3 = timeout secs (default 3). Prints the tool's payload — MCP
# double-encodes it, as a JSON string inside a text content block, so this
# unwraps one layer and the caller parses the rest.
#
# Empty output on ANY failure: unreachable engine, gate refusal, malformed
# reply. Every caller is a hook that must never block a session, so a failure
# here is silence, not an error.
#
# `${_ling_auth[@]+…}` because bash 3.2 (stock macOS) treats an empty array
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
        ${_ling_auth[@]+"${_ling_auth[@]}"} \
        -d "$body" "$LINGGEN_MEMORY_URL" 2>/dev/null \
        | jq -r '.result.content[0].text // empty' 2>/dev/null || true
}
