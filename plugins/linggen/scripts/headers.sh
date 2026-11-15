#!/usr/bin/env bash
# headersHelper for both of the plugin's MCP servers (`.mcp.json`): this host's
# token for another machine's Linggen, from ~/.linggen/client.json, read at
# each connect. A token that changes (a new approval on the other machine)
# applies on reconnect, with no restart. No file, no token, no jq: `{}` — the
# static `headers` fallback stands. Loopback ignores the header.
f="${LINGGEN_DATA_DIR:-$HOME/.linggen}/client.json"
tok=""
if [ -r "$f" ] && command -v jq >/dev/null 2>&1; then
  tok="$(jq -r '.token // empty' "$f" 2>/dev/null)"
fi
if [ -n "$tok" ]; then
  jq -nc --arg t "$tok" '{"x-linggen-device": $t}'
else
  printf '{}\n'
fi
