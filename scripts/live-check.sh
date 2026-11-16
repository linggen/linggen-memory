#!/usr/bin/env bash
# Live regression suite for Linggen's memory and its host plugins, against a
# scratch Linggen engine (`ling`, which hosts memory: /mem/* and /mcp/memory).
#
#   ./scripts/live-check.sh                 fixture store (CI-safe; the default)
#   ./scripts/live-check.sh --copy-live     fixture suite, then a copy of
#                                           ~/.linggen/memory (golden session
#                                           starts, real-data scope invariants,
#                                           v1 → v2 migration of the backups)
#   ./scripts/check.sh live [flags]         the same, from the check entry point
#
# Flags: --bin PATH (the `ling` under test; default $LINGGEN_BIN, else `ling` on PATH),
#        --port N (scratch port; default 29537, the copy uses N+1, a migration N+2),
#        --sync (plugin-check copies repo plugin files over drifted installs),
#        --update-golden (re-record the --copy-live snapshots),
#        --no-plugins (skip scripts/plugin-check.sh), --keep (leave the work dir).
#
# NEVER touches the real store or the real engine (9527, and the old 9528):
# every engine here is a scratch one, on a scratch port, with a scratch HOME and
# a scratch LINGGEN_HOME, loopback only, killed on exit. The live store is only
# ever copied (read). Everything is written under target/ — the fixture HOME
# lives there, not in a temp dir, because temp dirs can't hold rows.
#
# Each case prints PASS/FAIL/SKIP with a short reason; exit status is non-zero
# on any FAIL.

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REAL_HOME="$HOME"
BIN="${LINGGEN_BIN:-$(command -v ling 2>/dev/null || true)}"
PORT="${LIVE_CHECK_PORT:-29537}"
COPY_LIVE=0
SYNC=0
UPDATE_GOLDEN=0
PLUGINS=1
KEEP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --fixture) COPY_LIVE=0 ;;
    --copy-live) COPY_LIVE=1 ;;
    --bin) BIN="$2"; shift ;;
    --port) PORT="$2"; shift ;;
    --sync) SYNC=1 ;;
    --update-golden) UPDATE_GOLDEN=1 ;;
    --no-plugins) PLUGINS=0 ;;
    --keep) KEEP=1 ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "live-check: unknown flag $1" >&2; exit 2 ;;
  esac
  shift
done

for tool in jq curl git; do
  command -v "$tool" >/dev/null || { echo "live-check: needs $tool" >&2; exit 2; }
done
[ -n "$BIN" ] && [ -x "$BIN" ] || { echo "live-check: no ling binary (set LINGGEN_BIN or --bin)" >&2; exit 2; }
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"

WORK="$REPO/target/live-check"
FHOME="$WORK/home"
DATA="$WORK/data"
case "$WORK" in */target/live-check) rm -rf "$WORK" ;; *) exit 2 ;; esac
mkdir -p "$FHOME" "$DATA"

# ── Reporting ───────────────────────────────────────────────────────────────

N_PASS=0 N_FAIL=0 N_SKIP=0
RESULTS="$WORK/results.txt"
: >"$RESULTS"
report() { # status name reason
  printf '%-4s  %-52s %s\n' "$1" "$2" "${3:-}" | tee -a "$RESULTS"
  case "$1" in PASS) N_PASS=$((N_PASS + 1)) ;; FAIL) N_FAIL=$((N_FAIL + 1)) ;; SKIP) N_SKIP=$((N_SKIP + 1)) ;; esac
}
pass() { report PASS "$1" "${2:-}"; }
fail() { report FAIL "$1" "${2:-}"; }
skip() { report SKIP "$1" "${2:-}"; }
section() { printf '\n── %s\n' "$1"; }

# expect NAME JSON JQ-PREDICATE FAIL-REASON [PASS-NOTE]
expect() {
  if printf '%s' "$2" | jq -e "$3" >/dev/null 2>&1; then pass "$1" "${5:-}"
  else fail "$1" "$4 — got: $(printf '%s' "$2" | head -c 240 | tr '\n' ' ')"; fi
}

# ── Scratch engines ─────────────────────────────────────────────────────────

PIDS=()
cleanup() {
  for p in "${PIDS[@]+"${PIDS[@]}"}"; do kill "$p" 2>/dev/null; done
  wait 2>/dev/null
  [ "$KEEP" = 1 ] || rm -rf "$WORK/data" "$WORK/copy" "$WORK/mig"
}
trap cleanup EXIT INT TERM

port_free() { ! curl -s --max-time 1 "http://127.0.0.1:$1/api/health" >/dev/null 2>&1; }

# The scratch world one engine runs in: HOME and LINGGEN_HOME are scratch, the
# old loopback port and the LAN announcement are off.
scratch_env() { # HOME DATA PORT — prints nothing; sets ENV_ARGS
  ENV_ARGS=(env HOME="$1" LINGGEN_HOME="$2" LINGGEN_DATA_DIR="$2" LINGGEN_PORT="$3"
    LINGGEN_LEGACY_MEMORY_PORT=0 LINGGEN_NO_BONJOUR=1)
}

# start_daemon HOME DATA PORT — a background `ling --web`, wait for memory.
start_daemon() {
  local home="$1" data="$2" port="$3"
  case "$port" in 9527|9528) echo "live-check: refusing port $port" >&2; exit 2 ;; esac
  port_free "$port" || { echo "live-check: port $port is taken" >&2; exit 2; }
  case "$home" in "$REPO/target/"*) ;; *) echo "live-check: refusing HOME=$home (not under target/)" >&2; exit 2 ;; esac
  case "$data" in "$REPO/target/"*) ;; *) echo "live-check: refusing data dir $data (not under target/)" >&2; exit 2 ;; esac
  mkdir -p "$data" && touch "$data/no-telemetry"
  scratch_env "$home" "$data" "$port"
  (cd "$home" && exec "${ENV_ARGS[@]}" "$BIN" --web --port "$port" --host 127.0.0.1) >"$data.log" 2>&1 &
  PIDS+=($!)
  local i
  for i in $(seq 1 180); do
    curl -sf --max-time 1 "http://127.0.0.1:$port/mem/api/health" >/dev/null 2>&1 && return 0
    kill -0 "${PIDS[${#PIDS[@]}-1]}" 2>/dev/null || break
    sleep 1
  done
  echo "live-check: engine on $port did not come up; log: $data.log" >&2
  tail -5 "$data.log" >&2
  exit 1
}

