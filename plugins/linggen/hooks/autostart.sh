#!/usr/bin/env bash
#
# SessionStart hook for the linggen plugin.
#
# One program, one address. Linggen (`ling`, the engine) hosts memory; the
# plugin reaches it at the one address mcp.sh resolves (default
# 127.0.0.1:9527): memory at /mcp/memory, the engine's tools at /mcp.
#
# 1. Address on THIS machine: make sure Linggen is up. Running → nothing to
#    do. `ling` installed → start it (`ling --web`) and give it a few seconds.
#    Not installed → install it in the BACKGROUND (detached — session start
#    never blocks on the ~100MB download) and disclose that in the context
#    line. Linggen is a required component of this plugin; opt out of the
#    auto-install with LINGGEN_NO_ENGINE_INSTALL=1.
# 2. Address on ANOTHER machine (a machine with only the plugin): install
#    nothing, start nothing. A local Linggen would open a second, empty store
#    — the forked memory this whole arrangement exists to prevent. Such a host
#    needs the address and that machine's token, nothing more.
# 3. Emit what a session loads at start (one `memory_session_start` call with
#    the session's cwd and root): core memory, the "Memory scopes here"
#    candidates line, and the index of standing rows filed for this directory
#    and its parents — as `hookSpecificOutput.additionalContext` so the host
#    injects it into the agent's system prompt. Claude Code has no cd hook, so
#    the index is this directory's at start; recall covers the rest. Read over
#    MCP, so it works the same whether memory is on this machine or another.
#    CC honors the field natively; Codex ignores unknown JSON and keeps the
#    side effect (Linggen started). Either way the session never breaks.
#
# Bails silently on any failure — never blocks the session.

set -u

# Read the hook's stdin first (the session's cwd rides in it), so
# the engine start below never inherits an open pipe.
hook_input=""
[ -t 0 ] || hook_input="$(cat 2>/dev/null || true)"

# Address + `mcp_call`, shared with recall.sh. Located from this script's own
# path so it works regardless of which env vars a host sets.
# shellcheck source=./mcp.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/mcp.sh" 2>/dev/null || exit 0

PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-${PLUGIN_ROOT:-}}"

# If neither host injects a plugin-root env var (e.g. Codex's hook runner
# doesn't expand ${PLUGIN_ROOT} when launching shell hooks), we have no
# stable place to read the installer from. Bail silently — Linggen may still
# be reachable on whatever the user has already installed.
[ -z "$PLUGIN_ROOT" ] && exit 0

