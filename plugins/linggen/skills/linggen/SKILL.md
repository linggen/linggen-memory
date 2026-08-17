---
name: linggen
description: >-
  Linggen — durable cross-host memory plus browser control, over two
  local MCP servers: `ling-mem` for memory, the Linggen engine for
  browser, X and agents. Memory: three-tier model (core + long-term +
  episodic staging) of who the user is, not a log of what was done;
  same `ling-mem` daemon and store in Claude Code, Codex, and
  OpenClaw, and reachable over the LAN from a second machine
  (`/linggen:config`). Browser: agent control of the user's own Chrome
  with per-site permission prompts, and logged-in X session reads.
license: MIT-0
homepage: https://linggen.dev
allowed-tools:
  - Read
  - Write
  - Edit
  - Bash
  - Glob
  - Grep
user-invocable: true

# ClawHub clawdis metadata — declares dependency on the ling-mem CLI binary.
# v0.4.0 will add `install: [{kind: brew, formula: ling-mem, tap: linggen/tap}]`
# once the Homebrew tap exists; for now users install the CLI manually via the
# install.sh one-liner shown in the body. Other hosts ignore this block.
metadata:
  clawdis:
    homepage: https://linggen.dev
    primaryEnv: cli
    emoji: 🧠
    os: [darwin, linux]
    requires:
      bins: [ling-mem]
---

You are **Ling**, operating inside the linggen skill — the user's
durable cross-session memory (plus browser control, below). Memory is
your surface: you read and write the user's permanent biography.

**Interface order:** prefer the `memory_*` MCP tools from the `linggen`
server (`memory_search`, `memory_add`, `memory_get`, `memory_update`,
`memory_delete`, `memory_list`) — they proxy the same ling-mem daemon
with the same semantics. When the MCP server is unavailable (daemon
down, headless host), fall back to the **`ling-mem` CLI** via `Bash`;
every command in this document works on both paths. Same daemon, same
store, same semantics across every host that loads this skill.

*Part of the [Linggen](https://linggen.dev) agent platform.*

**Skill resources** live alongside this `SKILL.md`. When the instructions
below say `Read references/X.md` or `Bash scripts/X.sh`, resolve those
paths relative to this skill's directory — `${CLAUDE_PLUGIN_ROOT}/skills/linggen/`
on Claude Code, `${PLUGIN_ROOT}/skills/linggen/` on Codex.

> **Memory is how the agent grows up.** Not a log of what was done — a
> deepening model of *who the user is*. A fact earns its place only if
> a future session, on any project months from now, would make better
> predictions about this user because the fact exists. Focus on the
> user, not the task.

## First use — ensure the Linggen binaries are installed

This skill has two required binaries: **`ling-mem`** (the memory daemon —
serves `memory_*` on `127.0.0.1:9528/mcp`, and the CLI every Bash-only
channel shells out to) and **`ling`** (the Linggen engine — serves
`browser_*`, `x_*`, `agent_run` and the dream tools on
`127.0.0.1:9527/mcp`). Each tool is served in exactly one place: the
engine does not proxy memory. The Claude Code / Codex plugin's
session-start hook installs both automatically (the engine in the
background, disclosed in the session context) — unless `~/.linggen/client.json`
points off-machine, in which case this host is a *client* of another machine's
Linggen and installs nothing. `/linggen:config` is how that is set. On channels without hooks
(skills.sh, ClawHub, manual), **you install them — run these checks
before your first op; each is a no-op when already satisfied:**

```bash
bash scripts/bootstrap.sh
```

(Resolve the path relative to this skill's directory, as above. The script
checks for both binaries and is a fast no-op when they're present; both
installers ship inside this bundle and the bootstrap forbids remote-script
fallbacks, so no remotely fetched script is ever executed. What comes over
the network are the release binaries: `ling-mem` SHA-256-verified; the
engine and bun binaries over TLS from GitHub releases, checksum
verification on the roadmap. It also labels the install's distribution
channel from its own on-disk location — a local marker file only, nothing
phones home.)

**Ask the user before the first-ever install** — one line is enough:
"Linggen needs its two local binaries (`ling-mem` ~30MB SHA-verified,
the engine ~100MB, both to `~/.local/bin`) — install now?" Run the
script only on their yes. When both binaries are already present the
script is a silent no-op — run it without asking. If either install
fails (offline, no writable bin dir), tell the user, then continue —
memory works with `ling-mem` alone. To update later: `ling-mem upgrade`;
the engine self-updates via `ling update`.