# stop_daemon — the most recently started engine, and wait for it to go.
stop_daemon() {
  local p="${PIDS[${#PIDS[@]}-1]}" i
  kill "$p" 2>/dev/null
  for i in $(seq 1 20); do kill -0 "$p" 2>/dev/null || return 0; sleep 0.5; done
  kill -KILL "$p" 2>/dev/null
}

# api VERB JSON — memory's envelope; apid — its data.
api() { curl -sS --max-time 90 -H 'content-type: application/json' -d "$2" "http://127.0.0.1:$PORT/mem/api/memory/$1" 2>&1; }
apid() { api "$@" | jq -c '.data' 2>/dev/null; }
# mcp TOOL JSON — the JSON-RPC reply; mcpd — the tool's payload.
mcp() {
  local body
  body="$(jq -nc --arg n "$1" --argjson a "$2" '{jsonrpc:"2.0",id:1,method:"tools/call",params:{name:$n,arguments:$a}}')"
  curl -sS --max-time 90 -H 'content-type: application/json' -d "$body" "http://127.0.0.1:$PORT/mcp/memory" 2>&1
}
mcpd() { mcp "$@" | jq -c '.result.content[0].text | fromjson' 2>/dev/null; }
# cli ARGS… — `ling mem` against the scratch engine (it finds it by the store lock).
cli() { scratch_env "$FHOME" "$DATA" "$PORT"; "${ENV_ARGS[@]}" "$BIN" mem --data-dir "$DATA" "$@" 2>&1; }
# store_cli DATA ARGS… — `ling mem` on a store no engine holds (opens it directly).
store_cli() { local d="$1"; shift; scratch_env "$FHOME" "$d" 1; "${ENV_ARGS[@]}" "$BIN" mem --data-dir "$d" "$@" 2>/dev/null; }
# Rows out of a list/search/CLI reply, whatever the wrapper.
# The CLI prints one JSON object per row; the daemon one array.
rows() { jq -sc 'if length == 1 and (.[0] | type) == "array" then .[0] else [.[] | if type == "array" then .[] else (.data // .facts // .) end] | flatten end' 2>/dev/null; }

# ── Fixture home ────────────────────────────────────────────────────────────
#
#   ~/workspace                    parent of both projects
#   ~/workspace/lingjing           git root; story/ (README marker), story/ch1
#   ~/workspace/sanji              git root
#   ~/.linggen/skills/dj           a skill's own dir (app runtime memory)
#   ~/workspace/skills-dev/dj      the same app's dev dir

W="$FHOME/workspace"
L="$W/lingjing"
S="$W/sanji"
DJ="$FHOME/.linggen/skills/dj"
DJDEV="$W/skills-dev/dj"
mkdir -p "$L/story/ch1" "$S/src" "$DJ" "$DJDEV" "$FHOME/.local/bin" "$FHOME/.cache"
git init -q "$L" && git init -q "$S" && git init -q "$W/skills-dev"
echo "# story" >"$L/story/README.md"
# The embedding model: reuse this machine's hf cache (CI downloads it once).
if [ -d "$REAL_HOME/.cache/huggingface" ]; then
  ln -s "$REAL_HOME/.cache/huggingface" "$FHOME/.cache/huggingface"
fi

