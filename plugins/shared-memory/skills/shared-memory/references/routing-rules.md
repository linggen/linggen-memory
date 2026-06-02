# Routing rules — what to save, where, and how

This file is the canonical reference for save decisions. Both
`SKILL.md` (chat / dashboard / scan modes) and the dream `mission.md`
(nightly extractor) Read this when making save / dedup choices.

The principle: **memory grows with genuinely durable signal; drift
gets reconciled.** Net value goes up over time; row count alone is not
the measure.

## The durability test — both questions must pass

> 1. Would this still be true 6 months from now, in a totally different project?
> 2. Would a future agent, starting cold, make better predictions about what this user wants and how they work because this memory exists?

If both pass and the fact is about the **person** → core
(`ling-mem add ... --tier core`).
If both pass and it's not about the person (goal, preference, decision,
cross-project learning) → long-term (`tier=semantic`, the default) with
`contexts: ["cross-project"]`.
If either answer is NO → **skip**. The candidate is not memory.
**Memory does not write to project files** (`<project>/AGENTS.md`,
`CLAUDE.md`, source, docs); those are user-curated. If the user wants
project-internal knowledge captured there, that's a hand-edit they
make to their own file.

That covers everything: project-internal implementation detail,
activity / session-arc, meta-feedback about the memory skill or Linggen
tooling — all skipped.

## The three save rules

### 1. Don't memorize what lives in workspace files

Code, configs, READMEs, project docs — the agent reads them when it
needs them. Memory storing the same content creates a stale copy.

> *"In repo1, the planner module exposes a facade that returns a
> context object per tick"* — **skip.** The agent will read the planner
> sources next time it matters. Memory does not auto-write to the
> project's `AGENTS.md` either; the user authors that file by hand if
> they want the rule there.

### 2. User-stated preferences need a confidence gate

- **Save** — user is correcting agent behavior with commitment language
  and cross-project reach:
  > *"I want the agent to always keep UI and server aligned, don't leave
  > one half-done into the next task."*

  Record as `preference`.

- **Skip** — single architectural call, true today and possibly reversed
  next month:
  > *"We should decouple layer 1 from the core engine."*

  Belongs in design notes / PR description / project AGENTS.md, not
  cross-project memory.

- **Synthesize at retrieval, not extraction** — when many similar
  utterances accumulate (*"split this module"*, *"factor out Y"*,
  *"decouple X"*), the extractor still appends each one as its own row.
  It does **not** mint a higher-order rule. Synthesis happens live: when
  retrieval pulls several rows on the same theme, the agent reconciles
  in prose — the user sees the generalization and corrects it.

### 3. User-only knowledge — record, then maintain

Facts only the user can supply: life context, history, relationships,
dates, equipment, the people and animals around them.

- **Stamp ages relative to a date.**
  > *"I have a 3-year-old cat"* → save as *"User has a cat, age 3 as of
  > 2026-04-27"*, not *"the cat is 3 years old"*. Without the as-of
  > date, "3 years old" silently rots into "still 3 years old" forever.

  Record only what the user said. Don't invent a name, breed, or any
  other detail to make the entry feel complete — fabricated specifics
  mislead every future retrieval.

- **Append at extraction; reconcile at retrieval.** When the user
  revises a fact, append a new timestamped row — don't overwrite the
  existing one. Reconciliation happens at read time: when multiple
  matching rows surface, the agent merges them in the response,
  ordered by timestamp, and the user sees the synthesis live.

  > Stored: *"User has a cat"* (2024). Later: *"When I relocated, I
  > left the cat with a friend"* (2026). Retrieval surfaces both; the
  > agent renders *"From memory: you had a cat that you left with a
  > friend during your 2026 relocation."*

  Stale rows are removed only by an explicit user instruction
  (*"forget that I have a cat"*). The extractor never picks a winner
  and never marks one row as superseding another — that's destructive
  judgment reserved for the live agent + user.

## What NOT to save

| ❌ Wrong | Why | What to do |
|:---|:---|:---|
| `"User is leading X feature"` | Activity, not identity | Skip. Git log records it. |
| `"Agent fixed an issue in src/foo.rs"` | Bug fix, not cross-project wisdom | Skip. Commit message records it. |
| `"In repo1, function X does Y"` | Project-internal implementation detail | Skip. The agent will read the source next time. Memory does not write to `AGENTS.md`. |
| `"Always run npm build after UI changes"` | Project convention | Skip. If the user wants this rule in `<project>/CLAUDE.md`, they hand-edit it themselves. |
| `"User decided the dashboard wording should be 'Scan Today'"` | Meta-feedback about the memory skill itself | Skip. Code change is the artifact. |
| Two candidates restating the same fact | Dedup failure | Search + update the clearer one (mechanical rephrase only) |
| Inferred preferences (*"user seems to prefer Y"*) | No explicit statement | Skip — ask if it matters |
| Single architectural opinion (*"we should decouple X"*) | Rot-prone | Skip. Memory does not author project files; user-curated `AGENTS.md` is the right home if anywhere. |
| The user's API key / password / git remote with embedded PAT | **Never store secrets at any layer** | Skip the credential. Memory does not write the gotcha to a project file either — the user hand-edits if they want it there. |

