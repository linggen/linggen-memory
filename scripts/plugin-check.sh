#!/usr/bin/env bash
# Host-plugin checks for Linggen's memory: install parity, the CC/Codex hooks,
# the OpenClaw modules, one address everywhere, and protocol-text alignment
# across every memory surface.
#
#   ./scripts/plugin-check.sh                      parity + Codex schema +
#                                                  OpenClaw tests + text grep
#   ./scripts/plugin-check.sh --sync               also copy drifted repo files
#                                                  over each installed copy
#   scripts/live-check.sh runs it with --port/--home/--project/--skill so the
#   hooks run for real against its scratch Linggen and fixture home.
#
# Never writes the store; never `rsync --delete` into ~/.linggen/skills (that
# wipes skills' data/). --sync copies file by file and deletes nothing.

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REAL_HOME="$HOME"
PORT="" FHOME="" PROJ="" SKILL="" SYNC=0
while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift ;;
    --home) FHOME="$2"; shift ;;
    --project) PROJ="$2"; shift ;;
    --skill) SKILL="$2"; shift ;;
    --sync) SYNC=1 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "plugin-check: unknown flag $1" >&2; exit 2 ;;
  esac
  shift
done

N_FAIL=0
report() { printf '%-4s  %-52s %s\n' "$1" "$2" "${3:-}"; [ "$1" = FAIL ] && N_FAIL=$((N_FAIL + 1)); return 0; }
pass() { report PASS "$1" "${2:-}"; }
fail() { report FAIL "$1" "${2:-}"; }
skip() { report SKIP "$1" "${2:-}"; }
expect() {
  if printf '%s' "$2" | jq -e "$3" >/dev/null 2>&1; then pass "$1" "${5:-}"
  else fail "$1" "$4 — got: $(printf '%s' "$2" | head -c 200 | tr '\n' ' ')"; fi
}
md5of() { if command -v md5 >/dev/null; then md5 -q "$1"; else md5sum "$1" | cut -d' ' -f1; fi; }

# ── 1. Parity: repo plugin dirs vs every installed copy ─────────────────────

CCP="$REPO/plugins/linggen"
OCP="$REPO/plugins/openclaw"
SKP="${SHARED_MEMORY_SKILL:-$(cd "$REPO/.." && pwd)/skills/shared-memory}"
CC_VER="$(jq -r .version "$CCP/.claude-plugin/plugin.json")"