seed_rows() {
  jq -nc --arg L "$L" --arg S "$S" --arg W "$W" --arg DJ "$DJ" --arg DJDEV "$DJDEV" '
  def r($c; $tier; $type; $from; $scope): {content:$c, tier:$tier, type:$type, from:$from}
    + (if $scope == null then {} else {scope:$scope} end);
  {facts: ([
    (r("Alex lives in Lisbon and works as a novelist."; "core"; "fact"; "user"; null) + {_k:"core"}),
    (r("Alex prefers replies in plain English without jargon."; "semantic"; "preference"; "user"; null) + {_k:"person"}),
    (r("The Lingjing novel'"'"'s central artifact is a set of nine bronze cauldrons."; "semantic"; "decision"; "derived"; $L) + {_k:"l1"}),
    (r("Write every Lingjing chapter in the present tense, never in the past tense."; "semantic"; "preference"; "user"; $L) + {summary:"Chapters: present tense", indexed:true, _k:"l2"}),
    (r("Story chapters each end on a cliffhanger so the reader turns the page; the final line of a chapter is a question."; "semantic"; "preference"; "user"; ($L + "/story")) + {indexed:true, _k:"l3"}),
    (r("Commit straight to main and never open a branch in any workspace repo."; "semantic"; "preference"; "user"; $W) + {summary:"Commit to main, never branch", indexed:true, _k:"w1"}),
    (r("Sanji draws its dashboard cauldron chart with a canvas renderer."; "semantic"; "decision"; "derived"; $S) + {_k:"s1"}),
    (r("Sanji code always uses tabs for indentation."; "semantic"; "preference"; "user"; $S) + {indexed:true, summary:"Tabs, not spaces", _k:"s2"}),
    (r("The DJ app pairs song lyrics by clock time using LRCLIB copies."; "semantic"; "fact"; "derived"; $DJ) + {_k:"d1"}),
    (r("Building the DJ app: the lyrics pairing code lives in pair.js and is tested with fixture songs."; "semantic"; "fact"; "derived"; $DJDEV) + {_k:"d2"}),
    (r("The harbor ferry to Cacilhas leaves every twenty minutes from Cais do Sodre."; "semantic"; "fact"; "derived"; $L) + {_k:"n1"}),
    (r("The harbor ferry to Cacilhas leaves every 20 minutes from Cais do Sodre station."; "semantic"; "fact"; "derived"; $S) + {_k:"n2"}),
    (r("Harbor ferry to Cacilhas: it leaves every twenty minutes from Cais do Sodre."; "semantic"; "decision"; "derived"; $W) + {_k:"n3"}),
    (r("The harbor ferry to Cacilhas leaves every twenty minutes from Cais do Sodre, always take it."; "semantic"; "preference"; "user"; $S) + {_k:"n4"}),
    (r("Old episodic note from a long-past day about the ferry timetable."; "episodic"; "fact"; "derived"; $L) + {episodic:true, occurred_at:"2025-01-01T00:00:00Z", _k:"e1"}),
    (r("Fresh episodic note from today about the lingjing outline."; "episodic"; "fact"; "derived"; $L) + {episodic:true, _k:"e2"})
  ] + [range(0; 45) as $i |
    r("Chapter one rule number \($i): keep the scene tight and the dialogue short, never more than one beat."; "semantic"; "preference"; "user"; ($L + "/story/ch1"))
    + {indexed:true, summary:("Chapter one scene rule \($i) — tight scenes, short dialogue, one beat each"), _k:"ch1"}])}'
}

# ── Fixture suite ───────────────────────────────────────────────────────────

section "Fixture engine (port $PORT, HOME=$FHOME, $BIN)"
start_daemon "$FHOME" "$DATA" "$PORT"

H="$(curl -s "http://127.0.0.1:$PORT/mem/api/health")"
expect "health" "$H" '.ok == true and .data.status == "healthy"' "unhealthy" "$(printf '%s' "$H" | jq -r '.data.version')"
SV="$(cat "$DATA/memory/SCHEMA_VERSION" 2>/dev/null | tr -d '[:space:]')"
[ "$SV" = 2 ] && pass "schema sidecar reads 2" || fail "schema sidecar reads 2" "SCHEMA_VERSION=$SV"

# Seed — the batch path stores `scope` as given. Keys map seeds to ids.
SEED="$(seed_rows)"
SEEDED="$(api add_batch "$(printf '%s' "$SEED" | jq -c '{facts: [.facts[] | del(._k)]}')")"
if ! printf '%s' "$SEEDED" | jq -e '.ok' >/dev/null 2>&1; then
  fail "seed fixture rows" "$(printf '%s' "$SEEDED" | head -c 200)"
  exit 1
fi
ALL="$( { apid list '{"limit":500}'; apid list '{"limit":500,"episodic":true}'; apid list '{"limit":10,"tier":"core"}'; } | jq -sc 'add | unique_by(.id)')"
idof() { printf '%s' "$ALL" | jq -r --arg c "$1" 'map(select(.content == $c)) | .[0].id // empty'; }
seed_content() { printf '%s' "$SEED" | jq -r --arg k "$1" '.facts[] | select(._k == $k) | .content' | head -1; }
for k in core person l1 l2 l3 w1 s1 s2 d1 d2 n1 n2 n3 n4 e1 e2; do
  printf -v "ID_$k" '%s' "$(idof "$(seed_content "$k")")"
done
NSEEDED="$(printf '%s' "$ALL" | jq 'length')"
[ -n "$ID_l3" ] && [ -n "$ID_d1" ] && [ -n "$ID_e1" ] && [ "$NSEEDED" -ge 61 ] \
  && pass "seed fixture rows" "$NSEEDED rows" || fail "seed fixture rows" "only $NSEEDED rows / ids missing"
SEED_SCOPED="$(printf '%s' "$ALL" | jq --arg id "$ID_l3" --arg L "$L" 'map(select(.id == $id))[0] | .scope == ($L + "/story") and .indexed == true')"
[ "$SEED_SCOPED" = true ] && pass "batch import keeps scope + indexed" || fail "batch import keeps scope + indexed" "l3 lost scope/indexed"

section "session_start"
SS="$(apid session_start "$(jq -nc --arg c "$FHOME" '{cwd:$c}')")"
expect "session_start \$HOME: core only" "$SS" \
  '(.core | length) >= 1 and (.index | length) == 0 and (.candidates | length) == 0 and (.block | test("## Core memory")) and ((.block | test("## Index|Memory scopes here")) | not)' \
  "expected core alone"

SS="$(apid session_start "$(jq -nc --arg c "$L/story" --arg r "$L" '{cwd:$c, root:$r}')")"
expect "session_start project: candidates line" "$SS" \
  '(.candidates_line | test("^Memory scopes here: ")) and ([.candidates[].path] | index("'"$W"'") != null) and ([.candidates[].path] | index("'"$L/story"'") != null) and ([.candidates[].path] | index("'"$S"'") == null)' \
  "candidates should hold lingjing, story, workspace — never sanji"
ORDER="$(printf '%s' "$SS" | jq -c --arg a "$ID_l3" --arg b "$ID_l2" --arg c "$ID_w1" '[.index[].id] | [index($a), index($b), index($c)]')"
expect "session_start project: index nearest first" "$ORDER" '.[0] != null and .[1] != null and .[2] != null and .[0] < .[1] and .[1] < .[2]' "story < lingjing < workspace"
HEADS="$(printf '%s' "$SS" | jq -r '.index_block' | grep -c '^## Index — ')"
[ "$HEADS" = 3 ] && pass "session_start project: one heading per dir" || fail "session_start project: one heading per dir" "$HEADS headings"
expect "session_start: sibling project's index absent" "$SS" '[.index[].id] | index("'"$ID_s2"'") == null' "sanji's indexed row leaked"
expect "session_start: unsummarised row shows its opening" "$SS" \
  '.index_block | test("- Story chapters each end on a cliffhanger.*\\(id='"$ID_l3"'\\)")' "no opening line for l3"
expect "session_start: summary shown for summarised row" "$SS" '.index_block | test("- Chapters: present tense \\(id='"$ID_l2"'\\)")' "l2 summary missing"

SS="$(apid session_start "$(jq -nc --arg c "$L/story/ch1" --arg r "$L" '{cwd:$c, root:$r}')")"
expect "session_start: 3000-char budget, skipped counted" "$SS" \
  '(.index_block | length) <= 3000 and .index_skipped > 0 and (.index_block | startswith("## Index — "))' \
  "index over budget or nothing skipped ($(printf '%s' "$SS" | jq -c '{chars: (.index_block | length), skipped: .index_skipped, shown: (.index | length)}'))"
expect "session_start: budget fills nearest dir first" "$SS" \
  '(.index_block | split("\n") | map(select(startswith("## Index — "))) | .[0] | test("ch1"))' "first heading is not ch1"

SS="$(apid session_start "$(jq -nc --arg c "$L/story" '{cwd:$c}')")"
expect "session_start: root found from .git when unsent" "$SS" '[.index[].id] | index("'"$ID_l2"'") != null' "no root → lingjing index missing"
SS="$(apid session_start "$(jq -nc --arg c "$DJ" '{cwd:$c, core:false}')")"
expect "session_start skill: own dir only, core:false" "$SS" '(.core | length) == 0 and ([.index[].scope] | all(startswith("'"$DJ"'")))' "skill session saw outside rows"

section "Scope on add / update"
add_at() { # content cwd root [extra-json]
  local x="${4:-}"; [ -n "$x" ] || x='{}'
  apid add "$(jq -nc --arg c "$1" --arg cwd "$2" --arg r "$3" --argjson x "$x" '{content:$c, tier:"semantic", type:"decision", cwd:$cwd, root:$r} + $x')"
}
A="$(add_at "Scope probe one: the default lands on the session cwd." "$L/story" "$L")"
expect "add: default scope = cwd" "$A" '.fact.scope == "'"$L/story"'"' "scope not cwd"
A="$(add_at "Scope probe two: a candidate with a tilde is expanded." "$L" "$L" '{"scope":"~/workspace"}')"
expect "add: candidate ~/ scope expanded" "$A" '.fact.scope == "'"$W"'"' "~/workspace not expanded/accepted"
A="$(add_at "Scope probe three: a candidate shown relative to root's parent." "$L" "$L" '{"scope":"lingjing/story"}')"
expect "add: relative candidate resolves" "$A" '.fact.scope == "'"$L/story"'"' "lingjing/story not resolved"
A="$(add_at "Scope probe four: a sibling project is not a candidate here." "$L" "$L" '{"scope":"~/workspace/sanji"}')"
expect "add: non-candidate falls back to cwd" "$A" '.fact.scope == "'"$L"'"' "sibling scope accepted"
A="$(add_at "Scope probe five: a missing directory falls back too." "$L/story" "$L" '{"scope":"~/workspace/lingjing/nope"}')"
expect "add: non-existent dir falls back to cwd" "$A" '.fact.scope == "'"$L/story"'"' "missing dir accepted"
A="$(add_at "Scope probe six: global rows are about the person." "$L" "$L" '{"global":true}')"
expect "add: global:true → null scope" "$A" '.fact.scope == null' "global row got a scope"
A="$(add_at "Alex's second language is Portuguese." "$L" "$L" '{"tier":"core","type":"fact","from":"user"}')"
expect "add: core never gets a scope" "$A" '.fact.tier == "core" and .fact.scope == null' "core row scoped"
CORE_ID="$(printf '%s' "$A" | jq -r '.fact.id')"
U="$(apid update "$(jq -nc --arg id "$CORE_ID" --arg s "$L" '{id:$id, scope:$s, indexed:true}')")"
expect "update: core refuses scope + index" "$U" '.scope == null and (.indexed // false) == false' "core row took scope/index on update"
A="$(apid add "$(jq -nc '{content:"Scope probe seven: an unstamped host names a directory.", tier:"semantic", scope:"~/workspace/lingjing"}')")"
expect "add: unstamped host keeps a named dir" "$A" '.fact.scope == "'"$L"'"' "named scope dropped without host stamp"
PROBE="$(printf '%s' "$A" | jq -r '.fact.id')"
U="$(apid update "$(jq -nc --arg id "$PROBE" '{id:$id, scope:"~/workspace/sanji", summary:"moved", indexed:true}')")"
expect "update: scope ~/ expanded, summary + indexed" "$U" '.scope == "'"$S"'" and .summary == "moved" and .indexed == true' "update did not move/flag"
U="$(apid update "$(jq -nc --arg id "$PROBE" '{id:$id, global:true}')")"
expect "update: global clears scope" "$U" '.scope == null' "global did not clear"
U="$(api update "$(jq -nc --arg id "$PROBE" '{id:$id, scope:"relative/dir"}')")"
expect "update: relative scope refused" "$U" '.ok == false' "relative scope accepted"

section "Recall scope"
search() { # query [extra-json]
  local x="${2:-}"; [ -n "$x" ] || x='{}'
  apid search "$(jq -nc --arg q "$1" --argjson x "$x" '{query:$q, limit:10} + $x')"
}
# No row outside the root's subtree, its ancestors, or the person.
R="$(search "the nine bronze cauldrons artifact and the cauldron chart" "$(jq -nc --arg r "$L" '{scope_root:$r}')")"
expect "recall scope_root: subtree + ancestors + null" "$R" \
  '(map(.id) | index("'"$ID_l1"'") != null) and all(.[]; .scope == null or (.scope | startswith("'"$L"'")) or .scope == "'"$W"'")' \
  "a row outside lingjing/its parents, or l1 missing"
expect "recall: no cross-project leak (lingjing ↛ sanji)" "$R" 'map(.id) | index("'"$ID_s1"'") == null' "sanji's cauldron chart leaked into lingjing"
R2="$(search "the nine bronze cauldrons artifact and the cauldron chart" "$(jq -nc --arg r "$L" '{cwd_scope:$r}')")"
[ "$(printf '%s' "$R" | jq -c 'map(.id)|sort')" = "$(printf '%s' "$R2" | jq -c 'map(.id)|sort')" ] \
  && pass "recall: cwd_scope alias = scope_root" || fail "recall: cwd_scope alias = scope_root" "alias returned a different set"
R="$(search "the cauldron chart canvas renderer" "$(jq -nc --arg r "$S" '{scope_root:$r}')")"
expect "recall: no cross-project leak (sanji ↛ lingjing)" "$R" \
  '(map(.id) | index("'"$ID_s1"'") != null) and (map(.id) | index("'"$ID_l1"'") == null)' "lingjing leaked into sanji or s1 missing"

FERRY="The harbor ferry to Cacilhas leaves every twenty minutes from Cais do Sodre."
R="$(search "$FERRY" "$(jq -nc --arg r "$FHOME" '{scope_root:$r}')")"
expect "no-root: ≤ 2 project rows" "$R" '[.[] | select(.scope != null)] | length >= 1 and length <= 2' "scoped rows outside 1..2"
expect "no-root: project rows reach cosine ≥ 0.70" "$R" '[.[] | select(.scope != null) | .score] | all(. >= 0.70)' "a weak project row joined"
expect "no-root: scoped preferences excluded" "$R" '[.[] | select(.scope != null and .type == "preference")] | length == 0' "a scoped preference joined"
R="$(search "hey, how is your day going so far?" "$(jq -nc --arg r "$FHOME" '{scope_root:$r}')")"
expect "no-root: small talk brings no project rows" "$R" '[.[] | select(.scope != null)] | length == 0' "small talk pulled project rows"
R="$(search "Alex prefers plain English replies" "$(jq -nc --arg r "$FHOME/.linggen" '{scope_root:$r}')")"
expect "no-root: ~/.linggen sees person rows" "$R" 'map(.id) | index("'"$ID_person"'") != null' "person row missing at ~/.linggen"

R="$(search "song lyrics paired by clock time from LRCLIB; Alex prefers plain English" "$(jq -nc --arg r "$DJ" '{scope_root:$r}')")"
expect "skill session: own dir only, no person rows" "$R" \
  '(map(.id) | index("'"$ID_d1"'") != null) and all(.[]; (.scope // "") | startswith("'"$DJ"'"))' "skill saw rows outside its dir"
R="$(search "DJ lyrics pairing by clock time with LRCLIB" "$(jq -nc --arg r "$W/skills-dev" '{scope_root:$r}')")"
expect "app vs dev: dev session never sees runtime rows" "$R" \
  '(map(.id) | index("'"$ID_d1"'") == null) and (map(.id) | index("'"$ID_d2"'") != null)' "runtime row in dev recall, or dev row missing"
R="$(search "lyrics pairing code in pair.js with fixture songs" "$(jq -nc --arg r "$DJ" '{scope_root:$r}')")"
expect "app vs dev: skill session never sees dev rows" "$R" 'map(.id) | index("'"$ID_d2"'") == null' "dev row in skill recall"

section "Today's bug fixes"
A="$(apid add "$(jq -nc --arg c "$L" '{content:"Bugfix probe: a write with no tier is an episodic capture.", cwd:$c, root:$c}')")"
expect "fix 1: omitted tier → episodic" "$A" '.fact.tier == "episodic"' "omitted tier did not land episodic"
A="$(mcpd memory_add "$(jq -nc --arg c "$L" '{content:"Bugfix probe: Alex said the ferry is the best part of the week.", tier:"semantic", type:"fact", from:"user", cwd:$c, root:$c}')")"
expect "fix 2: memory_add accepts from" "$A" '.fact.from == "user"' "from not stored"
LOSER="$(apid add "$(jq -nc --arg c "$L" '{content:"Bugfix probe: the outline has seven acts (draft).", tier:"semantic", type:"decision", cwd:$c, root:$c}')" | jq -r '.fact.id')"
A="$(mcpd memory_add "$(jq -nc --arg c "$S" --arg id "$LOSER" '{content:"Bugfix probe: the outline settled on nine acts.", type:"decision", replace_ids:[$id], cwd:$c, root:$c}')")"
expect "fix 5: replace_ids keeps the loser's tier" "$A" '.fact.tier == "semantic"' "replacement tier is not the loser's"
expect "fix 4: replacement takes the losers' scope" "$A" '.fact.scope == "'"$L"'"' "replacement rescoped to the writer's cwd"
WINNER="$(printf '%s' "$A" | jq -r '.fact.id')"
MCONTENT="Bugfix probe: the merge keeps the surviving row's scope, not the writer's."
apid add "$(jq -nc --arg c "$L" --arg t "$MCONTENT" '{content:$t, tier:"semantic", type:"decision", cwd:$c, root:$c}')" >/dev/null
A="$(apid add "$(jq -nc --arg c "$S" --arg t "$MCONTENT" '{content:$t, tier:"semantic", type:"decision", cwd:$c, root:$c}')")"
expect "fix 4: dedup merge keeps scope" "$A" '.action == "merged" and .fact.scope == "'"$L"'"' "merge moved the row"
# The 2026-10-05 loss: a replacement whose words equal its loser's merged
# into the loser, then retired it — memory_get found nothing. Now the new row
# lands, the loser retires into it, and exactly one live row says the words.
RCONTENT="Bugfix probe: a replacement equal to its loser keeps one live row."
RLOSER="$(apid add "$(jq -nc --arg c "$L" --arg t "$RCONTENT" '{content:$t, tier:"semantic", type:"decision", cwd:$c, root:$c}')" | jq -r '.fact.id')"
A="$(mcpd memory_add "$(jq -nc --arg c "$L" --arg t "$RCONTENT" --arg id "$RLOSER" '{content:$t, type:"decision", replace_ids:[$id], cwd:$c, root:$c}')")"
RNEW="$(printf '%s' "$A" | jq -r '.fact.id')"
G="$(mcpd memory_get "$(jq -nc --arg id "$RNEW" '{id:$id}')")"
N="$(apid list '{"limit":500}' | jq --arg t "$RCONTENT" 'map(select(.content == $t)) | length')"
if printf '%s' "$G" | jq -e --arg t "$RCONTENT" '.content == $t' >/dev/null 2>&1 && [ "$N" = 1 ] && [ "$RNEW" != "$RLOSER" ]; then
  pass "replace into an equal row: one live row remains"
else
  fail "replace into an equal row: one live row remains" "add=$(printf '%s' "$A" | head -c 160) get=$(printf '%s' "$G" | head -c 80) live=$N"
fi
A="$(apid add "$(jq -nc --arg c "$L" --arg id "$RNEW" --arg s "$L/story" '{content:"Bugfix probe: a named scope beats the loser'"'"'s.", type:"decision", replace_ids:[$id], scope:$s, cwd:$c, root:$c}')")"
expect "replacement: a named scope wins over the loser's" "$A" '.fact.scope == "'"$L/story"'" and .fact.tier == "semantic"' "named scope lost"
R="$(apid list '{"episodic":true,"past_ttl":true,"limit":200}')"
expect "fix 6: one TTL clock (occurred_at first)" "$R" \
  '(map(.id) | index("'"$ID_e1"'") != null) and (map(.id) | index("'"$ID_e2"'") == null)' "past_ttl ignores occurred_at"

# CLI filters forwarded to the daemon.
SID="live-check-session-$$"
for t in fact decision; do
  apid add "$(jq -nc --arg c "$L" --arg s "$SID" --arg t "$t" '{content:("CLI probe row of type " + $t + " for source session filters."), tier:"semantic", type:$t, source_session:$s, cwd:$c, root:$c}')" >/dev/null
done
C="$(cli list --source-session "$SID" --limit 50 | rows)"
expect "fix 7: CLI --source-session forwarded" "$C" 'length == 2 and all(.[]; .source_session == "'"$SID"'")' "source_session filter dropped"
C="$(cli list --type preference --type decision --limit 200 | rows)"
expect "fix 7: CLI --type is multi" "$C" '(map(.type) | unique) as $t | ($t | index("preference") != null) and ($t | index("decision") != null) and ($t | all(. == "preference" or . == "decision"))' "multiple --type not forwarded"
C="$(cli list --superseded-by "$WINNER" | rows)"
expect "fix 7: CLI --superseded-by forwarded" "$C" 'length == 1 and .[0].id == "'"$LOSER"'"' "unpack query lost"
C1="$(cli list --include-expired --limit 500 | rows | jq 'map(.id) | index("'"$LOSER"'") != null')"
C2="$(cli list --limit 500 | rows | jq 'map(.id) | index("'"$LOSER"'") != null')"
[ "$C1" = true ] && [ "$C2" = false ] && pass "fix 7: CLI --include-expired forwarded" || fail "fix 7: CLI --include-expired forwarded" "with=$C1 without=$C2"

section "MCP"
TL="$(curl -sS -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' "http://127.0.0.1:$PORT/mcp/memory")"
expect "tools/list: memory_add has scope/summary/indexed/from" "$TL" \
  '.result.tools[] | select(.name == "memory_add") | .inputSchema.properties | has("scope") and has("summary") and has("indexed") and has("from") and has("global")' "missing fields"
expect "tools/list: host-filled fields marked" "$TL" \
  '[.result.tools[] | select(.name == "memory_add") | .inputSchema.properties | .cwd.description, .root.description, .source_session.description, .host.description] | map(select(. != null)) | all(test("HOST-FILLED"))' \
  "cwd/root/source_session/host not marked HOST-FILLED"
expect "tools/list: search takes scope_root (host-filled)" "$TL" \
  '.result.tools[] | select(.name == "memory_search") | .inputSchema.properties | has("scope_root") and (.scope_root.description | test("HOST-FILLED")) and (has("cwd_scope") | not)' \
  "scope_root missing or cwd_scope advertised"
expect "tools/list: no retired fields" "$TL" \
  '[.result.tools[].inputSchema.properties | keys[]] | (index("contexts") == null and index("tags") == null and index("hook") == null and index("contexts_any") == null)' \
  "contexts/tags/hook still advertised"

# The 2026-10-04 bug: string-typed booleans from a CC session.
BUG='{"content":"MCP probe: shared sessions replace guest mode (scratch).","tier":"semantic","type":"decision","scope":"~/workspace/lingjing","summary":"Shared sessions replace guest mode","indexed":"false"}'
R="$(mcp memory_add "$(printf '%s' "$BUG" | jq -c --arg c "$L/story" --arg r "$L" '. + {cwd:$c, root:$r}')")"
expect "MCP add: CC's string-typed indexed:\"false\"" "$R" \
  '.result.content[0].text | fromjson | .fact.scope == "'"$L"'" and .fact.summary == "Shared sessions replace guest mode" and ((.fact.indexed // false) == false)' \
  "the known bug"
STAMPED="$(jq -nc --argjson a "$(printf '%s' "$BUG" | jq -c '.content = "MCP probe: the same call through the CC stamp hook." | .indexed = false')" --arg c "$L/story" \
  '{tool_name:"mcp__plugin_linggen_memory__memory_add", session_id:"lc-sess", cwd:$c, tool_input:$a}' \
  | HOME="$FHOME" CLAUDE_PROJECT_DIR="$L" bash "$REPO/plugins/linggen/hooks/stamp-cwd.sh" | jq -c '.hookSpecificOutput.updatedInput')"
R="$(mcpd memory_add "$STAMPED")"
expect "MCP add: through stamp-cwd.sh (scope ~, summary, indexed:false)" "$R" \
  '.fact.scope == "'"$L"'" and .fact.source_session == "lc-sess" and .fact.host == "claude-code" and .fact.summary == "Shared sessions replace guest mode"' \
  "stamped add lost fields"
MID="$(printf '%s' "$R" | jq -r '.fact.id')"
# The 2026-10-05 bug: a session started in the non-git workspace, shell cd'd
# into one of its repos. Through the real stamp hook, the model's scope must
# land where it names — relative candidate, absolute dir under the session
# root — and only no scope or a dir outside root falls back to cwd.
via_hook() { # content cwd extra-json → stored scope
  jq -nc --arg t "$1" --arg c "$2" --argjson x "$3" \
    '{tool_name:"mcp__plugin_linggen_memory__memory_add", session_id:"lc-nest", cwd:$c, tool_input:({content:$t, skip_dedup:true} + $x)}' \
    | HOME="$FHOME" CLAUDE_PROJECT_DIR="$W" bash "$REPO/plugins/linggen/hooks/stamp-cwd.sh" \
    | jq -c '.hookSpecificOutput.updatedInput' | { read -r a; mcpd memory_add "$a"; } | jq -r '.fact.scope'
}
got="$(via_hook "Nested probe one: a relative candidate from a sibling repo." "$S/src" '{"scope":"workspace/lingjing/story"}')"
[ "$got" = "$L/story" ] && pass "hook add from nested repo: relative candidate kept" || fail "hook add from nested repo: relative candidate kept" "stored $got"
got="$(via_hook "Nested probe two: an absolute dir under the session root." "$L" "$(jq -nc --arg s "$S/src" '{scope:$s}')")"
[ "$got" = "$S/src" ] && pass "hook add from nested repo: absolute scope kept" || fail "hook add from nested repo: absolute scope kept" "stored $got"
got="$(via_hook "Nested probe three: no scope lands on the shell cwd." "$S/src" '{}')"
[ "$got" = "$S/src" ] && pass "hook add from nested repo: no scope → cwd" || fail "hook add from nested repo: no scope → cwd" "stored $got"
got="$(via_hook "Nested probe four: a dir outside the root falls back." "$L" '{"scope":"/etc"}')"
[ "$got" = "$L" ] && pass "hook add from nested repo: outside root → cwd" || fail "hook add from nested repo: outside root → cwd" "stored $got"

R="$(mcpd memory_add '{"content":"MCP probe: an unadvertised flag sent as a string.","skip_dedup":"true"}')"
expect "MCP add: unadvertised skip_dedup:\"true\" is a boolean" "$R" '.action == "added"' "string boolean refused"
R="$(mcp memory_add '{"content":"MCP probe: a field of the wrong type is named.","indexed":"perhaps"}')"
expect "MCP add: bad type reports the field, not a decode error" "$R" \
  '.error.message | (test("indexed") and (test("error decoding response body") | not))' "opaque error"
R="$(mcpd memory_get "$(jq -nc --arg id "$MID" '{id:$id}')")"
expect "MCP get: round-trip" "$R" '.id == "'"$MID"'" and .scope == "'"$L"'"' "get lost the row"
R="$(mcpd memory_update "$(jq -nc --arg id "$MID" '{id:$id, summary:"Shared sessions (updated)", indexed:"true", scope:"~/workspace"}')")"
expect "MCP update: summary/indexed/scope (string bool)" "$R" '.summary == "Shared sessions (updated)" and .indexed == true and .scope == "'"$W"'"' "update failed"
R="$(mcpd memory_search "$(jq -nc --arg r "$L" '{query:"MCP probe: the same call through the CC stamp hook.", scope_root:$r, limit:"5"}')")"
expect "MCP search: scope_root + string limit" "$R" 'type == "array" and (map(.id) | index("'"$MID"'") != null)' "search missed the row"
R="$(mcpd memory_session_start "$(jq -nc --arg c "$L" --arg r "$L" '{cwd:$c, root:$r}')")"
expect "MCP session_start: index carries the updated row" "$R" '.index_block | test("Shared sessions \\(updated\\)")' "index missing the row"
R="$(mcpd memory_list "$(jq -nc '{indexed:"true", limit:"200"}')")"
expect "MCP list: indexed filter (string bool)" "$R" 'type == "array" and length > 0 and all(.[]; .indexed == true)' "indexed filter ignored"

section "CLI parity"
cd "$L/story" || exit 1
C="$(cli add "CLI parity probe: the CLI writes a scope and a summary." --tier semantic --type decision --cwd "$L/story" --root "$L" --scope "~/workspace/lingjing" --summary "CLI parity" --indexed)"
expect "CLI add --scope ~/ --summary --indexed" "$C" '(.fact // .) | .scope == "'"$L"'" and .summary == "CLI parity" and .indexed == true' "CLI add lost fields"
CID="$(printf '%s' "$C" | jq -r '(.fact // .).id')"
C="$(cli list --scope-root "$L" --indexed true --limit 200 | rows)"
expect "CLI list --scope-root --indexed" "$C" '(map(.id) | index("'"$CID"'") != null) and all(.[]; .indexed == true)' "list filter"
C="$(cli search "the CLI writes a scope and a summary" --scope-root "$S" --limit 10 | rows)"
expect "CLI search --scope-root excludes other project" "$C" 'map(.id) | index("'"$CID"'") == null' "lingjing row in sanji search"
C="$(cli search "the CLI writes a scope and a summary" --scope-root "$L" --limit 10 | rows)"
expect "CLI search --scope-root finds own row" "$C" 'map(.id) | index("'"$CID"'") != null' "row missing"
C="$(cli update "$CID" --scope "~/workspace" --summary "CLI parity moved")"
expect "CLI update --scope --summary" "$C" '(.fact // .) | .scope == "'"$W"'" and .summary == "CLI parity moved"' "CLI update"
C="$(cli session-start --cwd "$L/story" --root "$L" --format json)"
expect "CLI session-start --format json" "$C" '(.block // .data.block) | test("Memory scopes here")' "CLI session-start"
cd "$REPO" || exit 1

# ── Plugins ─────────────────────────────────────────────────────────────────

if [ "$PLUGINS" = 1 ]; then
  section "Plugins (scripts/plugin-check.sh)"
  PC_ARGS=(--port "$PORT" --home "$FHOME" --project "$L" --skill "$DJ")
  [ "$SYNC" = 1 ] && PC_ARGS+=(--sync)
  while IFS= read -r line; do
    printf '%s\n' "$line"
    case "$line" in
      PASS\ *) N_PASS=$((N_PASS + 1)); printf '%s\n' "$line" >>"$RESULTS" ;;
      FAIL\ *) N_FAIL=$((N_FAIL + 1)); printf '%s\n' "$line" >>"$RESULTS" ;;
      SKIP\ *) N_SKIP=$((N_SKIP + 1)); printf '%s\n' "$line" >>"$RESULTS" ;;
    esac
  done < <(bash "$REPO/scripts/plugin-check.sh" "${PC_ARGS[@]}" 2>&1)
