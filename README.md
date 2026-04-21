<p align="center">
  <img src="webui/public/logo.svg" width="120" alt="linggen-memory logo">
</p>

# linggen-memory

**A semantic memory store for AI assistants.**

`ling-mem` is a single-binary CLI + optional web UI that remembers useful facts about you and your work across every session, every tool, and every project. LanceDB-backed, local-first, markdown-native where it counts.

Built as the default memory skill for [Linggen](https://github.com/linggen/linggen); works equally well invoked from Claude Code or any tool that can shell out.

> 🔧 **Status: v0.1 in active development on `memory-refactor`.** The main branch reflects the archived code-indexing tool this project evolved from. See the `v0-legacy` tag for that snapshot.

---

## What it does

- **Remembers across sessions.** Facts about who you are, how you prefer to work, what you've tried, what worked, what didn't.
- **Semantic retrieval.** Everything stored gets embedded (384-dim via `all-MiniLM-L6-v2`). Find "berth calibration" by asking about "dock alignment."
- **Typed facts.** Seven categories — `fact / preference / decision / tried / fixed / learned / built` — each with a clear retrieval trigger.
- **Forgetting is first-class.** `archive`, `delete`, `forget` by filter. Time-decay and access-decay automatic.
- **Three ways to use it:**
  - As a **Linggen skill** — web app UI + `Memory.*` tool dispatch in the agent.
  - As a **Claude Code skill** — SKILL.md body, model calls the CLI via Bash.
  - **Standalone** — any script or tool can shell out to `ling-mem`.

See `doc/product-spec.md` for the full product story and `doc/tech-spec.md` for the implementation contract.

---

## Quick look

```bash
# Add a fact
ling-mem add "prefers concise replies, no hedging" \
  --type preference --from user

# Semantic search
ling-mem search "how do I format logs in dev" \
  --context code/linggen --limit 5

# Browse by filter
ling-mem list --type fixed --since 2026-01-01 --format=table

# Forget a finished project
ling-mem forget --context trip-japan-2026 --yes
```

Default output is NDJSON on stdout — any model / script / shell can parse it.

---

## Install (v0.1 — in development)

Currently no prebuilt releases. To build from source on the `memory-refactor` branch:

```bash
git clone https://github.com/linggen/linggen-memory
cd linggen-memory
git checkout memory-refactor
cargo build --release
./target/release/ling-mem --help
```

Prebuilt binaries for macOS (Intel + Apple Silicon) and Linux (x86_64 + aarch64) land once v0.1 tagging begins. See `doc/tech-spec.md` → *Release process*.

---

## Layout

```
linggen-memory/
├── Cargo.toml          # single crate
├── src/                # all Rust code
├── webui/              # Vite + TS web UI (Phase 8 rebuilds)
├── doc/
│   ├── product-spec.md # features, user-facing behavior, scenarios
│   └── tech-spec.md    # schema, storage, CLI contract, release process
├── scripts/            # build + release (cross-compile via GitHub Actions)
├── assets/             # icon etc.
├── DESIGN.md           # rolling locked-decisions log
└── README.md           # you are here
```

---

## License

MIT. See `LICENSE`.

---

## History

This repo began as a code-indexing tool (RAG for your codebase, tree-sitter AST, local LLM chat). In 2026 it was refactored into a general-purpose semantic memory store for AI assistants. The pre-refactor tree is preserved at the `v0-legacy` git tag if you need to recover any of the original indexing logic.
