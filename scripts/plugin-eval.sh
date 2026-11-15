#!/usr/bin/env bash
# `claude plugin eval` suite for the linggen Claude Code plugin, isolated from
# the real store.
#
#   ./scripts/plugin-eval.sh                  every case, 1 run, haiku, no baseline
#   ./scripts/plugin-eval.sh [eval flags…]    flags replace those defaults, e.g.
#                                             --runs 3 --model sonnet --case 'remember-*'
#   ./scripts/check.sh eval [eval flags…]     the same, from the check entry point
#   PLUGIN_EVAL_KEEP=1 ./scripts/plugin-eval.sh …   keep each run's temp dir
#                                             (trace.jsonl, mock-calls.jsonl)
#
# It calls a real model on the signed-in Claude Code account (subscription or
# API key — whatever `claude` uses), so it is NOT part of the default suite.
#
# The suite lives in plugins/linggen/evals-isolated/, deliberately not evals/:
# a bare `claude plugin eval .` finds no cases, because a run outside this
# script would point the plugin's hooks at the real 9527.
#
# ISOLATION. An eval run gets a temp HOME and only an allowlisted environment
# (LINGGEN_PORT is not on it), so neither the plugin's .mcp.json nor its hooks
# can be pointed away from 9527 by env. Instead:
#   - MCP: the memory and linggen servers are MOCKED (evals-isolated/mocks,
#     per-case mocks/). Real plugin servers are never started — never pass
#     --allow-real-servers or --mocks off: .mcp.json would then reach 9527.
#   - Hooks: each case's scaffold.sh writes the run's ~/.linggen/client.json
#     pointing at the scratch Linggen this script starts on 29548 (a `ling`
#     binary, LINGGEN_BIN), seeded with invented rows. The host is written
#     0.0.0.0, so autostart.sh treats it as another machine and installs/starts
#     nothing.
#   - Afterwards the real store is checked READ-ONLY: no row may carry an eval
#     session id or an eval temp path. Exit 3 if one does.
#
# The mocked server carries no MCP instructions, so each case appends the
# scratch engine's memory instructions (/mcp/memory) as a system prompt; this
# script re-syncs them before every run (a diff in prompt.md = the server text
# moved).

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN="$REPO/plugins/linggen"
SUITE="evals-isolated"
REAL_HOME="$HOME"
BIN="${LINGGEN_BIN:-$(command -v ling || true)}"
PORT=29548 # fixed: every case's scaffold.sh points the hooks here
REAL_PORT=9527

for tool in jq curl python3 claude; do
  command -v "$tool" >/dev/null || { echo "plugin-eval: needs $tool" >&2; exit 2; }
done
[ -x "$BIN" ] || { echo "plugin-eval: no ling binary (set LINGGEN_BIN)" >&2; exit 2; }
for a in "$@"; do
  case "$a" in --allow-real-servers|--mocks|--mocks=*)
    echo "plugin-eval: $a would start the plugin's real MCP servers (.mcp.json → $REAL_PORT); refused" >&2; exit 2 ;;
  esac
done

WORK="$REPO/target/plugin-eval"
FHOME="$WORK/home"
DATA="$WORK/data"
case "$WORK" in */target/plugin-eval) rm -rf "$WORK" ;; *) exit 2 ;; esac
mkdir -p "$FHOME/.cache" "$DATA"
[ -d "$REAL_HOME/.cache/huggingface" ] && ln -s "$REAL_HOME/.cache/huggingface" "$FHOME/.cache/huggingface"
touch "$DATA/no-telemetry"

# ── Scratch Linggen ──────────────────────────────────────────────────────────

DPID=""
KEPT=()
cleanup() {
  [ -n "$DPID" ] && kill "$DPID" 2>/dev/null
  wait 2>/dev/null
  [ "${PLUGIN_EVAL_KEEP:-0}" = 1 ] && return
  for d in "${KEPT[@]+"${KEPT[@]}"}"; do
    case "$d" in /private/tmp/e-*|/tmp/e-*) chmod -R u+rwx "$d" 2>/dev/null; rm -rf "$d" ;; esac
  done
}
trap cleanup EXIT INT TERM

if curl -s --max-time 1 "http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1; then
  echo "plugin-eval: port $PORT is taken (a stale scratch engine?)" >&2; exit 2
fi
# Everything the engine keeps lives under $DATA; it binds loopback only, never
# the old 127.0.0.1:9528, and announces nothing on the LAN.
(cd "$FHOME" && exec env HOME="$FHOME" LINGGEN_HOME="$DATA" LINGGEN_DATA_DIR="$DATA" LINGGEN_PORT="$PORT" \
  LINGGEN_LEGACY_MEMORY_PORT=0 LINGGEN_NO_BONJOUR=1 \
  "$BIN" --web --port "$PORT" --host 127.0.0.1) >"$WORK/daemon.log" 2>&1 &
DPID=$!
for _ in $(seq 1 180); do
  curl -sf --max-time 1 "http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1 && break
  kill -0 "$DPID" 2>/dev/null || break
  sleep 1
done
curl -sf --max-time 1 "http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1 \
  || { echo "plugin-eval: scratch engine did not come up; log: $WORK/daemon.log" >&2; tail -5 "$WORK/daemon.log" >&2; exit 1; }

