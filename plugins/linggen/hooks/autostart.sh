#!/usr/bin/env bash
#
# SessionStart hook for the linggen plugin.
#
# 1. Bootstrap the ling-mem binary on first run (or after a plugin update
#    that bumped VERSION) into the ONE canonical, cross-host location
#    ~/.local/bin/ling-mem — a real file shared by every host/channel, not
#    a per-plugin copy. install-bin resolves the (range) pin, won't
#    downgrade a newer shared binary, and replaces any legacy symlink.
# 2. Start the daemon idempotently (restart only if it's older than the
#    on-disk binary — never downgrade a daemon another host started).
# 3. Ensure the Linggen daemon is up on :9527 — the plugin's MCP server
#    (browser control, x reads, memory_* proxy, agent_run) lives there.
#    Starts the daemon when the `ling` binary exists; when it doesn't,
#    installs the engine in the BACKGROUND (detached — session start never
#    blocks on the ~100MB download) and discloses that in the context line.
#    Both binaries are required components of this plugin. Opt out of the
#    engine auto-install with LINGGEN_NO_ENGINE_INSTALL=1.
# 4. Emit core memory as `hookSpecificOutput.additionalContext` so the
#    host injects always-on identity facts (name, role, location, family,
#    standing-instruction preferences) into the agent's system prompt.
#    CC honors the field natively; Codex ignores unknown JSON and just
#    keeps the side-effect (daemon start). Either way the session never
#    breaks.
#
# Bails silently on any failure — never blocks the session.

set -u

PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-${PLUGIN_ROOT:-}}"

# If neither host injects a plugin-root env var (e.g. Codex's hook runner
# doesn't expand ${PLUGIN_ROOT} when launching shell hooks), we have no
# stable place to read VERSION or scripts/install-bin.sh from. Bail
# silently — the daemon may still be reachable on whatever the user has
# already installed; nothing here can do useful work blind.
[ -z "$PLUGIN_ROOT" ] && exit 0

# ONE canonical, cross-host binary path. Every channel (CC/Codex plugin,
# OpenClaw, skills.sh, Linggen engine) installs/upgrades this same real file —
# never a per-plugin copy or symlink — so there's one binary, one daemon, one
# store. ~/.local/bin (not /usr/local) because plugins/skills install without
# sudo. install-bin.sh defaults to this dir too.
DEST="$HOME/.local/bin"
BIN="$DEST/ling-mem"
# Range-pin the shared binary to the latest 1.x so patch/minor releases reach
# users without a plugin update — matches the engine's LING_MEM_PIN=^1 and the
# SKILL.md first-use gate. Overridable via $LING_MEM_VERSION. The VERSION file
# records the exact release this bundle was built against; autostart floats
# within the major so the one shared cross-host binary stays current.
# (`^1` resolves to the highest v1.x.x; `~1` is NOT valid — install-bin's `~`
# needs X.Y like `~1.0`.)
PIN="${LING_MEM_VERSION:-^1}"
mkdir -p "$DEST" 2>/dev/null || true

# Install/upgrade the shared binary. install-bin resolves the range pin, verifies
# SHA-256, won't downgrade a newer shared binary, and replaces a legacy symlink
# with a real file. Idempotent and cheap when already satisfied.
bash "$PLUGIN_ROOT/scripts/install-bin.sh" \
  --version "$PIN" --dest "$DEST" --quiet >/dev/null 2>&1 || true

[ -x "$BIN" ] || exit 0
HAVE="$("$BIN" --version 2>/dev/null | awk '{print $2}')"

# Reconcile the RUNNING daemon with the on-disk binary: start if down; restart
# only if the daemon is OLDER than the binary on disk (so a stale-pinned host
# never downgrades a daemon another host already started — we only move
# forward). $BIN is used by absolute path to dodge stale PATH caching.
_st="$("$BIN" status --format json 2>/dev/null)"
_state="$(printf '%s' "$_st" | jq -r '.state // empty' 2>/dev/null)"
_running="$(printf '%s' "$_st" | jq -r '.version // empty' 2>/dev/null)"
if [ "$_state" != "running" ]; then
  "$BIN" start >/dev/null 2>&1 || true
elif [ -n "$HAVE" ] && [ -n "$_running" ] && [ "$_running" != "$HAVE" ] && \
     [ "$(printf '%s\n%s\n' "$_running" "$HAVE" | sort -V | head -n1)" = "$_running" ]; then
  "$BIN" restart >/dev/null 2>&1 || true
fi

# ── Ensure the Linggen daemon (the plugin's MCP server) is up ────────────────
#
# The MCP connection in .mcp.json points at http://127.0.0.1:9527/mcp. The
# engine is a required component of this plugin — when the `ling` binary is
# absent, install it here, but DETACHED in the background: session start
# never blocks on the ~100MB download, and the context line below discloses
# the install (README + plugin description disclose it up front). A lock dir
# keeps parallel session starts from racing; a crashed install's stale lock
# is cleared after 30 minutes. LINGGEN_NO_ENGINE_INSTALL=1 opts out.