Rule of thumb: if the entry reads as *"true about this person in any
context"*, it's right. If it reads as *"what they worked on this week"*
or *"how this specific project works"*, **drop it**. Memory does not
write to project files; the user authors those by hand.

## Maintenance — fix when you see it; ask when unsure

The agent — whether in live chat, dream, or the per-turn encoder
subagent — is **always near a user**. Memory hygiene is a hard floor:
when you see drift, fix it in the same pass. Don't accumulate it. The
only split is whether you ask first or act silently.

### Mechanical maintenance — fire-and-forget

Pure rule application. No LLM judgment, no asking.

| Operation | Where | Why |
|:---|:---|:---|
| Append a new row | Anywhere | Pure additive |
| Exact-content dedup at write (binary `insert_with_dedup` rejects identical content) | Binary | Pure equality check |
| Cross-tier exact-content dedup on `add` (HTTP path) | Daemon | Equality check + tier-rank merge |
| Extend `contexts[]` / `tags[]` from new evidence | Anywhere | Array union |
| Retire past-TTL episodic via the `dream` worklist (promote / delete) | `dream` | Engine-selected worklist, terminal decision per row |

### Semantic maintenance — silent when confident, AskUser when not

These need LLM judgment, available on every memory pass (live chat,
per-turn capture, the `dream` consolidation). The line between silent
and ask-first is **confidence**, not "live vs offline" — except that a
headless `dream` (no user present) can't ask, so it **defers** a
contradiction (leaves the candidate in episodic) rather than guessing:

| Operation | Silent if… | Ask if… |
|:---|:---|:---|
| Dedup two rows that mean the same thing | Same value, near-identical phrasing, no scope difference (same `contexts` / `cwd`) | Different scopes, different timestamps, or any value drift between them |
| Resolve a contradiction (same subject, incompatible values) | **Never silent.** Always ask. | Always |
| Generalize utterances into a "user always X" rule | **Never.** Append individual utterances; live retrieval surfaces patterns. | — |
| Merge distinct facts into one synthesized story | **Never.** They're distinct; append both. | — |

**How to ask** depends on the host (`SKILL.md` → *Memory hygiene*):
Linggen's `AskUser` engine tool, Claude Code's `AskUserQuestion`, or
plain chat text + numbered options when neither exists.

**Bulk forget by filter** is user-initiated only. The model can iterate
`search` → `delete` for small sets when explicitly asked.

### Hard rules — what extraction must NEVER do

- Never delete a `semantic` row **silently** to resolve a contradiction.
  Ask first via AskUser; on the user's pick, write the winner
  (`ling-mem add "<winner>" --type ... --from ...`) then delete each
  loser (`ling-mem delete <id> --yes`). Silent deletion is the floor
  violation.
- Never write a contradicting pair as separate atoms hoping live recall
  resolves it later. That's drift accumulation — the cost we're trying
  to stop paying. Ask now.
- Never merge two distinct rows into one synthesized story. If they're
  distinct facts (not phrasings of one fact), append both — they're
  not duplicates.
- Never mint a "user always X" generalization across rows. Append the
  individual utterances; live retrieval surfaces the pattern.

## Routing summary by tier

When a candidate emerges, route to one of two tiers or drop it.
**Memory does not write to project files** (`<project>/AGENTS.md`,
`CLAUDE.md`, source, docs); those are user-curated.

| tier | When | Action |
|:---|:---|:---|
| `core` | Universal-about-person OR cross-project behavioral rule for the agent | `ling-mem add "..." --tier core --type <fact\|preference> ...` |
| `semantic` (default) | Cross-project intent / decision / preference / learning | `ling-mem add "..." --type <type> ...` (omit `--tier`) |
| (skip) | Project-internal implementation detail / activity / session-arc / meta-feedback | Drop. The agent reads code or user-authored project files when needed. |

Most candidates skip. The core tier grows slowly by design — a noisy
`core` set pollutes every session's prompt. Long-term stays dense;
we'd rather miss 3 saves than force the user to curate 30 low-signal
rows.

## Contexts and tags

- **`contexts`** — primary scope dimension. 1–3 typical.
  - `cross-project` — retrieves in any session.
  - `code/linggen`, `music/piano`, `trip-japan-2026` — domain scopes.
  - **`project/<name>`** — **legacy only**, do not write new rows with
    this context. Project-internal facts get dropped under the current
    rules; they're not memory. Older rows still retrieve normally.

- **`tags`** — free-form metadata (0–5 typical, prefix convention).
  - `intent:goal`, `topic:networking`, `person:maria`.

## Outcome field — only for action-flavored types

`outcome: positive | negative | neutral` is meaningful **only** for
`tried` / `fixed` / `decision`. Omit entirely for `fact` /
`preference` / `learned` / `built` — setting `outcome: neutral` as a
placeholder for those types is visual noise on the dashboard and a
sign of extractor drift.
