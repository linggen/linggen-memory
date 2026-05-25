# linggen-memory

**A semantic memory store for AI assistants.**

`ling-mem` is a single-binary CLI + optional web UI that remembers useful facts about you and your work across every session, every tool, and every project. LanceDB-backed, local-first, markdown-native where it counts.

Built as the default memory skill for [Linggen](https://github.com/linggen/linggen); works equally well invoked from Claude Code or any tool that can shell out.

> 🚀 **Status: v0.7.0 — prebuilt binaries available** for macOS Apple Silicon and Linux (x86_64 + aarch64). Active development on `main`. The pre-refactor code-indexing tool is preserved at the `v0-legacy` git tag.

---

## What it does

- **Remembers across sessions.** Facts about who you are, how you prefer to work, what you've tried, what worked, what didn't.
- **Semantic retrieval.** Everything stored gets embedded (1024-dim via `Qwen3-Embedding-0.6B`, multilingual). Find "berth calibration" by asking about "dock alignment."
- **Typed facts.** Four default categories — `fact / preference / decision / learned` — plus `tried / fixed / built` for trajectory-level patterns.
- **Forgetting is first-class.** `delete` by id, `forget` by filter — refuses empty filters as a guardrail.
- **Self-updating.** `ling-mem upgrade --check` reports the latest release; `ling-mem start`, `restart`, and `status` all embed the same cached probe in their JSON so the agent can prompt the user when a new version ships without making extra network calls. `upgrade --yes` swaps the binary atomically and restarts the daemon. (`self-update` still works as an alias.)
- **Three ways to use it:**
  - As the **`shared-memory` skill on Linggen** — web app UI + `Memory_*` tool dispatch in the agent.
  - As the **`shared-memory` skill on Claude Code / Codex / OpenClaw** — SKILL.md body, model calls the CLI via Bash, recall hook injects context every turn.
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
ling-mem list --type preference --since 2026-01-01 --format text

# Forget a finished project
ling-mem forget --context trip-japan-2026 --yes
```

Default output is NDJSON on stdout — any model / script / shell can parse it. Pass `--format text` for human-readable lines.

The daemon (`ling-mem start`) also serves a built-in Data Browser at `http://127.0.0.1:9888` for hands-on filter / edit / batch-delete.

---

## Install

The `ling-mem` binary ships as part of the **`shared-memory` skill** (in the [linggen/skills](https://github.com/linggen/skills) repo at `shared-memory/`). Installing the skill is the recommended path — it fetches the prebuilt binary, wires up the SKILL.md, and seeds the core memory files.

Best experience: **Linggen agent**, which exposes typed `Memory_query` / `Memory_write` tools and a built-in dashboard. The skill also works with **any other agent** that can shell out (Claude Code, Codex, OpenClaw, plain scripts) — they just call the `ling-mem` CLI directly.

```bash
# One-liner installer (cross-host: drops the canonical bundle at
# ~/.linggen/skills/shared-memory and per-host stubs under
# ~/.claude/, ~/.codex/, ~/.openclaw/ if those dirs exist):
curl -fsSL https://linggen.dev/install-shared-memory.sh | bash

# Or from a checked-out clone:
git clone https://github.com/linggen/skills
cd skills/shared-memory
./install.sh                                # auto-detects every host runtime
LING_MEM_VERSION=v0.7.0 ./install.sh        # pin a specific binary version
```

Prebuilt binaries are available for macOS Apple Silicon and Linux (x86_64 + aarch64) on the [releases page](https://github.com/linggen/linggen-memory/releases).

To build from source instead:

```bash
git clone https://github.com/linggen/linggen-memory
cd linggen-memory
cargo build --release
./target/release/ling-mem --help
```

See `doc/tech-spec.md` → *Release process* for the cross-compile + signing flow.

---

## Layout

```
linggen-memory/
├── Cargo.toml          # single crate
├── src/                # all Rust code (CLI, HTTP daemon, embed pipeline,
│                       #  LanceDB store)
├── static/             # Data Browser UI (baked into the binary via rust-embed,
│                       #  served at 127.0.0.1:9888 by the daemon)
├── doc/
│   ├── product-spec.md # features, user-facing behavior, scenarios
│   ├── tech-spec.md    # schema, storage, CLI contract, release process
│   └── ui-spec.md      # Data Browser UI: layout, endpoints, interactions
├── scripts/            # release.sh + Dockerfile.linux (multi-arch buildx)
├── assets/             # icon etc.
├── CHANGELOG.md        # release notes per version
├── LICENSE             # MIT
└── README.md           # you are here
```

The thin **skill wrapper** (SKILL.md + dashboard + install.sh + scan/extract scripts) lives in the [`linggen/skills` repo at `ling-mem/`](https://github.com/linggen/skills/tree/main/ling-mem) — separate from this binary's source.

---

## Telemetry

`ling-mem` sends a small amount of anonymous usage data to `https://linggen.dev/api/track` so we can see whether anyone's using it and which features matter. Specifically:

- **`install`** — once on first launch on a machine, and once after each upgrade. Includes the install source (e.g. `wrapper`, `linggen`, `clawhub`, `unknown`) and the previous + current versions.
- **`command`** — one event per `Memory.*` HTTP call, with the verb name only (`memory.search`, `memory.add`, `memory.forget`, …).

Daily/weekly active counts are derived server-side from any event row, so there's no separate heartbeat ping — every active user already produces at least one `command` event per day.

What's **never** sent: fact content, query text, embeddings, file paths, your IP (the receiver doesn't store it), or any user-identifying string. The `installation_id` is a random UUIDv4 generated on first run and stored at `~/.linggen/installation_id`.

**Disabling telemetry:**

- Runtime: set `LING_MEM_NO_TELEMETRY=1`, or `touch ~/.linggen/no-telemetry`.
- Compile time: build with `cargo build --release --no-default-features` (no telemetry code is even linked in).

Source is open on both ends: client at [`src/telemetry/`](src/telemetry/), receiver at [`linggensite/functions/api/_lib/analytics.ts`](https://github.com/linggen/linggensite/blob/main/functions/api/_lib/analytics.ts).

---

## License

MIT. See `LICENSE`.

---

## History

This repo began as a code-indexing tool (RAG for your codebase, tree-sitter AST, local LLM chat). In 2026 it was refactored into a general-purpose semantic memory store for AI assistants. The pre-refactor tree is preserved at the `v0-legacy` git tag if you need to recover any of the original indexing logic.
