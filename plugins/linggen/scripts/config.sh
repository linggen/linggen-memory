#!/usr/bin/env bash
# Point this host at a Linggen — `/linggen:config`.
#
# The CLIENT declaration: where THIS machine goes to find `ling` and `ling-mem`.
# On the machine that runs them, that is loopback and nobody ever needs this.
# On a second machine, it is the first machine's LAN address plus a paired
# device token — and nothing has to be installed locally, because the hooks talk
# MCP over HTTP and the memory tools come from the daemon, not a binary.
#
# It is deliberately NOT the engine's `[server].url`. That says where the daemon
# on a machine BINDS. This says where a client CONNECTS. On one machine they
# coincide, which is what makes them easy to confuse; on two they are different
# facts about different hosts.
#
#   config.sh                              # show what is configured, and probe it
#   config.sh --ling http://192.168.1.5:9527
#   config.sh --ling-mem 192.168.1.5:9528 --token <device-token>
#   config.sh --local                      # back to this machine
#
# Writes ~/.linggen/client.json, which the hooks read. Also mirrors the two
# addresses into each host's own MCP wiring, because a plugin's MCP declaration
# cannot read a file: Claude Code's settings.json `env` (its `.mcp.json` takes
# `${VAR}`, expanded at startup), and Codex's config.toml `[mcp_servers.*]`
# tables (Codex expands nothing in a URL, so its plugin declaration is literal
# loopback and an off-machine address must override it there). Then says
# plainly whether that took.

set -u

DATA_DIR="${LINGGEN_DATA_DIR:-$HOME/.linggen}"
CLIENT_FILE="$DATA_DIR/client.json"
CC_SETTINGS="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/settings.json"
CODEX_CONFIG="${CODEX_HOME:-$HOME/.codex}/config.toml"

DEFAULT_LING="http://127.0.0.1:9527"
DEFAULT_LING_MEM="http://127.0.0.1:9528"

die() { printf '%s\n' "$*" >&2; exit 1; }
command -v jq   >/dev/null 2>&1 || die "config: jq is required"
command -v curl >/dev/null 2>&1 || die "config: curl is required"

# ── Arguments ───────────────────────────────────────────────────────────────

ling="" ling_mem="" token="" reset=0 show_only=1

while [ $# -gt 0 ]; do
    case "$1" in
        --ling)     ling="${2:-}";     shift 2; show_only=0 ;;
        --ling-mem) ling_mem="${2:-}"; shift 2; show_only=0 ;;
        --token)    token="${2:-}";    shift 2; show_only=0 ;;
        --local)    reset=1;           shift;   show_only=0 ;;
        -h|--help)  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)          die "config: unknown argument $1" ;;
    esac
done

# `host:port` is accepted and normalised — that is how a person says an address,
# and refusing it would be pedantry.
normalize() {
    local u="$1"
    [ -z "$u" ] && return 0
    case "$u" in http://*|https://*) ;; *) u="http://$u" ;; esac
    printf '%s' "${u%/}"
}

ling="$(normalize "$ling")"
ling_mem="$(normalize "$ling_mem")"

# ── Read, merge, write ──────────────────────────────────────────────────────

current() { [ -r "$CLIENT_FILE" ] && jq -r --arg k "$1" '.[$k] // empty' "$CLIENT_FILE" 2>/dev/null; }

cur_ling="$(current ling)"
cur_ling_mem="$(current ling_mem)"
cur_token="$(current token)"

if [ "$reset" = 1 ]; then
    new_ling="$DEFAULT_LING"; new_ling_mem="$DEFAULT_LING_MEM"; new_token=""
else
    new_ling="${ling:-${cur_ling:-$DEFAULT_LING}}"
    new_ling_mem="${ling_mem:-${cur_ling_mem:-$DEFAULT_LING_MEM}}"
    new_token="${token:-$cur_token}"
fi

# ── Probe before claiming anything works ────────────────────────────────────
#
# A config command that writes a file and declares success has told the user
# nothing they didn't type. What they want to know is whether the address is
# real, so ask it.

