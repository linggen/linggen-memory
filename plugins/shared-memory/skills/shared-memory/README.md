# shared-memory

**Persistent memory for AI assistants. Local, semantic, typed.**

A single-binary memory layer that remembers useful facts about you and your work across every session, every tool, every project. Works in Claude Code, OpenClaw, Linggen, or any agent that can shell out to a CLI.

## What it does

- **Auto-recall on every prompt.** A `UserPromptSubmit` hook runs a semantic search over your stored facts and injects the top matches as context — no manual tool call required. Relevant preferences and past decisions land in the agent's view automatically.
- **Semantic retrieval.** 1024-dim embeddings via `Qwen3-Embedding-0.6B` (multilingual). Find "berth calibration" by asking about "dock alignment."
- **Typed facts.** `fact`, `preference`, `decision`, `learned`, plus trajectory-level `tried`, `fixed`, `built`. Searches and filters operate on these tags.
- **Forgetting is first-class.** Delete by id, forget by filter — refuses empty filters as a guardrail.
- **Local-first storage.** The memory store is on disk in `~/.linggen/memory/` (LanceDB) — no cloud sync, no telemetry. Retrieved facts do enter your agent's prompt context on each turn, so they reach whichever LLM you've configured.
- **Self-updating.** `ling-mem upgrade --check` reports the latest release; `--yes` swaps the binary atomically. (`self-update` still works as an alias.)

## Quick start

Install from your agent's marketplace (pick one per host): Claude Code
`/plugin install shared-memory@linggen-memory`, Codex `codex plugin add
shared-memory@linggen-memory`, OpenClaw `clawhub install ling-mem`, any agent
`npx skills add linggen/linggen-memory@shared-memory`. The `ling-mem` binary
auto-installs on first use.

```bash
# Add a fact
ling-mem add "prefers concise replies, no hedging" --type preference --from user

# Semantic search
ling-mem search "how do I format logs" --limit 5 --format json

# List by filter
ling-mem list --type preference --limit 20

# Forget a specific row
ling-mem delete <id>
```

## How each host uses it

| Host | Integration |
|:-----|:------------|
| Claude Code | SKILL.md + a `UserPromptSubmit` hook (`hooks/recall.sh`). Hook auto-injects relevant memories every prompt; agent calls the CLI for ad-hoc lookups. |
| Codex / OpenClaw | Standard SKILL.md skill. Agent shells out via the CLI for every memory operation. |
| Linggen | This skill is loaded the same way (CLI via `Bash`). Separately, the Linggen engine ships built-in `Memory_query` / `Memory_write` tools wired to the same daemon for its own auto-recall + dream paths — same store, same semantics, no skill round-trip needed inside the engine. |
| Standalone | Any script shells out: `ling-mem search "query" --format json` |

The auto-detect installer (`install.sh`) places the skill into whichever host runtimes are present (`~/.claude/skills/`, `~/.openclaw/skills/`, `~/.linggen/skills/`).

## Platforms

- macOS Apple Silicon (M1+) — prebuilt binary
- Linux x86_64 / aarch64 — prebuilt binary

Intel Mac: prebuilt binaries not provided. Build from source with `cargo build --release` from [linggen/linggen-memory](https://github.com/linggen/linggen-memory).

## Why this exists

`CLAUDE.md` and equivalent project files handle static rules but don't grow with the user. MCP memory servers require a long-running process with JSON-RPC mediation, and aren't auto-injected into the agent's context — they're tools the agent has to remember to call. `shared-memory` fits between: a single binary (`ling-mem`) you shell out to, with semantic retrieval, typed facts, and explicit forget operations. The recall hook auto-injects relevant context on every prompt, so the agent doesn't have to remember to query.

## What's stored

- Stored: durable signal you (or the agent on your behalf) add via `ling-mem add` (or, inside the Linggen engine, the equivalent built-in `Memory_write` tool). Indexed in `~/.linggen/memory/memory.lancedb/` (two tables: `semantic` for promoted core/long-term rows, `episodic` for recently-encoded staging).
- Not stored: session transcripts, code you don't explicitly save, anything not added through the CLI.

## Links

- **Linggen platform: [linggen.dev](https://linggen.dev)** · [github.com/linggen/linggen](https://github.com/linggen/linggen)
- Source + binary releases: [github.com/linggen/linggen-memory](https://github.com/linggen/linggen-memory)
- Skill source: [github.com/linggen/skills/tree/main/shared-memory](https://github.com/linggen/skills/tree/main/shared-memory)
- Issues: [github.com/linggen/linggen-memory/issues](https://github.com/linggen/linggen-memory/issues)

## License

- **Skill code** (SKILL.md, install scripts, hooks): Apache 2.0 — see `LICENSE`.
- **`ling-mem` daemon binary**: MIT — built from [linggen/linggen-memory](https://github.com/linggen/linggen-memory).