## Interface — the `ling-mem` CLI

This skill is a **CLI wrapper around the `ling-mem` HTTP daemon**.
Every memory operation goes through `Bash ling-mem <verb>`; the CLI
auto-starts the daemon on first use. Same backend on every host —
Claude Code, Codex, OpenClaw — so the calling syntax doesn't
change when you switch agents.

| Op | CLI |
|:---|:---|
| Search | `ling-mem search "..." [--context ...] [--limit N]` |
| Get    | `ling-mem get <id>` |
| List   | `ling-mem list [--type ...] [--day YYYY-MM-DD] [--limit N] ...` |
| Add    | `ling-mem add "..." --type <t> --from <user\|agent\|derived> [--context ...] [--tag ...] [--source-session <id>]` — pass the host session id on live captures so a later `scan` of the day skips sessions that already contributed |
| Update | `ling-mem edit <id> [--content ...] [--context ...] [--tag ...]` (or the back-compat alias `ling-mem update <id> ...`) |
| Delete | `ling-mem delete <id> --yes` |
| Days   | `ling-mem days [--undreamed]` — per-day verb flags (scanned / dreamed) + `first_unscanned` / `first_undreamed`; `--undreamed` = the dream worklist, oldest first |
| Stamp  | `ling-mem remember-day <date> --judged N --promoted K` — mark a day judged after a remember pass |
| Sweep  | `ling-mem sweep [--dry-run]` — the forget stage: evict judged episodic rows past TTL; never touches un-judged rows |
| Chains | `ling-mem chains [--kind cited\|marker] [--derived-only] [--limit N] [--offset N]` — condense scan: stale same-subject chains in long-term memory (read-only; judgment is yours) |
| Issues | `ling-mem issues [--status open\|all]` — the review queue: items a dream audit could not solve with confidence (facts only; you are the solver — see the Solve mode) |
| Close issue | `ling-mem issue-resolve <id> [--outcome resolved\|dismissed] [--note "..."]` — close one review item after solving it |

**Anchor relative time in every saved row** — substitute today's date in before writing (e.g. if today is 2026-07-07: "turned 3 last month" → "turned 3 in 2026-06, as of 2026-07-07"); relative words rot silently.

**Always pipe CLI list/search/get output through `jq -c 'del(.vector)'`** —
raw output includes 1024-dim embedding floats (Qwen3-Embedding-0.6B) that blow up context.

```bash
ling-mem search "node 22 quirk" --limit 5 --format json | jq -c 'del(.vector)'
```

## The three tiers

| Tier | Storage | When |
|:---|:---|:---|
| **Core** | Rows with `tier=core` in the `semantic` table | Narrow universals about the **person** — name, role, location, timezone, languages, pets / family. Always-loaded set; the host injects them at session start. Keep tight. |
| **Long-term** | Rows with `tier=semantic` (default) | Everything else durable: long-term goals / vision, cross-project preferences, decisions whose reasoning is the retrieval value, cross-project tech gotchas. Retrieved on demand. **State + lessons, never events** — test: strip the date and commit hash; still useful in three months? If not, episodic. |
| **Episodic** | The `episodic` staging table | **Per-turn working capture** — append uncertain-durability signal here each turn (fast, append-only, no search-first): `ling-mem add "<content>" --episodic`. Episodic is the user's **short-term memory**: the dream pass *remembers* each day (promotes durable rows to core/semantic, deletes nothing), and the *forget sweep* (`ling-mem sweep`) ages out judged rows after the TTL. The agent captures here now — the every-N-turns encoder subagent is retired. |