probe() { # $1 base url, $2 token — prints a one-line verdict
    local base="$1" tok="${2:-}" auth=() ver code tools
    [ -n "$tok" ] && auth=(-H "x-linggen-device: $tok")

    # Ask `/mcp` for its tool list, NOT `/api/health`. Health is deliberately
    # open so a probe works before pairing — which means it answers "ok" for a
    # daemon this host cannot actually use, and the user would find out later
    # as silently empty recall. `tools/list` goes through the same gate the
    # real calls do, so this verdict is the one that matters.
    tools="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 --connect-timeout 2 \
        -H 'Content-Type: application/json' ${auth[@]+"${auth[@]}"} \
        -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
        "$base/mcp" 2>/dev/null)"
    case "$tools" in
        200) ;;
        401|403) printf 'reachable, but this host is not paired — pair on that machine, then --token <t>'; return 1 ;;
        000) printf 'no answer'; return 1 ;;
        *)   printf 'HTTP %s from /mcp' "$tools"; return 1 ;;
    esac

    # Reachable AND allowed. The version is a nicety, from the open probe.
    ver="$(curl -fsS --max-time 4 --connect-timeout 2 ${auth[@]+"${auth[@]}"} \
        "$base/api/health" 2>/dev/null | jq -r '.version // .data.version // empty' 2>/dev/null)"
    printf 'ok%s' "$([ -n "$ver" ] && printf ' · %s' "$ver")"
}

if [ "$show_only" = 1 ]; then
    printf 'ling      %s  ' "$new_ling";     probe "$new_ling"     ""           || true; printf '\n'
    printf 'ling-mem  %s  ' "$new_ling_mem"; probe "$new_ling_mem" "$new_token" || true; printf '\n'
    printf 'token     %s\n' "$([ -n "$new_token" ] && printf 'set' || printf '(none — loopback needs none)')"
    printf 'config    %s\n' "$([ -r "$CLIENT_FILE" ] && printf '%s' "$CLIENT_FILE" || printf 'not written yet (defaults)')"
    exit 0
fi

mkdir -p "$DATA_DIR" 2>/dev/null || die "config: cannot create $DATA_DIR"
tmp="$CLIENT_FILE.tmp.$$"
jq -n --arg l "$new_ling" --arg m "$new_ling_mem" --arg t "$new_token" \
    '{ling: $l, ling_mem: $m, token: $t}' > "$tmp" || die "config: could not write $tmp"
mv "$tmp" "$CLIENT_FILE" || die "config: could not replace $CLIENT_FILE"

# ── Mirror into Claude Code's env ───────────────────────────────────────────
#
# Derived, never authored here: client.json above is the one place a human
# edits. This exists only because `.mcp.json` cannot read a file.

