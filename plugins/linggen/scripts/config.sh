#!/usr/bin/env bash
# Point this host at a Linggen — `/linggen:config`.
#
# The CLIENT declaration: where THIS machine goes to find Linggen — one address
# (host + port; memory is at /mcp/memory there, the engine's tools at /mcp) and,
# for another machine, that machine's token. On the machine that runs Linggen
# it is loopback and nobody ever needs this. A machine with only the plugin
# points it at another machine's Linggen, and nothing is installed or started
# locally: the hooks talk MCP over HTTP.
#
# It is deliberately NOT the engine's `[server].url`. That says where the engine
# on a machine BINDS. This says where a client CONNECTS. On one machine they
# coincide, which is what makes them easy to confuse; on two they are different
# facts about different hosts.
#
#   config.sh                               # show what is configured, and probe it
#   config.sh --linggen 192.168.1.5         # use that machine's Linggen (port 9527)
#   config.sh --linggen 192.168.1.5:9527 --token <token>   # with a token in hand
#   config.sh --local                       # back to this machine
#   config.sh --primary 192.168.1.5         # this machine's memory follows that one
#   config.sh --primary local               # this machine's memory is the primary
#   config.sh --mirror                      # re-mirror client.json into the hosts (hooks)
#
# Writes ~/.linggen/client.json (merged: keys it does not own are kept), which
# the hooks read. Also mirrors the address into each host's own MCP wiring,
# because a plugin's MCP declaration cannot read a file: Claude Code's
# settings.json `env` (its `.mcp.json` takes `${VAR}`, expanded at startup), and
# Codex's config.toml `[mcp_servers.*]` tables (Codex expands nothing in a URL,
# so its plugin declaration is literal loopback and an off-machine address must
# override it there). Then says plainly whether that took.

set -u

DATA_DIR="${LINGGEN_DATA_DIR:-$HOME/.linggen}"
CLIENT_FILE="$DATA_DIR/client.json"
CC_SETTINGS="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/settings.json"
CODEX_CONFIG="${CODEX_HOME:-$HOME/.codex}/config.toml"

DEFAULT_LINGGEN="http://127.0.0.1:9527"

die() { printf '%s\n' "$*" >&2; exit 1; }
command -v jq   >/dev/null 2>&1 || die "config: jq is required"
command -v curl >/dev/null 2>&1 || die "config: curl is required"

# The address as the hooks resolve it (client.json > env > default, an old
# memory address read where unambiguous) — one reader, shared with them.
# shellcheck source=../hooks/mcp.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/hooks/mcp.sh" || die "config: hooks/mcp.sh missing"

# ── Arguments ───────────────────────────────────────────────────────────────

addr="" token="" reset=0 show_only=1 mirror_only=0 primary=""

while [ $# -gt 0 ]; do
    case "$1" in
        --linggen)  addr="${2:-}";    shift 2; show_only=0 ;;
        --token)    token="${2:-}";   shift 2; show_only=0 ;;
        --local)    reset=1;          shift;   show_only=0 ;;
        --primary)  primary="${2:-}"; shift 2 ;;
        --mirror)   mirror_only=1;    shift;   show_only=0 ;;
        -h|--help)  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)          die "config: unknown argument $1" ;;
    esac
done

# `host` and `host:port` are accepted and normalised — that is how a person
# says an address, and refusing it would be pedantry. No port: 9527.
normalize() {
    local u="$1" hp
    [ -z "$u" ] && return 0
    case "$u" in http://*|https://*) ;; *) u="http://$u" ;; esac
    u="${u%/}"
    hp="${u#*://}"
    case "$hp" in *:*) ;; *) u="$u:9527" ;; esac
    printf '%s' "$u"
}

addr="$(normalize "$addr")"

# ── Read, merge, write ──────────────────────────────────────────────────────

cur_addr="$LINGGEN_URL"
cur_token="$LINGGEN_TOKEN"
cur_device_id="$(_ling_client_get device_id)"
old_mem="$(_ling_client_get ling_mem)"

if [ "$reset" = 1 ]; then
    new_addr="$DEFAULT_LINGGEN"; new_token=""
else
    new_addr="${addr:-$cur_addr}"
    if [ -n "$token" ]; then new_token="$token"
    elif [ -n "$addr" ] && [ "$(_ling_host_of "$addr")" != "$(_ling_host_of "$cur_addr")" ]; then new_token=""
    else new_token="$cur_token"; fi
fi
_ling_is_local "$(_ling_host_of "$new_addr")" && new_local=1 || new_local=0
# A token is for another machine; loopback never needs one.
[ "$new_local" = 1 ] && new_token=""

