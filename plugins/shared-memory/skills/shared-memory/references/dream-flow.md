# Dream flow — remember + forget (canonical runbook)

Two user-facing functions: **scan** (stage a day's session logs) and
**dream** (= remember + forget). This file is the canonical procedure
every trigger runs:

- **Linggen** — the built-in `dream` mission under the `memory` agent
  runs every dream: the nightly cron, the memory app's Run-dream
  button, and the calendar day buttons (day-scoped trigger). The
  skill session runs only **scan** (`/shared-memory scan <date>`) and
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
On **other hosts**, the CLI is 1:1: `ling-mem days [--pending]`,
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
  "<gist>"` per promotion → `DAY <date> done judged=<n> promoted=<k>`
  → `SWEEP removed=<n>` → one final totals sentence. Never print a
  status line for a call you didn't make.

## `dream` (no argument) — remember all pending days

1. Fetch the worklist: `days` with `pending_only` (CLI:
   `ling-mem days --pending`). Empty → run **Forget** below, reply
   that memory is up to date, done.
2. Take the **oldest** pending day → run **Remember one day** below.
3. Repeat from 1. If the same day comes back with an undropped
   `unjudged` count, **stop and report** ("stalled") instead of
   looping.
4. When no days remain: run **Forget**, then report totals.

## `dream <YYYY-MM-DD>` — remember one day

- Day has episodic rows → **Remember one day** below, then one sweep.
- Day has **no rows at all** → nothing to dream; suggest a scan if the
  user worked that day.
- Today / future dates are not dreamable — say so and stop.

## `scan <YYYY-MM-DD>` — stage one day's session logs

Backfill staging, always user-triggered, idempotent:

1. Run `Bash bash ~/.linggen/skills/shared-memory/scripts/scan.sh <date>`
   (zero-LLM session walk → `.scan-output.jsonl`).
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
   remembered: new rows make it *pending*, and the next dream (nightly
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
4. **Judge each cluster** — promote or skip:
   - **Promote** durable signal (user biography, cross-project
     preference, decision-with-reasoning, re-hit gotcha, shipped
     milestone, run learning): `add` with the row's content
     **verbatim**, its `type`/`from`/`contexts`, `occurred_at` carried
     forward (else `created_at`), `source_session` if present. Never
     pass `id`/`replace_ids`. Omit tier (defaults semantic);
     `tier=core` only for a narrow universal about the person.
     Search-first: a quick semantic `search` on the gist — but a hit
     with `tier=episodic` never counts as "already in semantic".
   - **Skip** noise (activity logs, file-derivable facts,
     single-mention chatter) and already-in-semantic facts: do
     nothing — the row ages out on its own.
   - **Never** merge distinct facts, generalize into "user always X"
     rules, or resolve contradictions — promote the contradicting row
     alongside the old one; recall-time reconciliation (user present)
     picks winners.
5. **Stamp.** `{"verb":"remember_day","date":"<date>","judged":<seen>,"promoted":<adds>}`
   (CLI: `ling-mem remember-day <date> --judged N --promoted K`).
   Never skip the stamp, even with zero promotions — it's what moves
   the day out of pending.

## Forget (the sweep)

`{"verb":"sweep"}` (CLI: `ling-mem sweep`). Mechanical, zero-LLM,
self-guarding: evicts only rows that are past the episodic TTL **and**
on a remembered day **and** created before that day's stamp. Un-judged
rows are untouchable — an undreamed day keeps its rows forever until
someone remembers it. Safe to call anytime; `--dry-run` previews.

## Reporting (Linggen dashboard)

No PageUpdate is needed: the memory page watches the tool stream and
repaints tier counts + the calendar from the daemon's `days` rollup
after your `Memory_write` calls land. End with the status lines and a
one-line totals sentence — e.g. *"Remembered 2 days: 5 promoted, 31
judged; sweep evicted 12."*