# Invented rows only (a person called Alex). Global rows: an eval workspace is
# a temp dir, which sees rows about the person.
SEED='{"facts":[
 {"content":"Alex is a backend developer who lives in Lisbon.","tier":"core","type":"fact","from":"user"},
 {"content":"Alex'"'"'s staging server is called heron-02, and staging deploys only go out on Thursdays.","tier":"semantic","type":"fact","from":"user"},
 {"content":"Alex prefers short code reviews that lead with the riskiest change.","tier":"semantic","type":"preference","from":"user"},
 {"content":"The harbor ferry to Cacilhas leaves every twenty minutes from Cais do Sodre.","tier":"semantic","type":"fact","from":"derived"}
]}'
curl -sf -H 'content-type: application/json' -d "$SEED" "http://127.0.0.1:$PORT/mem/api/memory/add_batch" | jq -e '.ok' >/dev/null \
  || { echo "plugin-eval: seeding the scratch engine failed" >&2; exit 1; }

# ── Server instructions → each case's append_system_prompt ──────────────────

INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"plugin-eval","version":"1"}}}'
curl -sf -H 'content-type: application/json' -d "$INIT" "http://127.0.0.1:$PORT/mcp/memory" \
  | jq -r '.result.instructions // empty' >"$WORK/instructions.txt"
[ -s "$WORK/instructions.txt" ] || { echo "plugin-eval: no MCP instructions from the scratch engine" >&2; exit 1; }
python3 - "$WORK/instructions.txt" "$PLUGIN/$SUITE" <<'PY'
import pathlib, re, sys
text = open(sys.argv[1]).read().rstrip("\n")
body = "# MCP Server Instructions\n\n## plugin:linggen:memory\n" + text
block = "append_system_prompt: |\n" + "\n".join(("  " + l) if l else "" for l in body.split("\n")) + "\n"
for p in sorted(pathlib.Path(sys.argv[2]).glob("*/prompt.md")):
    s = p.read_text()
    new = re.sub(r"append_system_prompt: \|\n(?:(?:  .*)?\n)*?(?=---\n)", lambda m: block, s, count=1)
    if new != s:
        p.write_text(new)
        print(f"plugin-eval: synced server instructions into {p.parent.name}/prompt.md")
PY

# ── Real store, before (read-only) ──────────────────────────────────────────

real_rows() { # every row in the real store, all tiers, read-only
  { for q in '{"limit":100000}' '{"limit":100000,"episodic":true}' '{"limit":1000,"tier":"core"}'; do
      curl -sf --max-time 20 -H 'content-type: application/json' -d "$q" "http://127.0.0.1:$REAL_PORT/mem/api/memory/list" | jq -c '.data // []'
    done; } | jq -sc 'add | unique_by(.id)'
}
REAL_UP=0
curl -sf --max-time 2 "http://127.0.0.1:$REAL_PORT/api/health" >/dev/null 2>&1 && REAL_UP=1
[ "$REAL_UP" = 1 ] && real_rows >"$WORK/real-before.json"

# ── Run ─────────────────────────────────────────────────────────────────────

[ $# -eq 0 ] && set -- --runs 1 --ablation none --model haiku
echo "plugin-eval: scratch Linggen on $PORT (HOME=$FHOME); real store on $REAL_PORT checked read-only"
cd "$PLUGIN" || exit 2
# No tool grants: a Bash grant needs Claude Code's sandbox, which refuses to run
# on a Mac whose ~/.docker credential store holds a symlink — cases are built
# to need only Skill and the mocked MCP tools.
claude plugin eval . --eval-dir "$SUITE" --scaffold --trust-plugin --no-publish --keep-temp \
  "$@" 2>&1 | tee "$WORK/eval.log"
EVAL_RC=${PIPESTATUS[0]}

# ── Isolation proof ─────────────────────────────────────────────────────────

while IFS= read -r d; do KEPT+=("$d"); done < <(sed -n 's/^ *kept temp: //p' "$WORK/eval.log" | sort -u)
SESSIONS="$(for d in "${KEPT[@]+"${KEPT[@]}"}"; do
  jq -r '.session_id // empty' "$d/out/trace.jsonl" 2>/dev/null
done | sort -u | jq -Rsc 'split("\n") | map(select(length > 0))')"
echo
echo "isolation: $(printf '%s' "$SESSIONS" | jq length) eval session(s): $(printf '%s' "$SESSIONS" | jq -r 'join(" ")')"

if [ "$REAL_UP" = 1 ]; then
  real_rows >"$WORK/real-after.json"
  printf '%s' "$SESSIONS" >"$WORK/sessions.json"
  NB="$(jq length "$WORK/real-before.json")"; NA="$(jq length "$WORK/real-after.json")"
  NEW="$(jq -c --slurpfile b "$WORK/real-before.json" '($b[0] | map(.id)) as $ids | map(select(.id as $i | $ids | index($i) | not)) | map({id, host, created_at})' "$WORK/real-after.json")"
  LEAK="$(jq -c --slurpfile s "$WORK/sessions.json" '
    map(select((.source_session // "") as $x | ($s[0] | index($x)) != null
      or ([.scope, .cwd, .content] | map(. // "") | join(" ") | test("/private/tmp/e-|/tmp/e-")))) | map(.id)' "$WORK/real-after.json")"
  echo "isolation: real store $NB → $NA rows; new during the run: $(printf '%s' "$NEW" | jq length) $NEW"
  if [ "$(printf '%s' "$LEAK" | jq length)" != 0 ]; then
    echo "isolation: FAIL — real-store rows carry an eval session or temp path: $LEAK" >&2
    exit 3
  fi
  echo "isolation: PASS — no real-store row carries an eval session id or eval temp path"
else
  echo "isolation: real Linggen on $REAL_PORT not running — nothing to check"
fi
exit "$EVAL_RC"