fi

# ── Copy of the live store ──────────────────────────────────────────────────

migrate_check() { # name backup-dir
  local name="$1" src="$2" mig="$WORK/mig/$1" before after a2 a3 again
  mkdir -p "$mig/memory"
  cp -R "$src/memory.lancedb" "$mig/memory/" && cp "$src/SCHEMA_VERSION" "$mig/memory/"
  local dump='def keep: {id, content, type, tier, from, outcome, scope, summary, indexed, created_at, updated_at, occurred_at, source_session, host, superseded_by, expired_at, account_id};'
  # `export` opens the store itself, so it runs while no engine holds it.
  dump_rows() { { store_cli "$mig" export -; store_cli "$mig" --episodic export -; } | jq -sc "$dump"' map(keep) | sort_by(.id)'; }
  local saved_port="$PORT"
  PORT=$((saved_port + 2))
  before="$(dump_rows)"
  start_daemon "$FHOME" "$mig" "$PORT"
  a2="$(curl -sS -H 'content-type: application/json' -d '{"confirm":true}' "http://127.0.0.1:$PORT/mem/api/schema/apply")"
  a3="$(curl -sS -H 'content-type: application/json' -d '{"confirm":true}' "http://127.0.0.1:$PORT/mem/api/schema/apply")"
  stop_daemon
  after="$(dump_rows)"
  local sv; sv="$(tr -d '[:space:]' <"$mig/memory/SCHEMA_VERSION")"
  local n; n="$(printf '%s' "$before" | jq length)"
  if [ "$sv" = 2 ] && [ "$n" -gt 0 ] && [ "$before" = "$after" ] && printf '%s' "$a2" | jq -e '.ok' >/dev/null 2>&1; then
    pass "migration $name → v2 keeps every value" "$n rows"
  else
    fail "migration $name → v2 keeps every value" "sidecar=$sv rows=$n same=$([ "$before" = "$after" ] && echo y || echo n) apply=$(printf '%s' "$a2" | head -c 160)"
  fi
  again="$after"
  if printf '%s' "$a3" | jq -e '.ok' >/dev/null 2>&1 && [ "$again" = "$after" ] && [ "$sv" = 2 ]; then
    pass "migration $name: idempotent" "$(printf '%s' "$a3" | jq -c '.data' | head -c 80)"
  else
    fail "migration $name: idempotent" "second apply: $(printf '%s' "$a3" | head -c 160)"
  fi
  PORT="$saved_port"
}