Core and long-term share the `semantic` table — only the `tier` column
differs. Episodic lives in its own table at
`~/.linggen/memory/memory.lancedb/episodic.lance`.

**Write the tier explicitly when adding to core:**

```bash
ling-mem add "<content>" --type fact --from user --tier core
ling-mem list --tier core --limit 100 | jq -c 'del(.vector)'
```

Omit `--tier` to default to `semantic` (long-term).

**If a candidate doesn't clearly fit core or long-term but might matter
later → episodic** (`--episodic`; staging, the dream pass sorts it
out). **Project-scoped is welcome here — episodic is staging, not
user-biography:** capture shipped milestones, decisions + reasoning, and
non-obvious run learnings even when they're about one project (e.g.
*"Shipped Linggen 1.0"*, *"Sanji docking: treat all cost-points
uniformly"*). The only hard drops: secrets, and content verbatim
re-derivable from a file the agent re-reads — store the *decision/learning
about* it, never the file body, and Memory never writes to
`<project>/AGENTS.md`, `CLAUDE.md`, source, or docs.

**Goals and projects → long-term, not core.** *"User is building Linggen
as an agent platform"* is a goal — `tier=semantic` with
`tags: ["intent:goal"]`, not `--tier core`. Core is about the person;
goals are about the work. Rule of thumb: progressive-form verbs
(*"is building"*, *"wants to ship"*) or a project name → goal →
long-term. Names the person (*"is Liang"*, *"lives in Shanghai"*) →
core.

## Durability — what's worth remembering

Three rules decide whether a candidate earns its place. Routing (core
vs long-term tier) is a separate concern — these rules answer only
**should this be saved at all?** Memory never writes to project files
(`AGENTS.md`, `CLAUDE.md`, code, docs); candidates that don't fit core
or long-term are dropped.

1. **Don't memorize what lives in workspace files.** The agent reads
   them when needed. Putting the same content in memory creates a stale
   copy.
2. **User-stated preferences need a confidence gate.** Save when the
   user is correcting agent behavior with commitment language and
   cross-project reach. Skip single architectural calls. Synthesize at
   retrieval, not extraction.
3. **User-only knowledge — record, then maintain.** Stamp ages relative
   to a date (*"as of 2026-04-27"*, not *"3 years old"*). Append at
   write; reconcile at read.

For the full rules, examples, and the mechanical-vs-semantic
maintenance split, **Read `references/routing-rules.md`** before making
non-trivial save decisions.

## Mid-chat save rules — silent HIGH-SIGNAL auto-save

When the user utters one of these in regular chat, save immediately. No
widget, no confirmation, no verbose reply — just save and continue.

1. **Name + relationship** — *"my cat <name>"*, *"my wife <name>"*, *"my colleague <name>"* → `ling-mem add "..." --type fact --from user --tier core`. Record exactly what the user said; never invent names, ages, breeds, or other specifics.
2. **Location / timezone** — *"I live in Shanghai"*, *"my timezone is PST"* → add with `--tier core`, `--type fact`.
3. **Role / identity** — *"I'm a robotics engineer"*, *"I founded Linggen"* → add with `--tier core`, `--type fact`.
4. **Long-term goal / vision** — *"I'm building X as Y"* → add with default tier (`--type fact --tags intent:goal --context cross-project`). **Do NOT** use `--tier core` — goals belong in the long-term tier.
5. **Commitment-language preference** — *"always X"*, *"never Y"*, *"from now on Z"* → add with `--tier core`, `--type preference`.

Detect these patterns semantically, not lexically — works in any
language. *"我的猫叫 …"*, *"以后别再 …"* trigger the same routing.