# ── Ask to connect ──────────────────────────────────────────────────────────
#
# Pointed at another machine with no token: ask it. That machine shows the
# request in Linggen; a click on Allow there hands this host a token, which is
# saved below like a typed --token. Nobody sees or copies it.

mcp_code() { # $1 base, $2 token — HTTP status of a memory tools/list
    local auth=()
    [ -n "${2:-}" ] && auth=(-H "x-linggen-device: $2")
    curl -s -o /dev/null -w '%{http_code}' --max-time 5 --connect-timeout 2 \
        -H 'Content-Type: application/json' ${auth[@]+"${auth[@]}"} \
        -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' "$1/mcp/memory" 2>/dev/null
}

new_uuid() {
    { uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid 2>/dev/null \
        || od -An -N16 -tx1 /dev/urandom | tr -d ' \n'; } | tr 'A-Z' 'a-z'
}

device_id="${cur_device_id:-$(new_uuid)}"
device_name="$(hostname -s 2>/dev/null || hostname)"

ask_status() { # $1 base, $2 request id — prints the status JSON
    curl -s --max-time 5 --connect-timeout 2 "$1/mem/api/connect/status/$2" 2>/dev/null
}

ask_to_connect() { # $1 base — prints the token, or dies with why not
    local base="$1" body reply code rid st
    body="$(jq -nc --arg n "$device_name" --arg d "$device_id" '{device_name: $n, device_id: $d}')"
    reply="$(curl -s -w '\n%{http_code}' --max-time 5 --connect-timeout 2 \
        -H 'Content-Type: application/json' -d "$body" "$base/mem/api/connect/request" 2>/dev/null)"
    code="${reply##*$'\n'}"
    case "$code" in
        200) ;;
        403) die "config: that machine denied $device_name. Un-deny it in Linggen there, then retry." ;;
        429) die "config: too many requests — wait a minute and retry." ;;
        *)   die "config: could not ask $base to connect (HTTP $code)." ;;
    esac
    rid="$(printf '%s' "${reply%$'\n'*}" | jq -r '.data.request_id // empty')"
    [ -n "$rid" ] || die "config: $base did not open a request."
    printf 'Approve %s in Linggen on %s…\n' "$device_name" "$(_ling_host_of "$base")" >&2
    for _ in $(seq 1 60); do
        st="$(ask_status "$base" "$rid")"
        case "$(printf '%s' "$st" | jq -r '.data.state // empty' 2>/dev/null)" in
            allowed) printf '%s' "$st" | jq -r '.data.device_token // empty'; return 0 ;;
            denied)  die "config: denied on that machine." ;;
            expired) die "config: the request expired — run this again." ;;
        esac
        sleep 2
    done
    die "config: no answer within two minutes — run this again."
}

if [ "$show_only" = 0 ] && [ "$mirror_only" = 0 ] && [ "$new_local" = 0 ] && [ -z "$token" ]; then
    case "$(mcp_code "$new_addr" "$new_token")" in
        401|403)
            new_token="$(ask_to_connect "$new_addr")" || exit 1
            [ -n "$new_token" ] || die "config: approved, but no token came back — run this again."
            printf 'Approved.\n' >&2 ;;
    esac
fi

# ── Mirror into Claude Code's env ───────────────────────────────────────────
#
# Derived, never authored here: client.json above is the one place a human
# edits. This exists only because `.mcp.json` cannot read a file. The retired
# memory daemon's keys (LING_MEM_*) are removed in the same write.