# parity NAME SRC DST [find-args…] — files SRC ships, compared by md5.
parity() {
  local name="$1" src="$2" dst="$3"; shift 3
  if [ ! -d "$dst" ]; then skip "parity: $name" "not installed ($dst)"; return; fi
  local drift=() missing=() f
  while IFS= read -r f; do
    if [ ! -f "$dst/$f" ]; then missing+=("$f")
    elif [ "$(md5of "$src/$f")" != "$(md5of "$dst/$f")" ]; then drift+=("$f"); fi
  done < <(cd "$src" && find . -type f ! -name .DS_Store "$@" | sed 's|^\./||' | sort)
  local bad=("${drift[@]+"${drift[@]}"}" "${missing[@]+"${missing[@]}"}")
  if [ ${#bad[@]} -eq 0 ]; then pass "parity: $name" "md5 match"; return; fi
  if [ "$SYNC" = 1 ]; then
    for f in "${bad[@]}"; do mkdir -p "$(dirname "$dst/$f")" && cp -p "$src/$f" "$dst/$f"; done
    pass "parity: $name" "synced ${#bad[@]}: ${bad[*]}"
  else
    fail "parity: $name" "${#drift[@]} drifted, ${#missing[@]} missing: ${bad[*]:0:6} (--sync)"
  fi
}
parity "CC marketplace" "$CCP" "$REAL_HOME/.claude/plugins/marketplaces/linggen-memory/plugins/linggen"
parity "CC cache $CC_VER" "$CCP" "$REAL_HOME/.claude/plugins/cache/linggen-memory/linggen/$CC_VER"
parity "Codex cache $CC_VER" "$CCP" "$REAL_HOME/.codex/plugins/cache/linggen-memory/linggen/$CC_VER"
# OpenClaw ships what package.json "files" lists (tests and .gitignore stay home).
parity "OpenClaw extension" "$OCP" "$REAL_HOME/.openclaw/extensions/linggen" \
  ! -path './test/*' ! -path './node_modules/*' ! -name .gitignore ! -name 'package-lock.json'
if [ -d "$SKP" ]; then
  parity "Linggen skill shared-memory" "$SKP" "$REAL_HOME/.linggen/skills/shared-memory" \
    ! -path './tests/*' ! -path './data/*' ! -path './.git/*'
else
  skip "parity: Linggen skill shared-memory" "no repo copy at $SKP"
fi

# ── 2. Codex hook declaration ───────────────────────────────────────────────

CX="$(cat "$CCP/hooks/codex.hooks.json")"
expect "Codex hooks: nested map keyed by event" "$CX" \
  '(.hooks | type == "object") and (.hooks | keys | all(IN("SessionStart","UserPromptSubmit","PreToolUse","PostToolUse","Stop"))) and ([.hooks[][] | .hooks[] | .type] | all(. == "command"))' \
  "not {hooks:{Event:[{hooks:[{type:command}]}]}}"
expect "Codex hooks: absolute \${CLAUDE_PLUGIN_ROOT} paths" "$CX" \
  '[.hooks[][] | .hooks[].command | sub("^([A-Z_]+=[^ ]+ )+"; "")] | all(startswith("${CLAUDE_PLUGIN_ROOT}/"))' "a relative command path"
for cmd in $(printf '%s' "$CX" | jq -r '.hooks[][] | .hooks[].command | sub("^([A-Z_]+=[^ ]+ )+"; "")' | sed 's|${CLAUDE_PLUGIN_ROOT}/||'); do
  [ -x "$CCP/$cmd" ] && pass "Codex hooks: $cmd executable" || fail "Codex hooks: $cmd executable" "missing or not +x"
done
CCH="$(cat "$CCP/hooks/hooks.json")"
expect "CC hooks: PreToolUse stamps add/search/session_start" "$CCH" \
  '.hooks.PreToolUse[0].matcher as $m | ("mcp__plugin_linggen_memory__memory_add" | test($m)) and ("mcp__x__memory_search" | test($m)) and ("mcp__x__memory_session_start" | test($m)) and (("mcp__x__memory_delete" | test($m)) | not)' \
  "matcher misses a verb"

# One address: every declaration names Linggen's port and nothing else; memory
# is /mcp/memory beside the engine's /mcp. The retired port never appears.
MJ="$(cat "$CCP/.mcp.json")"
expect "CC .mcp.json: memory + linggen on one address" "$MJ" \
  '(.mcpServers | keys) == ["linggen","memory"] and (.mcpServers.memory.url | endswith(":${LINGGEN_PORT:-9527}/mcp/memory")) and (.mcpServers.linggen.url | endswith(":${LINGGEN_PORT:-9527}/mcp")) and ((.mcpServers.memory.url | sub("/memory$"; "")) == .mcpServers.linggen.url)' \
  "servers or URLs differ"
CXM="$(cat "$CCP/.codex-plugin/mcp.json")"
expect "Codex mcp.json: memory + linggen on one address" "$CXM" \
  '(.mcpServers | keys) == ["linggen","memory"] and .mcpServers.memory.url == "http://127.0.0.1:9527/mcp/memory" and .mcpServers.linggen.url == "http://127.0.0.1:9527/mcp"' \
  "servers or URLs differ"
old="$(grep -rnIE '9528|LING_MEM_(HOST|PORT|TOKEN)|ling-mem (start|restart|status|upgrade)|install-bin' "$CCP/.mcp.json" "$CCP/.codex-plugin" "$CCP/hooks" "$CCP/scripts" "$OCP/src" 2>/dev/null \
  | grep -vE 'retired|old memory|older file|legacy|_ling_pick|pick\(' || true)"
[ -z "$old" ] && pass "no retired address or daemon verbs in code" \
  || fail "no retired address or daemon verbs in code" "$(printf '%s' "$old" | head -3 | tr '\n' ' ')"

# ── 3. Hooks against the scratch Linggen ────────────────────────────────────

if [ -n "$PORT" ] && [ -n "$FHOME" ] && [ -n "$PROJ" ]; then
  WORKP="$(dirname "$FHOME")/plugin"
  mkdir -p "$WORKP/bin"
  # A PATH with jq/curl/git/node only — no `ling`, so autostart cannot start
  # the real engine; the proxy answers health, so it starts nothing anyway.
  for t in jq curl git node python3 bash; do ln -sf "$(command -v "$t")" "$WORKP/bin/$t"; done

  # A recording proxy in front of the scratch engine: every MCP body a hook sends
  # lands in $LOG, so the checks can see the args, not just the answers.
  PPORT=$((PORT + 10))
  LOG="$WORKP/mcp.log"
  : >"$LOG"
  python3 - "$PPORT" "$PORT" "$LOG" <<'PY' &
import sys, urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
port, up, log = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        r = urllib.request.urlopen(f"http://127.0.0.1:{up}{self.path}")
        b = r.read(); self.send_response(r.status); self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        with open(log, "ab") as f: f.write(body + b"\n")
        req = urllib.request.Request(f"http://127.0.0.1:{up}{self.path}", body, {"content-type": "application/json"})
        b = urllib.request.urlopen(req).read()
        self.send_response(200); self.send_header("content-type", "application/json"); self.end_headers(); self.wfile.write(b)
HTTPServer(("127.0.0.1", port), H).serve_forever()
PY
  PROXY=$!
  disown "$PROXY" 2>/dev/null
  trap 'kill $PROXY 2>/dev/null' EXIT
  for _ in $(seq 1 30); do curl -s --max-time 1 "http://127.0.0.1:$PPORT/api/health" >/dev/null 2>&1 && break; sleep 0.2; done

  hook_env=(env -i HOME="$FHOME" PATH="$WORKP/bin:/usr/bin:/bin" TMPDIR="$WORKP" LC_ALL=en_US.UTF-8
    LINGGEN_HOST=127.0.0.1 LINGGEN_PORT="$PPORT" LINGGEN_NO_ENGINE_INSTALL=1
    LINGGEN_UPKEEP_CACHE_MIN=0)
  sent() { grep "\"$1\"" "$LOG" | tail -1 | jq -c '.params.arguments' 2>/dev/null; }

  # run_hooks HOST PLUGIN-ROOT RUN-DIR — the three hooks as HOST launches them.
  run_hooks() {
    local host="$1" root="$2" rundir="$3" out
    : >"$LOG"
    out="$(cd "$rundir" && printf '%s' "$(jq -nc --arg c "$PROJ/story" '{session_id:"pc-1", hook_event_name:"SessionStart", source:"startup", cwd:$c}')" \
      | "${hook_env[@]}" CLAUDE_PLUGIN_ROOT="$root" ${CC_PROJECT:+CLAUDE_PROJECT_DIR="$PROJ"} bash "$root/hooks/autostart.sh" 2>&1)"
    expect "$host autostart: valid JSON, core + candidates + index" "$out" \
      '.hookSpecificOutput.hookEventName == "SessionStart" and (.hookSpecificOutput.additionalContext | test("## Core memory") and test("Memory scopes here") and test("## Index — "))' \
      "SessionStart context incomplete"
    expect "$host autostart: sends cwd + root" "$(sent memory_session_start)" \
      '.cwd == "'"$PROJ/story"'" and .root == "'"$PROJ"'"' "session_start args"
    out="$(cd "$rundir" && jq -nc --arg c "$PROJ" '{session_id:"pc-1", hook_event_name:"UserPromptSubmit", prompt:"what is the central artifact in the nine bronze cauldrons novel?", cwd:$c}' \
      | "${hook_env[@]}" CLAUDE_PLUGIN_ROOT="$root" bash "$root/hooks/recall.sh" 2>&1)"
    if printf '%s' "$out" | grep -q '^From memory (.*from=\(user\|derived\|agent\).*id=' && printf '%s' "$out" | grep -q 'Memory capture:'; then
      pass "$host recall: lines carry from=, capture nudge" "$(printf '%s' "$out" | grep -c '^From memory') hits"
    else
      fail "$host recall: lines carry from=, capture nudge" "$(printf '%s' "$out" | head -c 200 | tr '\n' ' ')"
    fi
    expect "$host recall: scope_root sent (not cwd_scope)" "$(sent memory_search)" \
      '.scope_root == "'"$PROJ"'" and (has("cwd_scope") | not)' "search args"
    if printf '%s' "$out" | grep -q 'Sanji'; then fail "$host recall: no cross-project rows" "a sanji row surfaced"
    else pass "$host recall: no cross-project rows"; fi
  }
  CC_PROJECT=1 run_hooks "CC" "$CCP" "$PROJ"
  # Codex: hooks from the plugin root it installed, run from an unrelated cwd,
  # no CLAUDE_PROJECT_DIR.
  CC_PROJECT="" run_hooks "Codex" "$CCP" "/"

  # stamp-cwd.sh with PreToolUse payloads.
  stamp() { # tool tool_input-json cwd
    jq -nc --arg t "$1" --argjson i "$2" --arg c "$3" '{tool_name:$t, session_id:"pc-1", cwd:$c, hook_event_name:"PreToolUse", tool_input:$i}' \
      | "${hook_env[@]}" ${STAMP_PROJECT:+CLAUDE_PROJECT_DIR="$STAMP_PROJECT"} bash "$CCP/hooks/stamp-cwd.sh" 2>&1
  }
  STAMP_PROJECT="$PROJ"
  T=mcp__plugin_linggen_memory__memory_add
  out="$(stamp "$T" '{"content":"x","scope":"~/workspace","summary":"s","indexed":false}' "$PROJ/story")"
  expect "stamp add: fills cwd/root/source_session/host" "$out" \
    '.hookSpecificOutput.permissionDecision == "allow" and (.hookSpecificOutput.updatedInput | .cwd == "'"$PROJ/story"'" and .root == "'"$PROJ"'" and .source_session == "pc-1" and .host == "claude-code")' \
    "missing stamps"
  expect "stamp add: scope ~, summary, indexed:false intact" "$out" \
    '.hookSpecificOutput.updatedInput | .scope == "~/workspace" and .summary == "s" and .indexed == false and (.indexed | type) == "boolean"' \
    "model fields changed"
  # No output = nothing to stamp = the call goes through as the caller wrote it.
  out="$(stamp "$T" '{"content":"x","cwd":"/elsewhere","root":"/elsewhere","source_session":"other","host":"codex"}' "$PROJ")"
  [ -n "$out" ] || out='{"hookSpecificOutput":{"updatedInput":{"cwd":"/elsewhere","root":"/elsewhere","source_session":"other","host":"codex"}}}' 
  expect "stamp add: never overwrites caller values" "$out" \
    '(.hookSpecificOutput.updatedInput | .cwd == "/elsewhere" and .root == "/elsewhere" and .source_session == "other" and .host == "codex")' \
    "caller value overwritten"
  out="$(stamp "$T" '{"content":"x","source_session":"another-session"}' "$PROJ")"
  expect "stamp add: foreign session keeps its own scope" "$out" \
    '.hookSpecificOutput.updatedInput | (has("cwd") | not) and (has("root") | not) and .source_session == "another-session"' \
    "this session's cwd stamped onto another session's row"
  out="$(stamp mcp__plugin_linggen_memory__memory_search '{"query":"q"}' "$PROJ/story")"
  expect "stamp search: scope_root = root" "$out" '.hookSpecificOutput.updatedInput | .scope_root == "'"$PROJ"'" and (has("cwd_scope") | not)' "scope_root"
  out="$(stamp mcp__plugin_linggen_memory__memory_search '{"query":"q","scope_root":"/mine"}' "$PROJ")"
  [ -z "$out" ] && pass "stamp search: caller scope_root kept" || fail "stamp search: caller scope_root kept" "$out"
  # A session started at $HOME (no project dir): a whole-store lookup.
  out="$(STAMP_PROJECT="" stamp mcp__plugin_linggen_memory__memory_search '{"query":"q"}' "$FHOME")"
  [ -z "$out" ] && pass "stamp search: \$HOME stays a whole-store lookup" || fail "stamp search: \$HOME stays a whole-store lookup" "$out"
  out="$(stamp mcp__plugin_linggen_memory__memory_session_start '{}' "$PROJ/story")"
  expect "stamp session_start: cwd + root" "$out" '.hookSpecificOutput.updatedInput | .cwd == "'"$PROJ/story"'" and .root == "'"$PROJ"'"' "session_start stamp"
  if [ -n "$SKILL" ]; then
    out="$(stamp "$T" '{"content":"x"}' "$SKILL/sub")"
    expect "stamp add: skill dir is its own root" "$out" '.hookSpecificOutput.updatedInput.root == "'"$SKILL"'"' "skill root"
  fi
  # A session started in a non-git workspace, shell cd'd into a nested repo:
  # root stays the workspace the candidates were shown against (the
  # 2026-10-05 bug stamped the nested repo's git root and the daemon dropped
  # the model's scope for cwd). A cwd outside the start dir roots on its own.
  WS="$(dirname "$PROJ")" SIB=""
  for d in "$WS"/*; do
    [ -d "$d/.git" ] && [ "$d" != "$PROJ" ] && { SIB="$d"; break; }
  done
  if [ -n "$SIB" ] && [ ! -e "$WS/.git" ]; then
    out="$(STAMP_PROJECT="$WS" stamp "$T" '{"content":"x","scope":"workspace/lingjing"}' "$SIB")"
    expect "stamp add: nested repo keeps the session root" "$out" \
      '.hookSpecificOutput.updatedInput | .root == "'"$WS"'" and .cwd == "'"$SIB"'" and .scope == "workspace/lingjing"' "root = nested git root"
    out="$(STAMP_PROJECT="$WS" stamp mcp__plugin_linggen_memory__memory_search '{"query":"q"}' "$PROJ/story")"
    expect "stamp search: nested repo keeps the session root" "$out" \
      '.hookSpecificOutput.updatedInput.scope_root == "'"$WS"'"' "scope_root = nested git root"
    out="$(STAMP_PROJECT="$PROJ" stamp "$T" '{"content":"x"}' "$SIB")"
    expect "stamp add: cwd outside the start dir roots on its own" "$out" \
      '.hookSpecificOutput.updatedInput.root == "'"$SIB"'"' "foreign cwd took the start root"
  else
    skip "stamp add: nested repo keeps the session root" "no sibling repo under a non-git $WS"
  fi
  out="$(stamp mcp__plugin_linggen_memory__memory_delete '{"id":"x"}' "$PROJ")"
  [ -z "$out" ] && pass "stamp: other tools untouched" || fail "stamp: other tools untouched" "$out"

  # OpenClaw modules against the same engine (through the proxy).
  : >"$LOG"
  oc="$(cd / && "${hook_env[@]}" node --input-type=module -e '
    const root = process.argv[1], proj = process.argv[2];
    const { resolveClient, readSettings } = await import(root + "/src/config.mjs");
    const { buildCoreContext } = await import(root + "/src/core.mjs");
    const { buildRecallContext } = await import(root + "/src/recall.mjs");
    const { stampCwd } = await import(root + "/src/stamp-cwd.mjs");
    const client = resolveClient();
    const core = await buildCoreContext(client, 8000, proj + "/story");
    const recall = await buildRecallContext({ client, prompt: "what is the central artifact in the nine bronze cauldrons novel?", cwd: proj, settings: { ...readSettings(), recallTimeoutMs: 8000 } });
    const stamped = stampCwd({ toolName: "memory__memory_add", params: { content: "x", scope: "~/workspace", indexed: false }, cwd: proj + "/story", sessionId: "oc-1" });
    console.log(JSON.stringify({ core, recall, stamped }));
  ' "$OCP" "$PROJ" 2>&1)"
  expect "OpenClaw core: core + candidates + index" "$oc" '.core | test("## Core memory") and test("Memory scopes here") and test("## Index — ")' "core block"
  expect "OpenClaw recall: from= lines, scope_root sent" "$oc" '.recall | test("From memory \\([a-z]+, from=")' "recall lines"
  expect "OpenClaw recall: scope_root arg" "$(sent memory_search)" '.scope_root == "'"$PROJ"'"' "search args"
  expect "OpenClaw stamp: cwd/root/host, model fields intact" "$oc" \
    '.stamped | .cwd == "'"$PROJ/story"'" and .root == "'"$PROJ"'" and .host == "openclaw" and .source_session == "oc-1" and .scope == "~/workspace" and .indexed == false' \
    "stamp"
  kill "$PROXY" 2>/dev/null
else
  skip "hooks against a scratch Linggen" "run via scripts/live-check.sh"
fi

# ── 4. OpenClaw package ─────────────────────────────────────────────────────

if command -v node >/dev/null; then
  bad=""
  for f in "$OCP"/src/*.mjs; do node --check "$f" 2>/dev/null || bad="$bad $(basename "$f")"; done
  [ -z "$bad" ] && pass "OpenClaw: node --check src/*.mjs" || fail "OpenClaw: node --check src/*.mjs" "$bad"
  t="$(cd "$OCP" && npm test --silent 2>&1)"
  if [ $? -eq 0 ]; then pass "OpenClaw: npm test" "$(printf '%s' "$t" | grep -E '^# pass' | head -1)"
  else fail "OpenClaw: npm test" "$(printf '%s' "$t" | grep -E 'not ok|# fail' | head -3 | tr '\n' ' ')"; fi
else
  skip "OpenClaw: node" "node not installed"
fi

# ── 5. Protocol text alignment ──────────────────────────────────────────────
#
# Stale v1 terms on any surface a model or a host reads. Each pattern is a
# claim the v2 protocol no longer makes; a surface that still makes it teaches
# the old store. Code that keeps an alias on purpose (cwd_scope) is matched
# only where it is presented as the name to use.

ENGINE="$(cd "$REPO/.." && pwd)/linggen"
SURFACES=(
  "$CCP/skills/linggen" "$CCP/commands" "$CCP/README.md" "$CCP/hooks"
  "$OCP/skills" "$OCP/commands" "$OCP/README.md" "$OCP/src"
)
ENGINE_SURFACES=(
  "$ENGINE/src/server/mcp.rs" "$ENGINE/crates/linggen-engine/src/engine/tools/memory_mcp.rs"
  "$ENGINE/crates/linggen-memory/src/http/mcp.rs" "$ENGINE/agents/memory.md"
)
for m in "$ENGINE"/missions/*dream* "$ENGINE"/agents/missions/*dream* "$(cd "$REPO/.." && pwd)"/skills/*/missions/*dream*; do
  [ -e "$m" ] && ENGINE_SURFACES+=("$m")
done
STALE=(
  'cwd_scope[^|]*(the|is|=) *(session|root|scope)|pass `?cwd_scope'
  '\bhook\b[^.]{0,40}(≤ ?80|one[ -]line([^r]|$)|summary line)'
  '"contexts"|`contexts`|contexts_any|--context\b'
  '"tags"|`tags`|--tag\b'
  '[Ss]tanding rules? (load|loaded|block|section)|## Standing rules'
  '[Rr]eview queue[^.]{0,80}(scope|index|summary|hook)( |,|\.)'
  'hook required|requires a hook|hook is required'
  # The retired memory daemon, presented as current: its port, its verbs, its
  # CLI name as the thing to run. (Lines that say it is retired are let through.)
  '127\.0\.0\.1:9528|:9528/mcp|`ling-mem [a-z-]+|ling-mem (daemon|start|upgrade|console)|Bash ling-mem'
)
scan() { # [--list] label paths… — --list: another repo owns these; print, don't fail
  local verdict=fail
  [ "$1" = --list ] && { verdict=list; shift; }
  local label="$1"; shift
  local hits="" p
  for p in "${STALE[@]}"; do
    hits="$hits$(grep -rnIE "$p" "$@" 2>/dev/null | grep -vE 'not queued|no longer|retired|dropped|removed|was `?cwd|alias|Was `|first shipped as|Removed|replaced|not by|assert|Historical|/scripts/install-engine\.sh:|/doc/shared-memory-design\.md:' || true)"$'\n'
  done
  hits="$(printf '%s' "$hits" | sed '/^$/d' | sort -u)"
  if [ -z "$hits" ]; then pass "text: $label" "no stale terms"
  else
    if [ "$verdict" = list ]; then report LIST "text: $label" "$(printf '%s\n' "$hits" | wc -l | tr -d ' ') stale line(s) to change there"
    else fail "text: $label" "$(printf '%s\n' "$hits" | wc -l | tr -d ' ') stale line(s)"; fi
    printf '%s\n' "$hits" | sed "s|$REAL_HOME|~|; s/^/        /" | head -20
  fi
}
scan "plugins + skill" "${SURFACES[@]}"
SKS=()
for p in "$SKP/SKILL.md" "$SKP/references" "$SKP/doc" "$SKP/README.md"; do [ -e "$p" ] && SKS+=("$p"); done
if [ ${#SKS[@]} -gt 0 ]; then scan --list "shared-memory skill (the skills repo owns it)" "${SKS[@]}"
else skip "text: shared-memory skill" "no repo copy at $SKP"; fi
# This repo's Rust crate still serves its own MCP text (the standalone
# binary); its future is decided apart from the plugins, so it is listed.
[ -e "$REPO/src/http/mcp.rs" ] && scan --list "this repo's crate (src/http/mcp.rs; decided apart from the plugins)" "$REPO/src/http/mcp.rs"
ENG=()
for p in "${ENGINE_SURFACES[@]}"; do [ -e "$p" ] && ENG+=("$p"); done
if [ ${#ENG[@]} -gt 0 ]; then scan --list "engine surfaces (the engine repo owns them)" "${ENG[@]}"
else skip "text: engine surfaces" "no ../linggen checkout"; fi

[ "$N_FAIL" -eq 0 ]
