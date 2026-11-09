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
  all normal. Never retry those, never re-verify.
- **A failed write doesn't end the day.** An error on `add` may still
  have saved the row — a timeout says so. Search its gist once: there →
  carry on; absent → retry the add once, then carry on either way.
  Finish the day and stamp it.
- **Status lines, not prose:** `DAY <date> rows=<n>` → `PROMOTE <id>
  "<gist>"` per promotion (`MERGE <new-id> replaces=<k> "<gist>"` per
  derived merge) → `DAY <date> done judged=<n> promoted=<k>` →
  `SWEEP removed=<n>` → `SETTLE <issue-id> [<kind>] merged|kept "<why>"`
  per item the audit settled itself → `QUEUE <issue-id> [<kind>]
  "<question>"` per item left for the user → `FIX <id> <field>=<new>
  (was <old>) "<why>"` per scope/index/summary fix → one final totals
  sentence (settled and queued counts included). These lines are the
  run's record of every change it made on its own. Never print a
  status line for a call you didn't make.

## Before any dream — where it runs

Memory is dreamed on the **primary** machine, by its Linggen engine when it
has one.

- `memory_role` first. `role: replica` → reply exactly
  `Memory is dreamed on the primary (<upstream>)` and stop.
- On the primary, if the Linggen engine's `memory_dream_run` tool answers,
  offload to it (pass the date when one was given), say the engine is
  dreaming, and stop. Run the pass below yourself only when no engine
  answers on this machine.

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
3. Repeat from 1. If a day you already stamped in this pass comes
   back, **stop and report** ("stalled") instead of looping. A day
   dreamed on an earlier night that late rows re-opened is not a
   stall — remember it.
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

0. **Lock the day.** `{"verb":"dream_lease","day":"<date>","holder":"claude-code@<host>","action":"acquire"}`
   (MCP `memory_dream_lease`) before reading anything — one executor per
   day. Refused → report the reason verbatim and skip the day.
1. **Re-pend check.** From the `days` rollup, note the day's
   `remembered_at`. If set, only rows created after it need judging —
   earlier rows were already judged, and the worklist leaves them out.
2. **Worklist.** List the day's unjudged episodic rows:
   `{"verb":"list","tier":"episodic","day":"<date>","unjudged":true,"limit":25,"sort":"oldest"}`
   (CLI: `ling-mem list --tier episodic --day <date> --unjudged --sort oldest`).
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
     content **verbatim**, its `type`/`from`, `tier=semantic`
     (always explicit — an omitted tier lands episodic), `occurred_at`
     carried forward (else `created_at`), `source_session` if present,
     the row's stored `scope` as the request's `cwd` if present, and its
     `summary` if present. `scope` is the directory the memory is
     ABOUT — carrying it is what keeps the promoted row findable from
     there; a row with no `scope` gets none, never this session's own
     directory. Never pass `id`; whether the row joins the index is the
     scope and index lane's call (below); `tier=core` only for a narrow
     universal about the person (core carries no scope). Search-first: a quick
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
5. **Stamp.** `{"verb":"remember_day","date":"<date>","judged":<seen>,"promoted":<adds>,"holder":"claude-code@<host>"}`
   (CLI: `ling-mem remember-day <date> --judged N --promoted K`).
   Never skip the stamp, even with zero promotions — it's what moves
   the day marked dreamed. Then release the lock:
   `{"verb":"dream_lease","day":"<date>","holder":"claude-code@<host>","action":"release"}`.

## Forget (the sweep)

`{"verb":"sweep"}` (CLI: `ling-mem sweep`). Mechanical, zero-LLM,
self-guarding: evicts only rows that are past the episodic TTL **and**
on a remembered day **and** created before that day's stamp. Un-judged
rows are untouchable — an undreamed day keeps its rows forever until
someone remembers it. Safe to call anytime; `--dry-run` previews.

## Audit (after the sweep, clean-worklist runs only)

**Settle it yourself; queue only what needs the user.** Every item the
audit would queue, it first tries to settle the way solve does —
evidence first, then the fix. Main run only: no Task or subagents,
this run is the only writer. Three capped passes:

1. **Condense cited chains** — `{"verb":"chains","kind":"cited","limit":10,"derived_only":true}`
   (CLI: `ling-mem chains --kind cited --derived-only --limit 10`).
   Pre-confirmed id-citation chains of your own notes: collapse each
   into ONE current-truth row via `replace_ids`. See
   `references/condense-flow.md` for drafting rules.