mirror_cc_env() {
    mkdir -p "$(dirname "$CC_SETTINGS")" 2>/dev/null || return 1
    [ -f "$CC_SETTINGS" ] || printf '{}\n' > "$CC_SETTINGS"
    local tmp="$CC_SETTINGS.tmp.$$"
    jq --arg h "$(_ling_host_of "$new_addr")" --arg p "$(_ling_port_of "$new_addr")" --arg tk "$new_token" '
        .env = ((.env // {}) + {LINGGEN_HOST: $h, LINGGEN_PORT: $p}
                | del(.LING_MEM_HOST, .LING_MEM_PORT, .LING_MEM_TOKEN))   # the old memory keys
        | if $tk == "" then del(.env.LINGGEN_TOKEN) else .env.LINGGEN_TOKEN = $tk end
    ' "$CC_SETTINGS" > "$tmp" 2>/dev/null && mv "$tmp" "$CC_SETTINGS"
}

# ── Mirror into Codex's config.toml ─────────────────────────────────────────
#
# Codex reads the plugin's own `.codex-plugin/mcp.json`, which is literal
# loopback — Codex expands no `${VAR}` in a URL. A `[mcp_servers.<name>]` table
# in config.toml overrides a plugin server of the same name, so that is where an
# address other than the default goes. On the default the tables are removed and
# the plugin's declaration stands, so a default install leaves no trace here.

mirror_codex_toml() { # exit 2 = no Codex on this machine
    [ -d "$(dirname "$CODEX_CONFIG")" ] || return 2
    [ -f "$CODEX_CONFIG" ] || : > "$CODEX_CONFIG"
    local tmp="$CODEX_CONFIG.tmp.$$"
    # Drop the tables this script writes (sub-tables included) wherever a
    # TOML-aware writer left them — a table runs from its header to the next
    # header or EOF. `linggen` and the retired `ling-mem` are always ours;
    # `memory` is a common name, so only a table whose url ends in /mcp/memory
    # goes. No BEGIN/END markers: Codex rewrites this file itself and moves
    # comments. The blank line we put before our tables goes with them when
    # they sat at EOF, so a round trip leaves the file byte-equal.
    awk '
        function flush_table() {
            if (nt == 0) return
            if (!(kind == "ours" || (kind == "memory" && ours_url))) {
                for (i = 1; i <= nt; i++) emit(tbl[i])
            } else {
                # Blank lines after a dropped table separate what follows.
                for (j = nt; j > 1 && tbl[j] ~ /^[[:space:]]*$/; j--) ;
                for (i = j + 1; i <= nt; i++) emit(tbl[i])
            }
            nt = 0; kind = ""; ours_url = 0
        }
        function emit(line) { if (held) print heldline; heldline = line; held = 1 }
        /^[[:space:]]*\[/ {
            flush_table()
            if ($0 ~ /^[[:space:]]*\[mcp_servers\.(ling-mem|linggen)(\.[^]]*)?\]/) { kind = "ours" }
            else if ($0 ~ /^[[:space:]]*\[mcp_servers\.memory(\.[^]]*)?\]/) { kind = "memory" }
            if (kind != "") {
                # A blank line just before our table belongs to it.
                if (held && heldline ~ /^[[:space:]]*$/) { held = 0; tbl[++nt] = heldline }
                tbl[++nt] = $0; next
            }
        }
        nt > 0 {
            tbl[++nt] = $0
            if ($0 ~ /^[[:space:]]*url[[:space:]]*=.*\/mcp\/memory"[[:space:]]*$/) ours_url = 1
            next
        }
        { emit($0) }
        END { flush_table(); if (held) print heldline }
    ' "$CODEX_CONFIG" > "$tmp" || { rm -f "$tmp"; return 1; }
    # Any address but the default needs the override — another machine, or
    # this machine's Linggen on a port of its own.
    if [ "$new_addr" != "$DEFAULT_LINGGEN" ]; then
        {
            printf '\n[mcp_servers.linggen]\nurl = "%s/mcp"\n' "$new_addr"
            [ -n "$new_token" ] && printf 'http_headers = { "x-linggen-device" = "%s" }\n' "$new_token"
            printf '\n[mcp_servers.memory]\nurl = "%s/mcp/memory"\n' "$new_addr"
            [ -n "$new_token" ] && printf 'http_headers = { "x-linggen-device" = "%s" }\n' "$new_token"
        } >> "$tmp"
    fi
    mv "$tmp" "$CODEX_CONFIG"
}

# ── State, drift, guidance ─────────────────────────────────────────────────

state_of() { # $1 base, $2 token — ok | not_approved | no_answer | http_NNN
    local code
    code="$(mcp_code "$1" "${2:-}")"
    case "$code" in
        200) printf ok ;;
        401|403) printf not_approved ;;
        000|"") printf no_answer ;;
        *) printf 'http_%s' "$code" ;;
    esac
}

label() { # $1 state, $2 base — the line's verdict
    case "$1" in
        ok) local ver
            ver="$(curl -fsS --max-time 4 --connect-timeout 2 "$2/api/health" 2>/dev/null \
                | jq -r '.version // .data.version // empty' 2>/dev/null)"
            printf 'ok%s' "$([ -n "$ver" ] && printf ' · %s' "$ver")" ;;
        not_approved) printf 'not approved' ;;
        no_answer) printf 'no answer' ;;
        *) printf 'HTTP %s' "${1#http_}" ;;
    esac
}