# ONE source for where the daemon is, read by BOTH this hook and .mcp.json's
# ${LINGGEN_HOST}/${LINGGEN_PORT} expansion — with the SAME defaults, written
# once each here and once each there and nowhere else.
#
# They used to be two independent literals, and they drifted: the 2026-07
# migration flipped .mcp.json to 9527 and left this at 9898, so every session
# start probed a port nothing served and launched a SECOND daemon there — one
# the plugin then never talked to, and which registered the same relay
# instance as the real one and split a paired phone's traffic between them.
#
# A config FILE cannot fix this: Claude Code resolves ${VAR} in .mcp.json at
# startup, before any hook runs, and no hook return value can change the
# environment CC itself reads. An env var with a shared default is the only
# thing both sides can follow.
LINGGEN_HOST="${LINGGEN_HOST:-127.0.0.1}"
LINGGEN_PORT="${LINGGEN_PORT:-9527}"
# Starting a daemon only makes sense for a daemon on THIS machine. Pointed at
# another host, an unreachable engine means "that machine is off" — spawning a
# local one on the same port would answer with the wrong store and hide it.
case "$LINGGEN_HOST" in
  127.0.0.1|localhost|::1|"") LINGGEN_LOCAL=1 ;;
  *) LINGGEN_LOCAL=0 ;;
esac

install_hint=""
if ! curl -fsS --max-time 2 "http://${LINGGEN_HOST}:${LINGGEN_PORT}/api/health" >/dev/null 2>&1 \
   && [ "$LINGGEN_LOCAL" = 1 ]; then
  LING_BIN="$(command -v ling || true)"
  [ -z "$LING_BIN" ] && [ -x "$HOME/.local/bin/ling" ] && LING_BIN="$HOME/.local/bin/ling"
  if [ -n "$LING_BIN" ]; then
    (nohup "$LING_BIN" --web --port "$LINGGEN_PORT" >/dev/null 2>&1 &) || true
  elif [ -n "${LINGGEN_NO_ENGINE_INSTALL:-}" ]; then
    install_hint="Linggen engine not installed (auto-install disabled via LINGGEN_NO_ENGINE_INSTALL) — browser/x/agent MCP tools are offline. Install: curl -fsSL https://linggen.dev/install.sh | bash"
  else
    LOCK="$HOME/.linggen/.engine-install.lock"
    LOG="$HOME/.linggen/engine-install.log"
    mkdir -p "$HOME/.linggen" 2>/dev/null || true
    [ -d "$LOCK" ] && find "$LOCK" -maxdepth 0 -mmin +30 -exec rmdir {} \; 2>/dev/null
    if mkdir "$LOCK" 2>/dev/null; then
      (nohup bash -c '
        curl -fsSL https://linggen.dev/install.sh | bash >>"$1" 2>&1
        LING="$(command -v ling || true)"
        [ -z "$LING" ] && [ -x "$HOME/.local/bin/ling" ] && LING="$HOME/.local/bin/ling"
        [ -n "$LING" ] && nohup "$LING" --web --port "$2" >/dev/null 2>&1 &
        rmdir "$3" 2>/dev/null
      ' _ "$LOG" "$LINGGEN_PORT" "$LOCK" >/dev/null 2>&1 &) || true
      install_hint="Installing the Linggen engine in the background (one-time, ~100MB; log: ~/.linggen/engine-install.log). Browser/x/agent MCP tools come online when it finishes — memory already works via the ling-mem CLI."
    else
      install_hint="Linggen engine install already in progress (log: ~/.linggen/engine-install.log) — browser/x/agent MCP tools come online when it finishes."
    fi
  fi
fi

# ── Inject core memory into the session's system prompt ─────────────────────
#
# Only when jq is available and the daemon answered our query. Empty store
# (fresh install) emits nothing — host gets a normal SessionStart with no
# additionalContext.

command -v jq >/dev/null 2>&1 || exit 0
[ -x "$BIN" ] || exit 0

core_rows="$("$BIN" list --tier core --limit 100 --format json --quiet 2>/dev/null || true)"
if [ -z "$core_rows" ]; then
  if [ -n "$install_hint" ]; then
    jq -nc --arg ctx "$install_hint" '{hookSpecificOutput:{hookEventName:"SessionStart",additionalContext:$ctx}}'
  fi
  exit 0
fi

# Defensive guard: if `ling-mem list --quiet` somehow leaked non-JSON
# to stdout (daemon bug, mixed warning, partial response), the jq
# pipeline below would fail silently and the session would start with
# no core context and no log of why. Validate parse first; on failure,
# log to stderr (CC shows hook stderr in the transcript) and bail
# cleanly without emitting hookSpecificOutput.
if ! printf '%s' "$core_rows" | jq -es '.' >/dev/null 2>&1; then
  printf 'linggen autostart: ling-mem list --tier core returned non-JSON; skipping core injection\n' >&2
  exit 0
fi

core_block="$(printf '%s' "$core_rows" | jq -sr '
  map(. // empty)
  | flatten
  | map(select(.content))
  | if length == 0 then empty
    else
      "## Core memory — always-on user identity\n\n"
      + (map("- \(.content) (id=\(.id))") | join("\n"))
    end
' 2>/dev/null || true)"

if [ -n "$install_hint" ]; then
  core_block="${core_block}${core_block:+\n\n}${install_hint}"
fi
[ -z "$core_block" ] && exit 0

jq -nc --arg ctx "$core_block" '{
  hookSpecificOutput: {
    hookEventName: "SessionStart",
    additionalContext: $ctx
  }
}'
