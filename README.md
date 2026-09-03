# linggen-memory

**Not just remember and recall. Everything a brain does.**

The most important thing in a memory system is not storing a fact, and it is not finding it again. A good one does what a brain does — holds some things for a day and others for years, brings back what matters, lets go of what does not, collapses duplicates, merges what belongs together, checks its own work, and settles contradictions when it finds them. That is what `ling-mem` does.

One local daemon. No SaaS, no API key, no signup. The same store in Claude Code, Codex, OpenClaw, and Linggen.

> **Status: v1.7.2 — stable.** Store schema and the CLI/HTTP/MCP contract are frozen. Prebuilt binaries for macOS Apple Silicon and Linux x86_64. The pre-refactor code-indexing tool is preserved at the `v0-legacy` git tag.

---

## How it works

```
   ┌──────────────────────── IN YOUR TURN ────────────────────────┐
   │  the Linggen engine + the plugin — on every host             │
   │                                                              │
   │     capture  ──▶  recall  ──▶  scope  ──▶  reconcile         │
   └───────────────────────────┬──────────────────────────────────┘
                        writes │  ▲ reads
                               ▼  │
   ╔══════════════════════════════════════════════════════════════╗
   ║  ling-mem — one local daemon, one store                      ║
   ║  every row and every mechanical operation. no model inside.  ║
   ╚══════════════════════════════════════════════════════════════╝
                         reads │  ▲ writes back
                               ▼  │
   ┌─────────────────────── AT 3AM, UNATTENDED ───────────────────┐
   │  the memory agent running the dream mission                  │
   │                                                              │
   │     dream  ──▶  forget  ──▶  audit  ──▶  review queue        │
   └───────────────────────────┬──────────────────────────────────┘
                               ▼  what it cannot settle alone
                       ┌────────────────────┐
                       │  solve — with you  │
                       └────────────────────┘
```