# What Claude Code's `.mcp.json` should see vs what its settings env holds.
cc_env_want() {
    printf '%s %s %s' "$(_ling_host_of "$new_addr")" "$(_ling_port_of "$new_addr")" "$new_token"
}
cc_env_have() {
    [ -r "$CC_SETTINGS" ] || { printf '127.0.0.1 9527 '; return; }
    jq -r '.env // {} | "\(.LINGGEN_HOST // "127.0.0.1") \(.LINGGEN_PORT // "9527") \(.LINGGEN_TOKEN // "")"
        + (if any(keys[]; startswith("LING_MEM_")) then " old" else "" end)' \
        "$CC_SETTINGS" 2>/dev/null   # " old": the old memory keys are still there; a mirror removes them
}
cc_env_drifted() { # no Claude Code here → nothing to drift
    [ -d "$(dirname "$CC_SETTINGS")" ] || return 1
    [ "$(cc_env_want)" != "$(cc_env_have)" ]
}

next_step() { # $1 state
    if cc_env_drifted; then
        echo "Next: run /linggen:config --linggen ${new_addr#*://}, then restart Claude Code."
    elif [ "$1" = not_approved ]; then
        echo "Next: run /linggen:config --linggen ${new_addr#*://} and click Allow in Linggen on $(_ling_host_of "$new_addr")."
    elif [ "$1" = no_answer ] && [ "$new_local" = 1 ]; then
        echo "Next: Linggen is not running here. Start it: ling --web (or open Linggen.app)."
    elif [ "$1" = no_answer ]; then
        echo "Next: nothing answers at $(_ling_host_of "$new_addr"). Check Linggen is on there and shares on the LAN."
    elif [ "$new_local" = 1 ]; then
        echo "All set. Change settings at ${new_addr}/settings"
    else
        echo "All set: memory comes from $(_ling_host_of "$new_addr")."
    fi
    return 0
}

old_mem_note() { # the retired memory daemon's address, if client.json still names it
    [ -n "$old_mem" ] || return 0
    printf 'note      client.json also names an old memory address (%s); this host uses %s. The next /linggen:config --linggen … or --local drops it.\n' \
        "$old_mem" "$new_addr"
}

guide() {
    local st role
    st="$(state_of "$new_addr" "$new_token")"
    printf 'linggen   %s  %s\n' "$new_addr" "$(label "$st" "$new_addr")"
    if [ "$new_local" = 1 ]; then
        role="$(local_role)"
        [ -n "$role" ] && printf 'memory    %s\n' "$(role_line "$role")"
    else
        printf 'memory    on %s (this machine has only the plugin)\n' "$(_ling_host_of "$new_addr")"
    fi
    old_mem_note
    printf '\n'
    if [ -n "${role:-}" ] && [ -n "$(role_next "$role")" ]; then
        role_next "$role"
    else
        next_step "$st"
    fi
    printf '\nCommon:\n'
    printf '  /linggen:config --linggen <ip>    use the Linggen on another machine (this one runs none)\n'
    printf '  /linggen:config --primary <ip>    this machine keeps a copy of that machine'\''s memory\n'
    printf '  /linggen:config --local           Linggen on this machine\n'
    printf '  /linggen:config                   check again\n'
}

# ── Primary or replica (the recommended way to share memory) ────────────────
#
# Every machine runs its own Linggen and every client talks to it on loopback.
# Its memory is the primary, or a replica of another machine's primary — set
# here, kept by Linggen itself (`/mem/api/sync/*`). Becoming a replica asks the
# primary to connect; someone clicks Allow there.

LOCAL_MEM="${LINGGEN_URL}/mem"

local_role() { curl -fsS --max-time 3 "$LOCAL_MEM/api/sync/role" 2>/dev/null | jq -c '.data // empty' 2>/dev/null; }

role_line() { # $1 role JSON — one line of facts
    printf '%s' "$1" | jq -r '
        if .role == "primary" then "primary (this machine)"
        else "replica of \(.upstream) · "
            + (if .approved | not then "not approved yet"
               elif .online then "online" else "offline" end)
            + (if .last_sync then " · last sync \(.last_sync[0:16])Z" else "" end)
            + " · \(.queued) queued"
        end'
}

role_next() { # $1 role JSON — the next step, or nothing
    printf '%s' "$1" | jq -r '
        if .role == "primary" then empty
        elif .waiting_for_approval then "Next: click Allow in Linggen on \(.upstream | split(":")[0])."
        elif .error and (.approved | not) then "Next: \(.error). Run /linggen:config --primary \(.upstream) to ask again."
        elif .online | not then "The primary is offline. Writes queue here and replay when it is back."
        elif .error then "Next: \(.error)."
        else empty end'
}