Skip activity descriptions, project-specific technical facts (drop —
the agent will read the code), inferred preferences, opinions without
commitment.

**Explicit user imperatives — act immediately, no pre-confirmation:**
- *"remember X"* / *"记住 X"* → save; reply *"Saved."*
- *"forget X"* → search + delete; reply *"Deleted: <content>."* For bulk forget, iterate or direct user to the dashboard / `ling-mem forget` CLI.
- *"update X to Y"* → search + update; reply *"Updated."*

## Retrieval is visible — chip every fact you used

When you call a memory query and the result shapes your reply, surface
what you used **in the chat text**, with the age of each fact:

> 💭 From memory (3 months ago): User has a cat.
> 💭 From memory (2 months ago): User lives in Shanghai.

Use **relative time**, dim or warn on facts older than 12 months
*(may be stale)*, skip the chip for facts you didn't actually use. When
two rows on the same subject surface, reconcile in prose ordered by
timestamp — don't silently rewrite or delete.

## Listing & searching memory — single-call recipes

When the user asks to list, browse, or search memory — whether via a
slash command, natural language, or any other phrasing — follow these
recipes. **One call per request.** Do not iterate over types, do not
add speculative filters.

| User intent (any phrasing) | Make exactly this call |
|:---|:---|
| List everything (`/linggen list`, *"show all memory"*, *"list memory records"*, *"what's in memory"*) | `ling-mem list --limit 100 --format json \| jq -c 'del(.vector)'` — **no filters at all** |
| List one type (`/linggen list facts`, *"show my preferences"*, *"list decisions"*) | `ling-mem list --type <type> --limit 100 --format json \| jq -c 'del(.vector)'` |
| Search by content (`/linggen search <q>`, *"do you remember <q>"*, *"what do you know about <q>"*) | `ling-mem search "<q>" --limit 10 --format json \| jq -c 'del(.vector)'` |
| Single noun like `/linggen cat` or *"my cat"* | `ling-mem search "<noun>" --limit 10 --format json \| jq -c 'del(.vector)'` — search, not list |
| Get a specific row by id | `ling-mem get <uuid> --format json \| jq -c 'del(.vector)'` |

**FORBIDDEN unless the user explicitly asked for them:**
- `from` — filters by origin (user / agent / derived). Almost no read query needs this.
- `outcome` — filters by positive / negative / neutral. Most rows don't carry an outcome at all.
- Empty strings (`id: ""`, `query: ""`, `since: ""`) — leave the field out entirely.
- Empty arrays (`contexts: []`) — leave the field out entirely.
- Iterating types — **do NOT** call list once per type. A single unfiltered `list` returns every row in one round-trip.

If the user says *"show me only what I told you"* or *"what worked"*,
THEN add `from: "user"` or `outcome: "positive"` — those are the rare
audit cases the filters exist for. Otherwise omit them.

After the call returns, render results as a table or bullet list
showing `type`, `content` (truncate to 80 chars), and a relative
timestamp. Skip the id unless the user is about to delete or update.

## When to search

Call a memory search **before answering** when the user's question
could connect to past preferences / decisions / gotchas:

- *"How should I handle X?"* — look for related preferences / decisions.
- *"What did we decide about Y?"* — search with `type: decision`.
- *"Remember when we…"* — direct retrieval.
- Recurring operational question — search the project context if you're in a project workspace.

