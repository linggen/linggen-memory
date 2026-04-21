---
type: spec
audience: product — features, user-facing behavior, scenarios
---

# linggen-memory — product spec

## What it is

**A semantic memory store for AI assistants.** Distributed as a single-binary CLI (`ling-mem`) with an optional web UI. Backs the memory skill for Linggen, and works equally well when invoked from Claude Code or any tool that can shell out.

The core product claim:

> An assistant that remembers useful things about you and your work, across every session, every tool, and every project.

Not a chat log. Not an index of your code. A curated store of facts that *help the assistant work better next time*.

## Who it's for

- **Solo users** who want their AI assistant to actually remember them across sessions.
- **Assistant authors** who want a pluggable memory backend and don't want to build their own vector store.
- **Claude Code users** who want cross-session recall in a tool that doesn't ship memory natively.

Intentionally *not* for: team-wide shared knowledge bases, production-scale document search, multi-tenant SaaS. Those are different products.

## Principles

1. **Every fact must have a retrieval trigger.** If you can't imagine when a future session would recall it, don't store it.
2. **Markdown is the identity layer.** Universal facts about the user live in plain markdown files that any tool can read. LanceDB is the activity / fact layer — richer, queryable, but never the only copy.
3. **Semantic search by default.** Everything stored gets embedded; retrieval is vector-similarity + metadata filter.
4. **CLI is the primary interface.** Any model in any tool can call `ling-mem` via Bash. Tool-namespace dispatch in Linggen is sugar on top.
5. **Humans can read and edit memory.** The skill's webpage is a row browser + editor that talks to `ling-mem` on the user's behalf; nightly markdown export feeds git/backup.
6. **Forgetting is a first-class operation.** `archive`, `delete`, `forget` are as important as `add`.

## Shape of a fact

Twelve fields (see `tech-spec.md` for the wire format). The user-facing mental model:

- **What** — the fact text itself, self-contained (including any scoping conditions).
- **Where it applies** — contexts (scope tags, e.g. `code/linggen`, `music/piano`).
- **Everything else about it** — free-form tags (topic, intent, people, mood).
- **What kind of fact** — one of seven canonical types.
- **Who said/did it** — user / agent / derived.
- **Result if applicable** — worked / failed / neutral.
- **When it happened** — separate from when it was added to memory.
- **Where it was captured** — cwd at extraction time.
- **Which session it came from** — escape hatch if the fact turns out ambiguous.

## The seven fact types

Memory is only useful when the fact has a reason to resurface later. Seven canonical categories:

| Type | Useful because | Retrieval trigger |
|:--|:--|:--|
| **fact** | Agent adapts to who you are | Every session |
| **preference** | Agent changes how it works | Any task where the rule applies |
| **decision** | Informs analogous future choices | A similar architectural / approach decision arises |
| **tried** (with `outcome`) | Prevents dead-end repeats | About to try something similar |
| **fixed** (with `outcome`) | Avoids re-solving the same bug | A future bug with overlapping symptoms |
| **learned** | Saves tool / env rediscovery | Same tool or environment encountered again |
| **built** | Provides project history | Asked "what's shipped for X?" |

No `activity` catch-all. Weekly-status-style entries (the drift category in prior memory systems) have no home here by design.

## The three retrieval modes

1. **Active injection (push).** When a session starts or a turn arrives, relevant facts are auto-attached to the prompt — scope-filtered, top-k by vector similarity. The assistant doesn't need to ask.
2. **Tool (pull).** `ling-mem search <query>` — invoked by the model when it decides memory would help.
3. **Browse (human).** The skill's webpage — filter, sort, star, bulk-archive, bulk-forget. Ships with the Linggen memory skill (`skills/memory/ui/`), not the `ling-mem` binary. Also nightly markdown export for git-sync and audit.

## The four forgetting mechanisms

1. **Time-decay** — old activity-flavored facts are archived automatically after N days.
2. **Access-decay** — facts unused in 90+ days drop in priority / archive.
3. **Durability filter at write time** — the extraction pipeline refuses ephemeral / project-specific facts from entering.
4. **Explicit user forget** — `ling-mem forget --context trip-japan-2026` bulk-removes when a phase of life is over.

## User-facing CLI

```bash
ling-mem add "prefers concise replies" --type preference --from user
ling-mem search "dock calibration" --context code/sanji --limit 5
ling-mem list --type fixed --since 2026-01-01
ling-mem update <id> --tags "intent:learn,topic:rust"
ling-mem archive <id>
ling-mem delete <id>
ling-mem forget --context code/sanji --older-than 30d

# Extraction
ling-mem collect --since 2026-04-01
ling-mem extract <session-path> --source cc
```

Web UI: provided by the Linggen memory skill, not the `ling-mem` binary.
See the skill's `skills/memory/ui/` in the main Linggen repo.

Default output: NDJSON on stdout. `--format=text` for human-readable. Errors on stderr with non-zero exit.

## How it fits with Linggen

- Installed as a Linggen **skill** that advertises `provides: [memory]`.
- The skill ships its own static HTML/JS at `skills/memory/ui/`. Linggen's `app: launcher: web` opens it in-app; the page calls `ling-mem` via Linggen's bash or `Memory.*` tool dispatch.
- Linggen's core routes `Memory.*` tool calls to the skill, which shells out to `ling-mem`.
- Separately, the skill's `SKILL.md` body is readable by any Claude Code session — same binary, same CLI, just invoked via Bash.

See `tech-spec.md` for the repo layout, CLI contract, schema, and release process.

## Out of scope for v0.1 (`ling-mem` binary)

- Soft-delete / `archive` (deferred until the hard-delete behavior proves insufficient)
- HTTP daemon mode (`serve`) — the skill webpage lives outside the binary, so the binary stays CLI-only
- Multi-provider memory skills (Linggen picks one active provider; switching is user-initiated)
- Team-shared memory across multiple users
- Sync protocol other than "nightly markdown export → git"

Web UI details are out-of-scope for *this repo*; they're tracked on the skill wrapper in the main Linggen repo.

## Future directions

- Temporal tracking — how facts change over time (inspired by Zep)
- Memory health scoring — auto-propose cleanup of degraded entries (inspired by OpenClaw)
- Access-weighted ranking surfacing pinned-important items above time-decayed ones
- P2P cross-device sync via Linggen's WebRTC transport