2. **Markers** — `{"verb":"chains","kind":"marker","limit":5}` (no
   `derived_only`: user-voice candidates still need judging). The
   daemon excludes rows any review issue names, open or closed
   (`queued_skipped` reports how many). Per candidate, in order:
   - **MERGE on the completion bar** — row AND neighbor are agent
     notes (`from=derived`, `tier=semantic`); the neighbor is strictly
     newer, the same subject (the same work, not merely the same
     project), and asserts the marked work done (SHIPPED / FIXED /
     DONE / VERIFIED / committed-and-pushed). Collapse per
     `references/condense-flow.md`, `replace_ids` = the marker row +
     every qualifying neighbor. Any clause in doubt → the next lane.
   - Skip rows younger than ~14 days (write-time supersede gets
     first chance).
   - **Settle from evidence.** `memory_get` each row in full,
     `memory_search` the subject for a row that completes, overtakes
     or contradicts it (any tier — recent episodic counts), and read
     git history since the row's date, code and docs when this host
     has those tools (Claude Code / Codex do; the Linggen nightly
     mission is memory-only). Settled, and every row that would
     change is `from=derived`:
     - a change is due → ONE `memory_add` + `replace_ids` writing
       current truth;
     - nothing to change (still true, keep separate) → write nothing.

     Record it either way: `memory_issue_add` (the kind, the row ids,
     note = what you did + why, one plain line), then at once
     `memory_issue_resolve` that id — `resolved` for a change,
     `dismissed` for keep-as-is — same note. The closed item is the
     record, and it stops the scan re-serving the rows.
   - **Queue only when** a `from=user` row would change (needs the
     user's consent), rows disagree and the user must pick
     (`contradiction`), or the evidence is out of reach on this host
     (permission declined, outside your grants, project not on this
     machine, no tool to check). `memory_issue_add {"kind":"<k>","row_ids":[...],"note":"..."}`
     (CLI: `ling-mem issue-add --kind <k> --row <id> "<note>"`). The
     note is ONE plain-word question with your recommendation — it
     may become the user's question as is. A deduped response
     ("already queued") is success.

   Never pass `user_directed:true` in a dream.

3. **Digest the quiet** —
   `{"verb":"chains","kind":"subject","limit":5,"derived_only":true}`
   (CLI: `ling-mem chains --kind subject --derived-only --limit 5`).
   The daemon serves only QUIET clusters (newest member >30 days),
   only your own notes, never rows a prior subject ruling covers.
   Read the members in full, then per cluster exactly one of:
   - **DIGEST** — the members (or a coherent 3+ subset) share ONE
     subject: one digest row per the condense drafting rules — `add`
     with `"tier":"semantic"` + `replace_ids` = the coherent subset
     only; outliers untouched. Members are archived, not deleted.
   - **SETTLE kept** — clearly different stories: `issue-add --kind
     subject` listing ALL member ids, note = "separate — <why>", then
     resolve it `dismissed` at once. The dismissal is the ruling; the
     cluster never re-forms.
   - **QUEUE** — only when you genuinely cannot tell from the full
     members: `issue-add --kind subject` listing ALL member ids, note
     = one plain question + your guess.

What stays open is the rare leftover for the attended `/linggen:solve`.

## Scope and index lane

Fixes you apply yourself, never queue. A row's scope, index flag and
summary say where it is filed and how the index shows it, not what it
says: change them on any row, `from=user` included, via
`memory_update` (CLI: `ling-mem edit`) — never its `content`. While
judging, fix:

- **scope** — filed under the wrong directory →
  `{"id":"<id>","scope":"<absolute dir>"}` (CLI: `--scope <dir>`); about
  the person but filed under a project → `{"id":"<id>","global":true}`
  (CLI: `--clear-scope`);
- **index in** — a `from=user` standing rule ("always…", "never…",
  "from now on…", "以后都…") not indexed →
  `{"id":"<id>","indexed":true,"summary":"<one line, ≤ 80 chars>"}`;
- **index out** — an indexed row that is no standing rule, or no
  longer holds → `{"id":"<id>","indexed":false}`;
- **summary** — an indexed row with none, or one that misreads the
  row → `{"id":"<id>","summary":"<one line>"}`. Leave other rows'
  summaries alone.

Only when sure; unsure → leave it (nothing is queued). At most 10 per
run, one line each: `FIX <id> <field>=<new value> (was <old>) "<why, ≤60 chars>"`.

## Reporting (Linggen dashboard)

No PageUpdate is needed: the memory page watches the tool stream and
repaints tier counts + the calendar from the daemon's `days` rollup
after your `Memory_write` calls land. End with the status lines and a
one-line totals sentence — e.g. *"Remembered 2 days: 5 promoted, 31
judged; sweep evicted 12; audit settled 4, queued 1."* The closed
review items (`memory_issues {"status":"resolved"}` / `"dismissed"`)
keep each settled ruling with its why.