# Which host is running this hook — observed from where the plugin actually
# lives, never asserted. Feeds the install-source marker's agent= field.
case "$PLUGIN_ROOT" in
  */.claude/*) PLUGIN_AGENT=cc ;;
  */.codex/*)  PLUGIN_AGENT=codex ;;
  *)           PLUGIN_AGENT="" ;;
esac

# ── Make sure Linggen is up (this machine only) ─────────────────────────────
#
# `LINGGEN_URL` and `LINGGEN_LOCAL` come from mcp.sh, which resolves them from
# `~/.linggen/client.json` (file > env > default) — ONE authored address for
# every hook. `.mcp.json` still needs the same values as environment variables,
# because Claude Code expands it at startup before any hook runs; `config.sh`
# mirrors them there when it writes the file.

healthy() { curl -fsS --max-time 2 "${LINGGEN_URL}/api/health" >/dev/null 2>&1; }

# Where `ling` is: LINGGEN_BIN (an explicit binary), else PATH, else the
# installer's fallback dir.
find_ling() {
  if [ -n "${LINGGEN_BIN:-}" ]; then
    [ -x "$LINGGEN_BIN" ] && printf '%s' "$LINGGEN_BIN"
    return 0
  fi
  local l
  l="$(command -v ling 2>/dev/null || true)"
  [ -z "$l" ] && [ -x "$HOME/.local/bin/ling" ] && l="$HOME/.local/bin/ling"
  printf '%s' "$l"
}

install_hint=""
if [ "$LINGGEN_LOCAL" = 1 ] && ! healthy; then
  LING_BIN="$(find_ling)"
  if [ -n "$LING_BIN" ]; then
    # No `--port`: the engine binds from its own `[server].url`. Passing a
    # port here would override the user's config with this hook's reading of
    # it, which is how an engine ends up somewhere its own config denies.
    # (A plain command, not `a && b &`: that backgrounds a whole subshell,
    # which then lingers as the engine's parent.)
    (cd "$HOME" || exit 0; nohup "$LING_BIN" --web >/dev/null 2>&1 &) || true
    # Memory opens with the engine; wait briefly so this session's core block
    # comes from it rather than from nothing.
    for _ in $(seq 1 "${LINGGEN_START_WAIT:-10}"); do healthy && break; sleep 1; done
  elif [ -n "${LINGGEN_NO_ENGINE_INSTALL:-}" ]; then
    install_hint="Linggen is not installed (auto-install disabled via LINGGEN_NO_ENGINE_INSTALL) — memory and the browser/x/agent tools are offline. Install: curl -fsSL https://linggen.dev/install.sh | bash"
  else
    # The installer that ships in this plugin (skills/linggen/scripts/
    # install-engine.sh, the linggen.dev installer vendored verbatim) — never
    # a remotely fetched script. LINGGEN_NO_REMOTE_SCRIPT=1 also keeps it from
    # fetching anything but the release binary. LINGGEN_INSTALL_SH names
    # another local installer (tests stand one in for the download).
    INSTALLER="${LINGGEN_INSTALL_SH:-$PLUGIN_ROOT/skills/linggen/scripts/install-engine.sh}"
    LOCK="$HOME/.linggen/.engine-install.lock"
    LOG="$HOME/.linggen/engine-install.log"
    mkdir -p "$HOME/.linggen" 2>/dev/null || true
    [ -d "$LOCK" ] && find "$LOCK" -maxdepth 0 -mmin +30 -exec rmdir {} \; 2>/dev/null
    if [ ! -f "$INSTALLER" ]; then
      install_hint="Linggen is not installed and this plugin's installer is missing ($INSTALLER) — memory and the browser/x/agent tools are offline. Install: curl -fsSL https://linggen.dev/install.sh | bash"
    elif mkdir "$LOCK" 2>/dev/null; then
      (cd "$HOME" || exit 0; LINGGEN_NO_REMOTE_SCRIPT=1 \
        LINGGEN_SOURCE="${LINGGEN_SOURCE:-plugin}" LINGGEN_AGENT="${LINGGEN_AGENT:-$PLUGIN_AGENT}" \
        nohup bash -c '
          bash "$1" >>"$2" 2>&1
          LING="${LINGGEN_BIN:-$(command -v ling || true)}"
          [ -z "$LING" ] && [ -x "$HOME/.local/bin/ling" ] && LING="$HOME/.local/bin/ling"
          if [ -n "$LING" ]; then nohup "$LING" --web >/dev/null 2>&1 & fi
          rmdir "$3" 2>/dev/null
        ' _ "$INSTALLER" "$LOG" "$LOCK" >/dev/null 2>&1 &) || true
      install_hint="Installing Linggen in the background (one-time, ~100MB; log: ~/.linggen/engine-install.log). Memory and the browser/x/agent tools come online when it finishes."
    else
      install_hint="Linggen install already in progress (log: ~/.linggen/engine-install.log) — memory and the browser/x/agent tools come online when it finishes."
    fi
  fi
fi

# ── Keep Claude Code's MCP wiring in step with client.json ──────────────────
#
# `.mcp.json` reads env that Claude Code expanded at startup; client.json is
# the authority (the engine's Settings page writes it too). When they differ,
# re-mirror through config.sh — the one writer — and say a restart applies it.

mirror_hint=""
if [ "$PLUGIN_AGENT" = cc ] && [ -r "$_ling_client_file" ]; then
  # Exit 0: the env was out of step and is now rewritten; 10: already matched.
  if bash "$PLUGIN_ROOT/scripts/config.sh" --mirror >/dev/null 2>&1; then
    mirror_hint="Linggen address changed — restart Claude Code to apply."
  fi
fi

# ── Inject what a session loads at start ────────────────────────────────────
#
# One call: `memory_session_start` with this session's cwd and root returns
# the core rows (who the user is), the scope candidates line and the index,
# already rendered as `block`. Linggen renders it so every host injects the
# same text. Everything else surfaces through per-turn recall (recall.sh).
# Over MCP, like recall — a host whose memory is on another machine gets the
# same block with no binary of its own. Empty store (fresh install) emits
# nothing.

command -v jq >/dev/null 2>&1 || exit 0

# shellcheck source=./scope.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/scope.sh" 2>/dev/null || true
session_cwd="$(printf '%s' "$hook_input" | jq -r '.cwd // empty' 2>/dev/null || true)"
[ -n "$session_cwd" ] || session_cwd="${CLAUDE_PROJECT_DIR:-$PWD}"
session_root=""
if command -v session_root >/dev/null 2>&1; then
  session_root="$(session_root "$session_cwd")"
fi
start_args="$(jq -nc --arg c "$session_cwd" --arg r "$session_root" '
  {cwd: $c} + (if ($r | length) > 0 then {root: $r} else {} end)
' 2>/dev/null || printf '{}')"

# A slightly longer budget than a per-turn recall: this runs once, at session
# start, and an engine that just started may still be opening the store.
to="${LINGGEN_CORE_TIMEOUT:-5}"
start_out="$(mcp_call memory_session_start "$start_args" "$to")"

# Defensive guard: a malformed payload would make the pipeline below fail
# silently, and the session would start with no core context and no log of
# why. Validate the parse first; on failure log to stderr (CC shows hook
# stderr in the transcript) and inject nothing.
core_block=""
if [ -n "$start_out" ]; then
  if printf '%s' "$start_out" | jq -e 'type == "object"' >/dev/null 2>&1; then
    core_block="$(printf '%s' "$start_out" | jq -r '.block // empty' 2>/dev/null || true)"
  else
    printf 'linggen autostart: memory_session_start returned non-JSON; skipping memory injection\n' >&2
  fi
fi

for hint in "$mirror_hint" "$install_hint"; do
  [ -n "$hint" ] || continue
  core_block="${core_block}${core_block:+

}${hint}"
done
[ -z "$core_block" ] && exit 0

jq -nc --arg ctx "$core_block" '{
  hookSpecificOutput: {
    hookEventName: "SessionStart",
    additionalContext: $ctx
  }
}'