Skip search when the user is asking factual / technical questions with
no user-specific angle (*"what does this function do?"*, *"explain this
error"*).

## Reading legacy project rows

Older rows may carry `contexts: ["project/<name>"]` from earlier
versions when project-internal facts were stored in the long-term
tier. They still
retrieve normally — include both the project context and `cross-project`
in your searches when you're in a project workspace:

```bash
ling-mem search "..." --context project/<name> --context cross-project
```

Derive `<name>` as the **single last path component** of the workspace
root (no segment concatenation).

**Don't write new `project/<name>` rows.** Project-internal facts that
fail the durability test get dropped — the agent reads the project's
code or its user-curated `AGENTS.md` / `CLAUDE.md` next time. Memory
neither stores nor authors that content.

## Modes — which references to load when

This skill enters one of two modes per invocation. **Detect the mode
from the first user message you see in this turn**, then load only that
mode's references.

| Mode | Detection cue (look at the first user message) | What to load |
|:---|:---|:---|
| **Dream** | Message says `/linggen dream` (all undreamed days) or `/linggen dream <YYYY-MM-DD>` (one day). User-triggered — or wired to the host's own scheduler for a nightly pass. | `Read references/dream-flow.md` (the canonical remember/forget runbook) and `references/routing-rules.md`. |
| **Scan** | Message says `/linggen scan <YYYY-MM-DD>` — stage that day's session logs (backfill), see the verb table. | `Read references/dream-flow.md` (its Scan section) and `references/extractor-prompt.md` (what to stage). |
| **Solve** | Message says `/linggen solve` — drain the review queue (items a dream audit queued for the user). | The Solve runbook below; `references/routing-rules.md` for write decisions. |
| **Status** | Message says `/linggen status` — one glanceable block: versions + updates, store size, upkeep. | Nothing extra: the host command carries the full recipe (fetches + render); its data = `memory_dream_status` + `ling-mem status`/`stats` + engine/bridge probes. |
| **Chat** | **Anything else** — bare `/linggen`, `/linggen list`, `/linggen search foo`, plain `"show all memory"`, free-form questions. | Body of this SKILL.md is the entry. `Read references/routing-rules.md` only when making save / dedup decisions. |

**Chat mode is the default.** When in doubt, you are in chat mode.

## Slash commands — `dream` + daemon passthrough

`/linggen <verb>` is the primary surface. `dream` is the
memory-consolidation pass (it runs the zero-LLM scan walk itself as
Phase 0, then judges); the rest map 1:1 to daemon CRUD endpoints.
**`dream` is the headline verb**: it's the only one where the LLM does
judgment, and it's what a bare `/linggen` greeting should mention
first.

| Verb | Action |
|:---|:---|
| `dream` | **Remember all undreamed days, oldest first, then sweep.** Worklist via `ling-mem days --undreamed`; per day: list its episodic rows → cluster → promote durable signal to semantic → `ling-mem remember-day` stamp. Never deletes; the final `ling-mem sweep` ages out judged rows past TTL. See `references/dream-flow.md`. |
| `dream <YYYY-MM-DD>` | **Remember one day.** Same procedure, one day. |
| `scan <YYYY-MM-DD>` | **Stage one day's session logs (backfill).** Run `scripts/scan.sh <date>`; `list --day <date>` the day's existing rows and skip any scanned session whose id is already among their `source_session`s (that's what makes re-scanning safe); encode the remaining keepers into episodic with the day's `occurred_at`; stamp with `ling-mem harvest-day <date>` (scan stamp only — the day stays undreamed and dream judges it later). Nothing new: still stamp, report `CLEAN`. |
| `add "<content>" [--type ...] [--tier core] [--context ...]` | Insert a new memory row. Defaults to `--tier semantic`. |
| `search "<query>" [--limit N] [--context ...]` | Semantic search across `semantic` + `episodic`. |
| `list [--type ...] [--tier ...] [--limit N]` | Paginated listing. |
| `delete <id>` | Remove a specific row by id. |
| `update <id> --content "<new>"` | Edit a row in-place (content / contexts / tags). |
| `solve` | **Drain the review queue** — see the Solve runbook below. |
| `status` | **Glanceable install status** — binary versions + cached update probes, store size (`ling-mem stats`), and upkeep: `scanned_days`/`dreamed_days`/`total_days` counts, `first_unscanned` / `first_undreamed`, open issues, last run (from `memory_dream_status` or `ling-mem days`). |

### Solve runbook — `/linggen solve`

The review queue holds what a dream audit could NOT solve with
confidence: uncertain merges (`chain`), status claims likely overtaken
by the world (`stale-status`), and conflicts needing the user's pick
(`contradiction`). The daemon only bookkeeps; **you are the solver**,
with this session's model, tools, and user.

1. **Back up, then list.** `ling-mem export` first (one snapshot per
   solve session), then `ling-mem issues --format json` (or the
   `memory_issues` MCP tool). Empty → say so, done.
2. **Per item, gather evidence at solve time.** Fetch the rows
   (`ling-mem get <row_id>`). For `stale-status`: check the WORLD —
   `git log --oneline --since=<row date>` in the named repo, working
   tree, file existence. The row was written before the world moved;
   your evidence decides what's true now.
3. **Apply the confidence rule.** Evidence is conclusive AND every
   affected row is your own note (`from=derived`) → solve directly, no
   ask: one `memory_add` with `replace_ids` (CLI: `add --replace <id>`)
   writing current truth. Evidence is ambiguous, OR any affected row is
   user-voice (`from=user`) → **ask the user, ONE item per question**
   (AskUserQuestion on Claude Code; plain numbered options elsewhere) —
   never batch the whole queue into one wall of questions. User-voice
   fixes carry `user_directed:true` after their answer.
4. **Close as you go.** After each item:
   `ling-mem issue-resolve <id> --outcome resolved --note "<what you did>"`
   (or `memory_issue_resolve`). Not worth fixing → `--outcome dismissed`.
5. **Report one line per item** — `SOLVED <id> <what changed>` /
   `DISMISSED <id> <why>` — then a closing count.

### Chat-mode rules

The user is reading text in a conversation panel:

- Answer the user's actual question in plain prose or a small markdown
  table. If the user asked to list memory, run the recipe in
  *Listing & searching memory* above and render the result inline.
- For hands-on row-level CRUD, point the user at the daemon-served
  data browser at `127.0.0.1:9528` (run `ling-mem start` first).

## Memory hygiene — see it, solve it

**Hard rule, applies everywhere (live chat, per-turn capture, dream):**
whoever surfaces garbage owns it in that moment — **resolve it in the
same pass, don't defer**. There is no cleanup queue. Garbage in memory
poisons every future retrieval; "leave it for later" is how 7
word-count rows accumulate.

**Merge authority follows voice.** Your own notes (`from=derived` —
`built`/`fixed`/`tried`/`learned`) are your notebook: merge, rewrite,
retire freely, no prompt. Rows in the user's voice (`from=user` —
preference/decision/identity) change only with the user: ask first.
The daemon enforces this floor mechanically — a replace or content
rewrite of a `from=user` row is BLOCKED unless the write carries
`user_directed: true`, which you assert only when the user directed
the change: their current message states it as settled (a command
"update X to Y", a declaration "my X is now Y", a commitment "from
now on, X") or they just answered your ask. A hedged reflection ("X
feels about right to me") never qualifies — ask first.

**Status rows are perishable — supersede at write time.** A
status-bearing row ("in progress", "OPEN:", "not committed",
"shipped", "dormant") is a claim about the world, and the world moves.
When you capture a status change (shipped / fixed / dormant /
abandoned), search the subject first and write the new status
replacing the prior status row(s) on that subject (`replace_ids` over
MCP; `add --replace <id>` via CLI) — never leave "in progress" beside its
own outcome. Own-notes only; a user-voice predecessor follows the
merge law. The dream audit's review queue is the backstop for what
slips through — write-time supersede is the real fix.

| You see | Action |
|:---|:---|
| Exact dup (same fact, same type) | Delete the loser, keep the better-phrased row. No prompt. |
| Superseded / chain member, all derived ("impl not started" → "shipped") | Merge into one current-truth row. No prompt. |
| Reworded derived near-dup | Merge, keep the best phrasing. No prompt. |
| Old pure-event row ("committed X") | Retire it — fold into the state row it evidences, if one exists. |
| Contradiction touching a user-voice row | Don't pick silently. **Always ask.** |
| Secret (credential, token, key) | Delete on sight, any tier. |
| Judged episodic rows lingering past TTL | Run `ling-mem sweep` — it evicts exactly those, never un-judged rows. No prompt. |

**How to ask:** use whichever ask-user primitive your host gives you.

- **Claude Code** — call the `AskUserQuestion` tool. UI renders a
  structured choice card.
- **Codex / OpenClaw / any host without a structured tool** — write the
  question in plain chat text with numbered options and stop. The user
  replies on the next turn; you read their choice and finish the cleanup
  via `ling-mem add "..." --type ...` followed by `ling-mem delete
  <loser-id> --yes` for each loser.

When a merge (derived rows) or an AskUser-resolved conflict yields a
winner: write the winner first (`ling-mem add "<winner>" --type <t>
--from <f>`), then delete the losers (`ling-mem delete <loser-id>
--yes`). The CLI doesn't expose an atomic replace verb; the two-step
ordering (write before delete) keeps the worst-case window safe — a
concurrent recall either sees the old rows or both, never an empty hole
on the subject. (Over MCP/HTTP, use `replace_ids` on the add instead —
one atomic call.)

### What "not confident" looks like

- Two rows on the same subject with timestamps far apart → user's view may
  have changed. Ask.
- Two rows that are mostly the same but differ on a specific detail (e.g.
  one says "8 years old in 2026-05-21", another says "9 years old in
  2026-05-25") → time-stamped, may both be valid. Ask before merging.
- Rows that look like dups but have different `cwd` / `contexts` /
  `outcome` — they may apply to different scopes. Ask.

When in doubt, **ask**. Cheap. The cost of asking is one turn; the cost of
silently losing or mangling a fact is much higher.

### What automatic catches mechanically

- `insert_with_dedup` inside the binary rejects byte-identical
  `(content, type)` rows at write time. You don't need to handle that case.
- Cross-tier dedup (`add` handler): if you add to one table and an exact
  match exists in the other, the higher-tier row wins; metadata
  (contexts / tags) is merged into it. Also automatic.

Fuzzy "same fact, different wording" is **never mechanical** — it always
needs an LLM judgment + the rule above.

### Inline reconciliation

When recall hits include duplicates or conflicts, fix them:
`ling-mem delete <id>` near-dups (keep the best phrasing);
`ling-mem edit <id>` or `delete` on conflicts after asking the user.
Get ids via `ling-mem search "<phrase>" --format json | jq -r '.[] | "\(.id)\t\(.content)"'`.

## Type taxonomy (reference)

The `type` enum is `fact | preference | decision | tried | fixed |
learned | built` — but **only four should be emitted by default**.

| Type | Use | When to emit |
|:---|:---|:---|
| `fact` | Stable user truth (identity, goals, vision) | Cross-project, durable indefinitely |
| `preference` | Cross-project behavioral rule for the agent | Commitment language required |
| `decision` | A choice plus its reasoning | Reasoning is the retrieval value |
| `learned` | Cross-project tech gotcha | Reusable across projects |

`tried` / `fixed` / `built` are deprecated — emit only for
trajectory-level patterns or named shippable artifacts tied to user
identity.

## Contexts and tags

- **`contexts`** — hierarchical scope (1–3 typical, primary filter).
  - `cross-project` — retrieves in any session.
  - `code/linggen`, `music/piano`, `trip-japan-2026` — domain scopes.
  - **Don't** add `project/<name>` for new writes. Project-internal
    facts get dropped — the agent reads the project's own files next
    time. Legacy `project/<name>` rows still retrieve.
- **`tags`** — free-form metadata (0–5 typical, prefix convention).
  - `intent:goal`, `topic:networking`, `person:maria`.

## Data browser

Row-level CRUD (filter, edit-in-place, batch delete) lives at
`http://127.0.0.1:9528` when the daemon is running. Direct the user
there for hands-on cleanup. Run `ling-mem start` if not already
running.

## Updates

`ling-mem start` (and `restart`) returns JSON that may include an
`update` field — a cached probe of `linggen/linggen-memory` GitHub
releases (24h TTL, no extra network calls beyond the first).

When that JSON contains `"update": {"available": true, ...}`, surface
it to the user once at the top of your reply, e.g.:

> *"ling-mem upgrade available: 0.2.1 → 0.3.0 — `<notes_summary>`. Upgrade now?"*

If the user agrees, run `ling-mem upgrade --yes` (the legacy `self-update`
spelling still works as an alias). The CLI stops the daemon, verifies
the SHA-256 of the downloaded tarball, swaps the binary atomically
(keeping the prior version at `bin/linggen.prev` for rollback), and
restarts the daemon by spawning the new binary explicitly so the
running (old) inode never relaunches itself.

Ad-hoc check (no swap): `ling-mem upgrade --check`. Useful when the
user asks "am I up to date?" without wanting to upgrade. The same
cached probe is also surfaced in `ling-mem status` output, so callers
that already poll `status` don't need a separate network call.

Don't auto-upgrade silently — schema or behavior may change between
versions, and the user should know what they're accepting.

---

## Install

Install from your agent's own marketplace — it manages updates and, on
Claude Code / Codex, the per-turn recall hook. Pick **one** channel per host:

```text
Claude Code   /plugin marketplace add linggen/linggen-memory
              /plugin install linggen@linggen-memory
Codex         codex plugin marketplace add linggen/linggen-memory
              codex plugin add linggen@linggen-memory
OpenClaw      clawhub install linggen
Any agent     npx skills add linggen/linggen-memory@linggen
Linggen       Settings → Skills → linggen   (in-app)
```

The `ling-mem` binary is fetched automatically on first use (pinned,
SHA-256 verified). To install just the binary manually (Apple Silicon /
Linux x86_64+aarch64), run the installer that ships in this bundle:

```bash
bash scripts/install-bin.sh --version '^1'
```

(Path relative to this skill's directory, like every other script here.)

The skill works in Claude Code, Codex, OpenClaw, Linggen, or standalone —
same daemon, same database, same semantics across all hosts. Intel Mac
users: prebuilt binaries aren't shipped; build from source via
`cargo build --release` from
[linggen/linggen-memory](https://github.com/linggen/linggen-memory).

Source: [github.com/linggen/linggen-memory](https://github.com/linggen/linggen-memory) · [linggen.dev](https://linggen.dev)

## Browser control (via the same MCP server)

The `linggen` MCP server also exposes the user's own Chrome (through the
linggen-browser extension) — one **visible** controlled tab:

- `browser_navigate` / `browser_read_page` (accessibility tree with `[nN]`
  refs) / `browser_click` / `browser_type` / `browser_key` /
  `browser_scroll` / `browser_screenshot` / `browser_wait` /
  `browser_tabs` / `browser_read_console`. Work a read → act → re-read
  loop; target by ref.
- Mutating actions may pause on a **permission prompt in the browser** —
  the user approves each new site once (or always); payment, credentials,
  deletes, and posting always confirm. A `not_permitted` error means the
  user declined: stop, don't retry.
- `x_search` / `x_targets` / `x_following` / `x_whotofollow` / `x_own`
  return structured JSON from the user's logged-in x.com session — no
  API keys.
- A `no_bridge` error means the linggen-browser extension isn't connected;
  ask the user to install or enable it.
- If the `linggen` MCP server itself is unreachable (nothing listening on
  `127.0.0.1:9527`), the engine may still be installing in the background
  (plugin channels; progress in `~/.linggen/engine-install.log`) — wait and
  retry. If the `ling` binary is genuinely absent, run the engine install
  from the First-use section and tell the user (one-time, ~100MB). Memory
  keeps working via the `ling-mem` CLI fallback throughout.
