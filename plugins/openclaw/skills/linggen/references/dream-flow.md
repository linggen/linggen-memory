# Dream flow — remember + forget (canonical runbook)

Two user-facing functions: **scan** (stage a day's session logs) and
**dream** (= remember + forget). This file is the canonical procedure
every trigger runs:

- **Linggen** — the built-in `dream` mission under the `memory` agent
  runs every dream: the nightly cron, the memory app's Run-dream
  button, and the calendar day buttons (day-scoped trigger). The
  skill session runs only **scan** (`/linggen scan <date>`) and
  explicit chat requests.
- **Claude Code / Codex / OpenClaw** — no mission runtime; the host
  agent runs the same steps via the `ling-mem` CLI (or the `memory_*`
  MCP tools).

Day-granular: the unit of work is one **local calendar day** of
episodic staging. Pending days drain **oldest first**.

## Interface

On **Linggen**, use the built-in `Memory_query` / `Memory_write` tools
(Chat-tier, ungated — zero permission prompts across a pass full of
writes): verbs `days`, `list` (+`day`), `add`, `remember_day`,
`harvest_day` (the scan stamp), `sweep`.
On **other hosts**, the CLI is 1:1: `ling-mem days [--undreamed]`,
`ling-mem list --tier episodic --day <date>`, `ling-mem add`,
`ling-mem remember-day <date>`, `ling-mem harvest-day <date>`,
`ling-mem sweep`. Always pipe CLI list/search output through
`jq -c 'del(.vector)'`.

State lives in the daemon (`.days.json` sidecar + the two tables) —
the old `.dream-state.json` / `.dream-history.jsonl` files are retired;
never write them.

## Ground rules

- **Unattended-safe.** Never call AskUser in a dream pass. When in
  doubt about durability, **promote** — a redundant semantic row is
  recoverable; lost signal isn't.
- **Remembering never deletes.** Episodic is short-term memory; judged
  rows stay until the sweep ages them out. One exception: a credential
  / API key / password found in staging is deleted on sight.
- **Only a tool_error is a failure.** `"action":"merged"` on add, a
  promoted row vanishing from episodic (the daemon's cross-tier dedup
  removed the twin during the add), `removed:false`, an empty list —
  all normal. Never retry, never re-verify.
- **Status lines, not prose:** `DAY <date> rows=<n>` → `PROMOTE <id>
  "<gist>"` per promotion (`MERGE <new-id> replaces=<k> "<gist>"` per
  derived merge) → `DAY <date> done judged=<n> promoted=<k>` →
  `SWEEP removed=<n>` → one final totals sentence. Never print a
  status line for a call you didn't make.

## `dream` (no argument) — remember all undreamed days

0. **Snapshot + in-flight check.** `ling-mem export` once (a store
   backup before any judged writes — the engine does the same before
   its mission runs). If the `memory_dream_status` MCP tool is
   reachable and reports `in_flight: true`, the Linggen engine is
   already dreaming — stop and say so; never run two dreams at once.
1. Fetch the worklist: `days` with `undreamed_only` (CLI:
   `ling-mem days --undreamed`). Empty → run **Forget** below, then
   **Audit** below, reply that memory is up to date, done.
2. Take the **oldest** undreamed day → run **Remember one day** below.
3. Repeat from 1. If the same day comes back with an undropped
   `unjudged` count, **stop and report** ("stalled") instead of
   looping.
4. When no days remain: run **Forget**, then **Audit**, then report
   totals.

## `dream <YYYY-MM-DD>` — remember one day

- Day has episodic rows → **Remember one day** below, then one sweep.
- Day has **no rows at all** → nothing to dream; suggest a scan if the
  user worked that day.
- Today / future dates are not dreamable — say so and stop.

## `scan <YYYY-MM-DD>` — stage one day's session logs

Backfill staging, always user-triggered, idempotent:

1. Run `Bash bash <skill-dir>/scripts/scan.sh <date>` (zero-LLM session
   walk → `.scan-output.jsonl`). `<skill-dir>` is this skill's own
   directory — the one holding this `references/`, resolved per
   SKILL.md (the plugin cache on Claude Code,
   `${PLUGIN_ROOT}/skills/linggen/` on Codex). Never hard-code an
   absolute path: the skill is not installed under `~/.linggen/skills`
   on these hosts.
2. **Skip covered sessions.** `list` the day's existing rows
   (`tier=episodic` + that `day`, and note promoted twins may live in
   semantic) and collect their `source_session` ids. Drop every
   scanned session already in that set — live capture or a prior scan
   contributed it. This is what makes re-scanning safe on
   partially-captured days.
3. Judge the remaining candidates per `extractor-prompt.md` +
   `routing-rules.md`; write keepers to episodic with `occurred_at`
   set to the session time.
4. Stamp scanned — `harvest_day` verb (CLI:
   `ling-mem harvest-day <date>`). This does **not** mark the day
   remembered: new rows clear its `dreamed` flag, and the next dream (nightly
   or the day's dream button) judges them. Nothing new staged → still
   stamp, report `CLEAN`.

## Remember one day

1. **Re-pend check.** From the `days` rollup, note the day's
   `remembered_at`. If set, judge **only** rows created after it —
   earlier rows were already judged.
2. **Worklist.** List the day's episodic rows:
   `{"verb":"list","tier":"episodic","day":"<date>","limit":25,"sort":"oldest"}`
   (CLI: `ling-mem list --tier episodic --day <date> --sort oldest`).
   Page with `offset` until every row is seen. Never pass
   `type`/`from`/`outcome` — they narrow the list to zero.
3. **Cluster.** Group near-duplicate rows on the same subject (per-turn
   capture restates facts across turns). Judge clusters, not
   restatements — one promotion per cluster, best-phrased
   representative; the rest simply age out later.
4. **Judge each cluster** — promote, merge, or skip:
   - **Promote** durable signal (user biography, cross-project
     preference, decision-with-reasoning, re-hit gotcha, state change
     like a shipped milestone, run learning): `add` with the row's
     content **verbatim**, its `type`/`from`/`contexts`, `occurred_at`
     carried forward (else `created_at`), `source_session` if present,
     `cwd` if present. `cwd` is WHERE the memory came from — carrying
     it is what keeps the promoted row findable from that project; a
     row with no `cwd` gets none, never this session's own directory.
     Never pass `id`. Omit tier (defaults semantic); `tier=core` only
     for a narrow universal about the person. Search-first: a quick
     semantic `search` on the gist — but a hit with `tier=episodic`
     never counts as "already in semantic". **The promote bar — state
     + lessons, never events.** Test: strip the date and the commit
     hash — still useful in three months? Per-event rows ("committed
     X", "pushed Y") fail: skip, or fold into the state row they
     evidence.
   - **Merge (own notes only).** If the pre-promote search surfaced
     older `semantic` rows on the same subject that are agent notes
     (`from=derived` — built/fixed/tried/learned) and the new row
     completes or obsoletes them ("impl not started" → "shipped"),
     write ONE current-truth row with `replace_ids` listing those
     semantic losers (atomic on every path; CLI:
     `ling-mem add ... --replace <id> --replace <id>`). Never list a user-voice row
     (`from=user`) or an episodic id.
   - **Skip** noise (activity logs, file-derivable facts,
     single-mention chatter) and already-in-semantic facts: do
     nothing — the row ages out on its own.
   - **Never** generalize into "user always X" rules, and never merge
     or resolve rows in the **user's voice** (`from=user`) — promote
     the contradicting row alongside the old one; recall-time
     reconciliation (user present) picks winners. Your own derived
     notes are the exception (Merge above).
5. **Stamp.** `{"verb":"remember_day","date":"<date>","judged":<seen>,"promoted":<adds>}`
   (CLI: `ling-mem remember-day <date> --judged N --promoted K`).
   Never skip the stamp, even with zero promotions — it's what moves
   the day marked dreamed.

## Forget (the sweep)

`{"verb":"sweep"}` (CLI: `ling-mem sweep`). Mechanical, zero-LLM,
self-guarding: evicts only rows that are past the episodic TTL **and**
on a remembered day **and** created before that day's stamp. Un-judged
rows are untouchable — an undreamed day keeps its rows forever until
someone remembers it. Safe to call anytime; `--dry-run` previews.

## Audit (after the sweep, clean-worklist runs only)

Confidence decides what happens to long-term staleness: solve what you
can prove, queue the rest for the user. Two capped passes:

1. **Condense cited chains** —
   `ling-mem chains --kind cited --derived-only --limit 10`.
   Pre-confirmed id-citation chains of your own notes: collapse each
   into ONE current-truth row (`replace_ids` over MCP; CLI: `--replace` on add). See `references/condense-flow.md`
   for drafting rules.
2. **Merge the provable, queue the rest** —
   `ling-mem chains --kind marker --limit 5` (no `--derived-only`:
   user-voice candidates still need queueing). The daemon excludes
   rows a review issue already names (`queued_skipped` reports how
   many), so every candidate is fresh. Per candidate, in order:
   - **MERGE on the completion bar** — every clause required: row AND
     neighbor are agent notes (`from=derived`, `tier=semantic`); the
     neighbor is strictly newer; same subject (the same work, not
     merely the same project); and it asserts completion of the
     marked work (SHIPPED / FIXED / DONE / VERIFIED /
     committed-and-pushed). The store already holds the answer —
     collapse per `references/condense-flow.md` drafting rules,
     `replace_ids` = the marker row + every qualifying neighbor
     (`replace_ids` over MCP; CLI: `--replace` on add). In doubt on ANY clause (partial completion, subject
     drift, a user-voice row in the cluster) the bar is NOT met —
     queue instead. A bad queue wastes a click; a bad merge loses a
     row.
   - Skip rows younger than ~14 days (write-time supersede gets
     first chance).
   - Otherwise queue via
     `ling-mem issue-add --kind <k> --row <id> [--row <id>] "<note>"` —
     `chain` for an uncertain merge (note: subject + both gists),
     `stale-status` for a provisional claim with no completing
     neighbor (note: the claim + "verify against git/files at solve
     time"), `contradiction` when user-voice rows disagree. `already
     queued` is success — the daemon dedups per (kind, row_ids).

   **Never merge `subject` clusters in a dream**, and never a marker
   candidate below the bar — solving queued items is `/linggen
   solve`, with the user present.

## Reporting (Linggen dashboard)

No PageUpdate is needed: the memory page watches the tool stream and
repaints tier counts + the calendar from the daemon's `days` rollup
after your `Memory_write` calls land. End with the status lines and a
one-line totals sentence — e.g. *"Remembered 2 days: 5 promoted, 31
judged; sweep evicted 12."*