if [ "$COPY_LIVE" = 1 ]; then
  # The engine over the copy runs in the scratch HOME like every other here;
  # the real directories below are only paths handed to it.
  section "Copy of the live store (port $((PORT + 1)), HOME=$FHOME)"
  LIVE="$REAL_HOME/.linggen/memory"
  CDATA="$WORK/copy"
  mkdir -p "$CDATA/memory"
  cp -R "$LIVE/memory.lancedb" "$CDATA/memory/" && cp "$LIVE/SCHEMA_VERSION" "$CDATA/memory/"
  FPORT="$PORT"
  PORT=$((FPORT + 1))
  start_daemon "$FHOME" "$CDATA" "$PORT"
  CSV="$(tr -d '[:space:]' <"$CDATA/memory/SCHEMA_VERSION")"
  [ "$CSV" = 2 ] && pass "live copy: schema sidecar reads 2" || fail "live copy: schema sidecar reads 2" "SCHEMA_VERSION=$CSV"

  SS="$(apid session_start "$(jq -nc --arg c "$REAL_HOME" '{cwd:$c}')")"
  expect "live copy: session_start \$HOME is core only" "$SS" '(.core | length) >= 1 and (.index | length) == 0 and ((.block | test("## Index|Memory scopes here")) | not)' "index/candidates at \$HOME"

  GOLDEN="${LIVE_CHECK_GOLDEN:-$REPO/target/live-check-golden}"
  mkdir -p "$GOLDEN"
  for dir in "$REAL_HOME/workspace/linggen/linggen-memory" "$REAL_HOME/workspace/linggen/skills/lingjing" "$REAL_HOME/workspace/linggen/linggen"; do
    [ -d "$dir" ] || { skip "live copy: golden $(basename "$dir")" "no such dir"; continue; }
    root="$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null || printf '%s' "$dir")"
    SS="$(apid session_start "$(jq -nc --arg c "$dir" --arg r "$root" '{cwd:$c, root:$r}')")"
    name="$(basename "$dir")"
    expect "live copy: $name index within budget, nearest first" "$SS" \
      '(.index_block | length) <= 3000 and (.candidates_line | test("^Memory scopes here"))' "over budget or no candidates"
    # Structure: the candidates line and the index headings, in order.
    shape="$(printf '%s' "$SS" | jq -r '.candidates_line, (.index_block | split("\n") | map(select(startswith("## Index — "))) | .[])')"
    if [ "$UPDATE_GOLDEN" = 1 ] || [ ! -f "$GOLDEN/$name.txt" ]; then
      printf '%s\n' "$shape" >"$GOLDEN/$name.txt"
      pass "live copy: golden $name" "recorded ($GOLDEN/$name.txt)"
    elif [ "$shape" = "$(cat "$GOLDEN/$name.txt")" ]; then
      pass "live copy: golden $name" "matches"
    else
      fail "live copy: golden $name" "differs: $(diff <(printf '%s\n' "$shape") "$GOLDEN/$name.txt" | head -4 | tr '\n' ' ') (--update-golden if intended)"
    fi
  done

  # Real-data scope invariants: no row outside root / its ancestors / the person.
  for root in "$REAL_HOME/workspace/linggen/linggen-memory" "$REAL_HOME/workspace/linggen/skills"; do
    [ -d "$root" ] || continue
    anc="$(jq -nc --arg r "$root" --arg h "$REAL_HOME" '[$r | split("/") as $p | range(1; $p | length) | $p[0:.] | join("/")] | map(select(length > ($h | length)))')"
    R="$(search "what did we decide about memory scope and recall" "$(jq -nc --arg r "$root" '{scope_root:$r, limit:20}')")"
    expect "live copy: recall in $(basename "$root") stays in scope" "$R" \
      "all(.[]; .scope as \$s | \$s == null or (\$s | startswith(\"$root/\") or . == \"$root\") or ($anc | index(\$s) != null))" "a row from outside the root"
  done
  SKILL="$REAL_HOME/.linggen/skills/lingjing"
  if [ -d "$SKILL" ]; then
    R="$(search "what happens in the story next" "$(jq -nc --arg r "$SKILL" '{scope_root:$r, limit:20}')")"
    expect "live copy: skill recall stays in its dir" "$R" "all(.[]; (.scope // \"\") | startswith(\"$SKILL\"))" "a row outside the skill dir"
  fi
  R="$(search "what should I cook for dinner tonight" "$(jq -nc --arg r "$REAL_HOME" '{scope_root:$r, limit:10}')")"
  expect "live copy: no-root rule on real rows" "$R" \
    '[.[] | select(.scope != null)] | length <= 2 and all(.[]; .type != "preference" and .score >= 0.70)' "no-root rule broken"
  stop_daemon
  PORT="$FPORT"

  section "Schema migration on scratch copies"
  # Migration backups expire after 14 days (src/memory/backups.rs), so these
  # checks skip once the live v1 copies are gone; cargo test covers v1 → v2.
  B="$REAL_HOME/.linggen/memory/backups"
  WITH="$(ls -d "$B"/schema-v2-* 2>/dev/null | tail -1)"
  WITHOUT="$(ls -d "$B"/pre-scope-deploy-* 2>/dev/null | tail -1)"
  [ -n "$WITH" ] && migrate_check "v1-with-hook" "$WITH" || skip "migration v1-with-hook" "no backups/schema-v2-*"
  [ -n "$WITHOUT" ] && migrate_check "v1-without-hook" "$WITHOUT" || skip "migration v1-without-hook" "no backups/pre-scope-deploy-*"
else
  section "Schema migration"
  skip "migration v1 → v2 (live backups)" "needs --copy-live; cargo test covers store::v1_to_v2"
fi

# ── Summary ─────────────────────────────────────────────────────────────────

printf '\n%d passed, %d failed, %d skipped (%s)\n' "$N_PASS" "$N_FAIL" "$N_SKIP" "$("$BIN" --version)"
[ "$N_FAIL" -eq 0 ]