set_primary() { # $1 host[:port] | local
    local reply role p="$1"
    [ "$LINGGEN_LOCAL" = 1 ] \
        || die "config: this host uses the Linggen on $LINGGEN_HOST and runs none of its own. Run /linggen:config --local first (that installs Linggen here)."
    case "$p" in local|*:*) ;; *) p="$p:9527" ;; esac
    reply="$(curl -sS --max-time 10 -X PUT -H 'Content-Type: application/json' \
        -d "$(jq -nc --arg p "$p" '{primary: $p}')" "$LOCAL_MEM/api/sync/primary" 2>/dev/null)" \
        || die "config: Linggen is not running here. Start it (ling --web, or open Linggen.app), then retry."
    printf '%s' "$reply" | jq -e '.ok' >/dev/null 2>&1 \
        || die "config: $(printf '%s' "$reply" | jq -r '.error // "refused"')"
    role="$(printf '%s' "$reply" | jq -c '.data')"
    if printf '%s' "$role" | jq -e '.waiting_for_approval' >/dev/null; then
        printf 'Approve %s in Linggen on %s…\n' "$device_name" "${p%%:*}"
        for _ in $(seq 1 65); do
            sleep 2
            role="$(local_role)"
            printf '%s' "$role" | jq -e '.waiting_for_approval | not' >/dev/null 2>&1 && break
        done
    fi
    printf 'memory    %s\n' "$(role_line "$role")"
    role_next "$role"
}

if [ -n "$primary" ]; then
    set_primary "$primary"
    exit 0
fi

if [ "$show_only" = 1 ]; then
    guide
    exit 0
fi

write_client() {
    mkdir -p "$DATA_DIR" 2>/dev/null || die "config: cannot create $DATA_DIR"
    local tmp="$CLIENT_FILE.tmp.$$" cur='{}'
    [ -r "$CLIENT_FILE" ] && cur="$(jq -c 'if type == "object" then . else {} end' "$CLIENT_FILE" 2>/dev/null || printf '{}')"
    printf '%s' "$cur" | jq --arg l "$new_addr" --arg t "$new_token" --arg d "$device_id" \
        '. + {ling: $l, token: $t, device_id: $d} | del(.ling_mem)' > "$tmp" || die "config: could not write $tmp"
    chmod 600 "$tmp" 2>/dev/null || true
    mv "$tmp" "$CLIENT_FILE" || die "config: could not replace $CLIENT_FILE"
}
# `--mirror` re-mirrors what client.json already says; it never rewrites it.
[ "$mirror_only" = 1 ] || write_client

# `--mirror` (the SessionStart hook): exit 0 when Claude Code's env changed
# and needs a restart, 10 when it already matched.
if [ "$mirror_only" = 1 ]; then
    mirror_codex_toml >/dev/null 2>&1 || true
    cc_env_drifted || exit 10
    mirror_cc_env || exit 1
    exit 0
fi

mirrored=1
mirror_cc_env || mirrored=0
codex_mirrored=1
mirror_codex_toml || codex_mirrored=$?

# ── Report ──────────────────────────────────────────────────────────────────

printf 'wrote %s\n\n' "$CLIENT_FILE"
printf 'linggen   %s  %s\n' "$new_addr" "$(label "$(state_of "$new_addr" "$new_token")" "$new_addr")"
printf '\n'

if [ "$mirrored" = 1 ]; then
    printf 'Claude Code: mirrored into %s (env). Restart it — MCP URLs are resolved at startup.\n' "$CC_SETTINGS"
else
    printf 'Claude Code: could not write %s — the hooks will follow client.json, but the MCP\n' "$CC_SETTINGS"
    printf 'servers will not. Export these in your shell profile instead:\n'
    printf '  export LINGGEN_HOST=%s LINGGEN_PORT=%s\n' "$(_ling_host_of "$new_addr")" "$(_ling_port_of "$new_addr")"
fi
case "$codex_mirrored" in
    1) if [ "$new_addr" != "$DEFAULT_LINGGEN" ]; then
           printf 'Codex: mirrored into %s ([mcp_servers.linggen], [mcp_servers.memory]). Restart it.\n' "$CODEX_CONFIG"
       else
           printf 'Codex: loopback — the plugin declaration stands; nothing written to %s.\n' "$CODEX_CONFIG"
       fi ;;
    2) ;;
    *) printf 'Codex: could not write %s — add [mcp_servers.memory] url = "%s/mcp/memory" yourself.\n' "$CODEX_CONFIG" "$new_addr" ;;
esac