Both halves of the loop meet at the same store. **The day half runs anywhere the plugin is installed.** The night half needs [Linggen](https://github.com/linggen/linggen), which ships the mission scheduler — install it alongside Claude Code or Codex to close the loop.

| stage | when | what it does |
|---|---|---|
| **capture** | live | Signal is saved in the turn it appears. What you state outright goes to long-term; the rest stages in short-term. |
| **recall** | live | Relevant rows surface at the start of every turn, and anything used in a reply is cited inline. |
| **scope** | live | Every row records the project it came from, so recall is scoped to where you are asking — plus everything that is about *you*. |
| **reconcile** | live | The agent rewrites its own notes freely. What you said changes only with you, and the store refuses a silent rewrite of your voice. |
| **dream** | nightly | Each unjudged day is reviewed one at a time. Durable rows are promoted; nothing unjudged is ever deleted. |
| **forget** | nightly | Once a day is judged, its staging rows fade after about a week unless they were promoted. Mechanical, no model in the loop. |
| **audit** | nightly | Proven chains collapse into one current row. Merges archive rather than delete, so any of it can be unpacked. |
| **solve** | with you | The agent drains the review queue with you present, fixing what evidence proves and asking about the rest one question at a time. |

---

## The parts

- **`ling-mem`** — the single binary and local daemon that owns the store (LanceDB + `Qwen3-Embedding-0.6B`, 1024-dim, multilingual). It performs every mechanical operation: write, search, dedup, archive, export. No LLM runs inside it, and every frontend goes through it, so removing a frontend never loses a row.
- **The Linggen engine** — memory tools for every agent, the always-on identity block at the top of each session, per-turn recall injection, and the capture protocol in the system prompt. This is the half that runs while you type.
- **The memory agent + dream mission** — the offline judgment brain, on a 3am schedule with a 24-hour catch-up if the machine was asleep. The memory app's buttons trigger the same mission, so the UI and the schedule cannot drift apart.
- **The `linggen` plugin & skill** — the same store inside Claude Code, Codex, and OpenClaw: recall each turn, the capture protocol, runbooks, and the memory app UI (calendar, dashboard, row browser). Any other host reaches the same rows over the daemon's `/mcp` `memory_*` group.

## Three tiers

Rows are separated by how durable they have proven to be, and the nightly pass is what moves them between shelves.

- **Core** — a handful of high-confidence universals about the person (name, role, hard work rules). Present in every session, costing no retrieval.
- **Long-term** — everything else durable, retrieved on demand. *State and lessons, never events* — the test is whether the row would still matter in three months.
- **Short-term** — per-turn working capture. Events and uncertain signal land here cheaply; the dream decides what earns a place before the rest fades.

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

Default output is NDJSON on stdout — any model, script, or shell can parse it. Pass `--format text` for human-readable lines.

The daemon (`ling-mem start`) also serves a built-in Data Browser at `http://127.0.0.1:9528` for hands-on filter / edit / batch-delete. Every row is yours to read, edit, or delete.

---

## Install

Install from your agent's own marketplace — it manages updates and, on Claude Code and Codex, the per-turn recall hook. Pick **one** channel per host.

```text
Claude Code   claude plugin marketplace add linggen/linggen-memory
              claude plugin install linggen@linggen-memory
Codex         codex plugin marketplace add linggen/linggen-memory
              codex plugin add linggen@linggen-memory
OpenClaw      clawhub install linggen
Any agent     npx skills add linggen/linggen-memory@linggen
Linggen       Settings → Skills → shared-memory   (in-app)
```

Run these in your shell, not in the agent prompt — the `@linggen-memory` qualifier is required. On Claude Code and Codex, restart the agent afterwards to load the plugin.

The `ling-mem` binary is fetched automatically on first use (pinned, SHA-256 verified) to the one cross-host location `~/.local/bin/ling-mem`. To install just the binary manually:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/linggen/linggen-memory/main/plugins/linggen/scripts/install-bin.sh) --version '^1'
```

Prebuilt binaries for macOS Apple Silicon and Linux x86_64 are on the [releases page](https://github.com/linggen/linggen-memory/releases).

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
│                       #  served at 127.0.0.1:9528 by the daemon)
├── plugins/
│   ├── linggen/        # Claude Code + Codex plugin — recall hook, commands,
│   │                   #  and the published `linggen` skill
│   └── openclaw/       # OpenClaw plugin
├── doc/
│   ├── product-spec.md      # features, user-facing behavior, scenarios
│   ├── tech-spec.md         # schema, storage, CLI contract, release process
│   ├── ui-spec.md           # Data Browser UI: layout, endpoints, interactions
│   ├── release-targets.md   # channel map
│   └── schema-versioning.md # store schema compatibility rules
├── benchmark/          # retrieval + consolidation evaluation
├── scripts/            # release.sh + Dockerfile.linux (multi-arch buildx)
├── tests/
├── assets/             # icon etc.
├── CHANGELOG.md        # release notes per version
├── LICENSE             # MIT
└── README.md           # you are here
```

The engine half of the system — the memory tools, the memory agent, and the dream mission — lives in the [`linggen/linggen`](https://github.com/linggen/linggen) engine repo. The product spec for the system as a whole is [`doc/memory-spec.md`](https://github.com/linggen/linggen/blob/main/doc/memory-spec.md) there, and the overview page is at [linggen.dev/memory](https://linggen.dev/memory).

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

The client side is open and in this repo: [`src/telemetry/`](src/telemetry/) — read exactly what is sent. The receiver is a Cloudflare function on `linggen.dev`, which is not a public repo.

---

## License

MIT. See `LICENSE`.

One subtree is different: **`plugins/linggen/skills/linggen/` is MIT-0**, carried in
its own `LICENSE` there. That subtree is the bundle published to ClawHub, and ClawHub
distributes *every* skill under MIT-0 — the registry types the field as `"MIT-0"|null`
and renders `PLATFORM_SKILL_LICENSE` for both, and `clawhub skill publish` sends
`acceptLicenseTerms: true` on every push. So the bundle states the terms a user
actually receives; declaring anything stricter there would advertise a grant the
platform does not pass on. It carried Apache 2.0 until 2026-08-14, inherited from the
`linggen/skills` repo when the plugin tree was scaffolded here.

The nearest `LICENSE` governs, so the rest of `plugins/linggen/` stays MIT, and
everything outside this repo keeps its own terms — the engine is Apache 2.0. Nothing
but the published skill bundle moved.

---

## History

This repo began as a code-indexing tool (RAG for your codebase, tree-sitter AST, local LLM chat). In 2026 it was refactored into a general-purpose semantic memory store for AI assistants. The pre-refactor tree is preserved at the `v0-legacy` git tag if you need to recover any of the original indexing logic.