mirror_cc_env() {
    local host_l port_l host_m port_m
    host_l="${new_ling#*://}"; port_l="${host_l##*:}"; host_l="${host_l%%:*}"
    host_m="${new_ling_mem#*://}"; port_m="${host_m##*:}"; host_m="${host_m%%:*}"
    mkdir -p "$(dirname "$CC_SETTINGS")" 2>/dev/null || return 1
    [ -f "$CC_SETTINGS" ] || printf '{}\n' > "$CC_SETTINGS"
    local tmp="$CC_SETTINGS.tmp.$$"
    jq --arg hl "$host_l" --arg pl "$port_l" --arg hm "$host_m" --arg pm "$port_m" --arg tk "$new_token" '
        .env = ((.env // {})
            + {LINGGEN_HOST: $hl, LINGGEN_PORT: $pl, LING_MEM_HOST: $hm, LING_MEM_PORT: $pm})
            | if $tk == "" then del(.env.LING_MEM_TOKEN) else .env.LING_MEM_TOKEN = $tk end
    ' "$CC_SETTINGS" > "$tmp" 2>/dev/null && mv "$tmp" "$CC_SETTINGS"
}

mirrored=1
mirror_cc_env || mirrored=0

# ── Mirror into Codex's config.toml ─────────────────────────────────────────
#
# Codex reads the plugin's own `.codex-plugin/mcp.json`, which is literal
# loopback — Codex expands no `${VAR}` in a URL. A `[mcp_servers.<name>]` table
# in config.toml overrides a plugin server of the same name, so that is where an
# off-machine address goes. On loopback the tables are removed and the plugin's
# declaration stands, so a default install leaves no trace here.

mirror_codex_toml() { # exit 2 = no Codex on this machine
    [ -d "$(dirname "$CODEX_CONFIG")" ] || return 2
    [ -f "$CODEX_CONFIG" ] || : > "$CODEX_CONFIG"
    local tmp="$CODEX_CONFIG.tmp.$$"
    # Drop our two tables (sub-tables included) wherever a TOML-aware writer
    # left them — a table runs from its header to the next header or EOF. No
    # BEGIN/END markers: Codex rewrites this file itself and moves comments.
    # One line of lookbehind so the blank line we put before our tables goes
    # with them when they sat at EOF — a round trip leaves the file byte-equal.
    awk '
        function flush() { if (held) { print heldline; held = 0 } }
        /^[[:space:]]*\[mcp_servers\.(ling-mem|linggen)(\.[^]]*)?\]/ { skip = 1; tail = 1; next }
        /^[[:space:]]*\[/ { skip = 0 }
        skip { next }
        { flush(); heldline = $0; held = 1; tail = 0 }
        END { if (held && !(tail && heldline ~ /^[[:space:]]*$/)) print heldline }
    ' "$CODEX_CONFIG" > "$tmp" || { rm -f "$tmp"; return 1; }
    if [ "$new_ling" != "$DEFAULT_LING" ] || [ "$new_ling_mem" != "$DEFAULT_LING_MEM" ]; then
        {
            printf '\n[mcp_servers.linggen]\nurl = "%s/mcp"\n' "$new_ling"
            printf '\n[mcp_servers.ling-mem]\nurl = "%s/mcp"\n' "$new_ling_mem"
            [ -n "$new_token" ] && printf 'http_headers = { "x-linggen-device" = "%s" }\n' "$new_token"
        } >> "$tmp"
    fi
    mv "$tmp" "$CODEX_CONFIG"
}

codex_mirrored=1
mirror_codex_toml || codex_mirrored=$?

# ── Report ──────────────────────────────────────────────────────────────────

printf 'wrote %s\n\n' "$CLIENT_FILE"
printf 'ling      %s  ' "$new_ling";     probe "$new_ling"     ""           || true; printf '\n'
printf 'ling-mem  %s  ' "$new_ling_mem"; probe "$new_ling_mem" "$new_token" || true; printf '\n'
printf '\n'

if [ "$mirrored" = 1 ]; then
    printf 'Claude Code: mirrored into %s (env). Restart it — MCP URLs are resolved at startup.\n' "$CC_SETTINGS"
else
    printf 'Claude Code: could not write %s — the hooks will follow client.json, but the MCP\n' "$CC_SETTINGS"
    printf 'servers will not. Export these in your shell profile instead:\n'
    printf '  export LINGGEN_HOST=%s LINGGEN_PORT=%s\n' "${new_ling#*://}" ""
    printf '  export LING_MEM_HOST=%s\n' "${new_ling_mem#*://}"
fi
case "$codex_mirrored" in
    1) if [ "$new_ling" != "$DEFAULT_LING" ] || [ "$new_ling_mem" != "$DEFAULT_LING_MEM" ]; then
           printf 'Codex: mirrored into %s ([mcp_servers.*]). Restart it.\n' "$CODEX_CONFIG"
       else
           printf 'Codex: loopback — the plugin declaration stands; nothing written to %s.\n' "$CODEX_CONFIG"
       fi ;;
    2) ;;
    *) printf 'Codex: could not write %s — add [mcp_servers.ling-mem] url = "%s/mcp" yourself.\n' "$CODEX_CONFIG" "$new_ling_mem" ;;
esac
