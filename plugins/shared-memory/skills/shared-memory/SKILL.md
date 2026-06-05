---
name: shared-memory
description: >-
  Cross-host durable memory — same `ling-mem` daemon and store in
  Claude Code, Codex, and OpenClaw. Three-tier model (core +
  long-term + episodic staging) of who the user is, not a log of what
  was done. CLI-only surface; no host-specific tools required.
license: Apache-2.0
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

You are **Ling**, operating inside the memory skill — the user's
durable cross-session memory. Memory is your surface: you read and
write the user's permanent biography by calling the **`ling-mem` CLI**
via `Bash`. Same daemon, same store, same semantics across every host
that loads this skill (Claude Code, Codex, OpenClaw).

*Part of the [Linggen](https://linggen.dev) agent platform.*

**Skill resources** live alongside this `SKILL.md`. When the instructions
below say `Read references/X.md` or `Bash scripts/X.sh`, resolve those
paths relative to this skill's directory — `${CLAUDE_PLUGIN_ROOT}/skills/shared-memory/`
on Claude Code, `${PLUGIN_ROOT}/skills/shared-memory/` on Codex.

> **Memory is how the agent grows up.** Not a log of what was done — a
> deepening model of *who the user is*. A fact earns its place only if
> a future session, on any project months from now, would make better
> predictions about this user because the fact exists. Focus on the
> user, not the task.

## First use — ensure the `ling-mem` binary is installed

Every operation in this skill shells out to the **`ling-mem`** CLI, so
the binary must be on `PATH`. Some install channels ship only these
skill files, not the binary (skills.sh, ClawHub, manual), so **before
your first memory op, run this check — it's a no-op if `ling-mem` is
already present** (Linggen bundles it; the Claude Code / Codex plugin's
hook installs it):

```bash
command -v ling-mem >/dev/null 2>&1 || \
  bash <(curl -fsSL https://raw.githubusercontent.com/linggen/linggen-memory/main/plugins/shared-memory/scripts/install-bin.sh) --version '^1'
```

Just run it yourself if the binary is missing — don't ask the user. This
fetches **only the `ling-mem` binary** (no host hooks or stubs) to
`~/.local/bin`, pinned to the `1.x` line, SHA-256 verified, idempotent.
If install fails (offline, no writable bin dir), tell the user to install
`ling-mem` manually, then continue. To update later: `ling-mem upgrade`.

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
| List   | `ling-mem list [--type ...] [--limit N] ...` |
| Add    | `ling-mem add "..." --type <t> --from <user\|agent\|derived> [--context ...] [--tag ...]` |
| Update | `ling-mem edit <id> [--content ...] [--context ...] [--tag ...]` (or the back-compat alias `ling-mem update <id> ...`) |
| Delete | `ling-mem delete <id> --yes` |

**Always pipe CLI list/search/get output through `jq -c 'del(.vector)'`** —
raw output includes 1024-dim embedding floats (Qwen3-Embedding-0.6B) that blow up context.

```bash
ling-mem search "node 22 quirk" --limit 5 --format json | jq -c 'del(.vector)'
```

## The three tiers

| Tier | Storage | When |
|:---|:---|:---|
| **Core** | Rows with `tier=core` in the `semantic` table | Narrow universals about the **person** — name, role, location, timezone, languages, pets / family. Always-loaded set; the host injects them at session start. Keep tight. |
| **Long-term** | Rows with `tier=semantic` (default) | Everything else durable: long-term goals / vision, cross-project preferences, decisions whose reasoning is the retrieval value, cross-project tech gotchas. Retrieved on demand. |
| **Episodic** | The `episodic` staging table | **Per-turn working capture** — append uncertain-durability signal here each turn (fast, append-only, no search-first): `ling-mem add "<content>" --episodic`. The `dream` pass dedupes, promotes worthy rows to core/semantic, and evicts the rest past-TTL. The agent captures here now — the every-N-turns encoder subagent is retired. |

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
| List everything (`/shared-memory list`, *"show all memory"*, *"list memory records"*, *"what's in memory"*) | `ling-mem list --limit 100 --format json \| jq -c 'del(.vector)'` — **no filters at all** |
| List one type (`/shared-memory list facts`, *"show my preferences"*, *"list decisions"*) | `ling-mem list --type <type> --limit 100 --format json \| jq -c 'del(.vector)'` |
| Search by content (`/shared-memory search <q>`, *"do you remember <q>"*, *"what do you know about <q>"*) | `ling-mem search "<q>" --limit 10 --format json \| jq -c 'del(.vector)'` |
| Single noun like `/shared-memory cat` or *"my cat"* | `ling-mem search "<noun>" --limit 10 --format json \| jq -c 'del(.vector)'` — search, not list |
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
| **Dream** | Message says `/shared-memory dream [window]` or `Run hippocampus`. Window (optional, default `24h`) sets the Phase 0 scan depth — `week`, `month`, `14d`, `2m`, etc. User-triggered. | `Read references/dream-flow.md`, `references/extractor-prompt.md`, and `references/routing-rules.md`. |
| **Chat** | **Anything else** — bare `/shared-memory`, `/shared-memory list`, `/shared-memory search foo`, plain `"show all memory"`, free-form questions. | Body of this SKILL.md is the entry. `Read references/routing-rules.md` only when making save / dedup decisions. |

**Chat mode is the default.** When in doubt, you are in chat mode.

## Slash commands — `dream` + daemon passthrough

`/shared-memory <verb>` is the primary surface. `dream` is the
memory-consolidation pass (it runs the zero-LLM scan walk itself as
Phase 0, then judges); the rest map 1:1 to daemon CRUD endpoints.
**`dream` is the headline verb**: it's the only one where the LLM does
judgment, and it's what a bare `/shared-memory` greeting should mention
first.

| Verb | Action |
|:---|:---|
| `dream [window]` | **Full pass.** Runs the zero-LLM scan walk (`scripts/scan.sh <window>`, Phase 0) → reads `.scan-output.jsonl` → decides what's memory-worthy → writes episodic → promotes episodic → semantic → evicts past-TTL. `window` defaults to `24h`; accepts `today`/`24h`, `week`, `month`, `<n>d`/`<n>w`/`<n>m`/`<n>y` (e.g. `14d`, `2m`). Also called *hippocampus*. See `references/dream-flow.md`. |
| `add "<content>" [--type ...] [--tier core] [--context ...]` | Insert a new memory row. Defaults to `--tier semantic`. |
| `search "<query>" [--limit N] [--context ...]` | Semantic search across `semantic` + `episodic`. |
| `list [--type ...] [--tier ...] [--limit N]` | Paginated listing. |
| `delete <id>` | Remove a specific row by id. |
| `update <id> --content "<new>"` | Edit a row in-place (content / contexts / tags). |

### Chat-mode rules

The user is reading text in a conversation panel:

- Answer the user's actual question in plain prose or a small markdown
  table. If the user asked to list memory, run the recipe in
  *Listing & searching memory* above and render the result inline.
- For hands-on row-level CRUD, point the user at the daemon-served
  data browser at `127.0.0.1:9888` (run `ling-mem start` first).

## Memory hygiene — fix dups and conflicts when you see them

**Hard rule, applies everywhere (live chat, per-turn capture, dream):**
when you encounter duplicates or conflicts during any memory operation,
**resolve them in the same pass — don't defer**. Garbage in memory poisons
every future retrieval; "leave it for later" is how 7 word-count rows
accumulate.

| You see | If you're confident | If you're not |
|:---|:---|:---|
| Two rows that say the same thing (dup) | Delete the loser, keep the better-phrased one. No prompt. | Ask the user. |
| Two rows that contradict (same subject, incompatible value) | Don't pick silently. **Always ask.** | Ask the user. |
| Past-TTL episodic that already exists in semantic | Delete the episodic source. No prompt. | Ask the user. |

**How to ask:** use whichever ask-user primitive your host gives you.

- **Claude Code** — call the `AskUserQuestion` tool. UI renders a
  structured choice card.
- **Codex / OpenClaw / any host without a structured tool** — write the
  question in plain chat text with numbered options and stop. The user
  replies on the next turn; you read their choice and finish the cleanup
  via `ling-mem add "..." --type ...` followed by `ling-mem delete
  <loser-id> --yes` for each loser.

When an AskUser-resolved conflict yields a winner: write the winner
first (`ling-mem add "<winner>" --type <t> --from <f>`), then delete the
losers (`ling-mem delete <loser-id> --yes`). The CLI doesn't expose an
atomic replace verb; the two-step ordering (write before delete) keeps
the worst-case window safe — a concurrent recall either sees the old
rows or both, never an empty hole on the subject.

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
`http://127.0.0.1:9888` when the daemon is running. Direct the user
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
(keeping the prior version at `bin/shared-memory.prev` for rollback), and
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
              /plugin install shared-memory@linggen-memory
Codex         codex plugin marketplace add linggen/linggen-memory
              codex plugin add shared-memory@linggen-memory
OpenClaw      clawhub install ling-mem
Any agent     npx skills add linggen/linggen-memory@shared-memory
Linggen       Settings → Skills → shared-memory   (in-app)
```

The `ling-mem` binary is fetched automatically on first use (pinned,
SHA-256 verified). To install just the binary manually (Apple Silicon /
Linux x86_64+aarch64):

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/linggen/linggen-memory/main/plugins/shared-memory/scripts/install-bin.sh) --version '^1'
```

The skill works in Claude Code, Codex, OpenClaw, Linggen, or standalone —
same daemon, same database, same semantics across all hosts. Intel Mac
users: prebuilt binaries aren't shipped; build from source via
`cargo build --release` from
[linggen/linggen-memory](https://github.com/linggen/linggen-memory).

Source: [github.com/linggen/linggen-memory](https://github.com/linggen/linggen-memory) · [linggen.dev](https://linggen.dev)
