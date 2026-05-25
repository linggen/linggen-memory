# Changelog

## [0.7.0] - 2026-05-25 — shared-memory skill + user-tunable TTL

The skill bundle was renamed `ling-mem` → `shared-memory` in the
[linggen/skills](https://github.com/linggen/skills) repo; the binary,
the CLI command, and the daemon stay named `ling-mem`. New install
path (cross-host one-liner):

```bash
curl -fsSL https://linggen.dev/install-shared-memory.sh | bash
```

### Added

- **`/api/config` endpoint** — `GET` / `PUT` `{episodic_ttl_days}`. Lets
  the user tune the episodic-row lifetime per machine; every dream pass
  on every host reads the same value through the daemon. Stored at
  `~/.linggen/memory/.config.json` (hand-editable).
- **Dashboard ⚙ Settings overlay** — gear icon in the top-right of
  the data browser writes the config via `PUT /api/config`. No restart
  required.
- **`ling-mem list --older-than <duration>`** — sugar for `--until <now
  − duration>`. Accepts `s|m|h|d|w` units (`30d`, `12h`, `1w`). The
  dream pass's Phase 3 worklist now reads `ling-mem list --episodic
  --older-than ${TTL_DAYS}d` instead of computing RFC-3339 cutoffs in
  bash.
- **`ling-mem edit --tier <core|semantic|episodic>`** — exposes the
  `tier` field on the patch surface. Useful for repairing rows whose
  tier drifted from their table identity (e.g. a pre-fix `add
  --episodic` write stuck on `tier=semantic`).

### Fixed

- **`add --episodic` (CLI direct-store)** now stamps `tier=Episodic`,
  matching the HTTP path's invariant. Previously the row went to the
  episodic table but carried `tier=semantic` — visible as the wrong
  badge on the dashboard's All tab.
- **Dashboard 'All' tab no longer double-fetches.** After the
  cross-table-reads daemon change (`39e100b`) the existing client-side
  merge double-counted episodic rows. Replaced with a single
  `/api/memory/list` call; row tier is derived from each row's `tier`
  field.

## [Unreleased]

### Added

- **Recall now spans both tables.** `search` / `/api/memory/search` query
  the curated `semantic` store *and* the staging `episodic` store together
  via a new `crate::memory::Recall` type, then merge — closing the "told
  you 5 min ago, not yet consolidated, forgotten" gap. No read-time
  re-rank: the union is ordered by the cosine each row already carries
  (same embedder → comparable). Cross-table and episodic-internal
  near-duplicates (cosine ≥ dedup threshold) collapse, curated `semantic`
  copy winning. `list`/browse and `--episodic` stay single-table.

### Changed (`facts` → `memory` rename)

- **Curated long-term table `facts` → `semantic`; staging table stays
  `episodic`.** One LanceDB directory holding both (Tulving's
  episodic/semantic split).
- **On-disk directory `~/.linggen/memory/facts.lancedb/` →
  `~/.linggen/memory/memory.lancedb/`.**
- **Rust API renamed throughout** — `FactsStore` → `MemoryStore`,
  `crate::facts` module → `crate::memory` (file tree `src/facts/` →
  `src/memory/`), struct `Fact` → `Memory`, `FactType` → `MemoryType`,
  `FactPatch` → `MemoryPatch`, `CliFactType` → `CliMemoryType`. The `fact`
  *category* (one of the seven `MemoryType` values) is unchanged — only
  the over-broad "everything is a fact" naming was retired.

### Breaking

- Existing data at the old `facts.lancedb` path is not auto-migrated.
  Remove `~/.linggen/memory/facts.lancedb/` and start fresh (consistent
  with the no-forward-migration policy). NDJSON/JSON wire format is
  unchanged (serde field/category names are stable).

## [0.5.1] - 2026-05-11 — dashboard relevance-score chip

### Added

- **Dashboard now renders the relevance score on each result card** when
  in search mode. Score is shown as a chip in the row header (right side,
  before the timestamp), formatted to 3 decimals. Title attribute
  documents that it's cosine similarity in `[-1, 1]`. Hidden in list
  mode where no score is meaningful.
- Score data was already in `/api/memory/search` responses since v0.5.0
  — only the rendering was missing.

## [0.5.0] - 2026-05-11 — Qwen3-Embedding-0.6B swap (multilingual)

### Changed

- **Embedding model: MiniLM-L6-v2 (384-dim, English-only) → Qwen3-Embedding-0.6B
  (1024-dim, 100+ languages).** Backed by `fastembed` v5's `qwen3` feature
  (candle backend), with Metal acceleration on macOS. Same-token-overlap
  retrieval scores jump from ~0.05–0.08 (MiniLM) to ~0.4–0.7 (Qwen3) on
  short factual rows. Chinese / mixed-language content is now properly
  embedded — previously Chinese tokens were emitted as essentially-random
  vectors and unsearchable.
- **`VECTOR_DIM` 384 → 1024.** The `facts` table's `vector` field is now
  a `FixedSizeList<Float32, 1024>`. Existing v0.4.x data is therefore
  **schema-incompatible**. On open, the store now surfaces a clear error
  pointing to the data dir; remove `~/.linggen/memory/facts.lancedb/`
  to start fresh. A non-destructive `ling-mem reindex` migration command
  is planned for a follow-up release.
- **CLI / HTTP search now uses query-side prefixing** (`"query: "`) and
  add uses passage-side (`"passage: "`) per Qwen3's retrieval convention.

### Added

- `Embedder::embed_query` for the search side; `embed_one` / `embed_many`
  remain for stored passages. Two sites updated: `cli/mod.rs` search path
  and `http/memory.rs` `/api/memory/search`.
- Schema-mismatch detection on `FactsStore::open` — surfaces a clear
  error pointing to the data dir when an incompatible legacy table
  is detected.

### Performance

- First-run download: ~1.2 GB (Qwen3-Embedding-0.6B BF16) vs ~23 MB MiniLM.
- Resident RAM: ~1.5–2 GB for the embedder (model + activations + overhead)
  vs ~50 MB for MiniLM.
- Per-embed latency (Apple Silicon Metal): ~80–200 ms vs ~5–10 ms for
  MiniLM. Acceptable for once-per-`search` and once-per-`add` calls.

### Breaking

- Existing LanceDB facts table is incompatible. Remove the data dir to
  start fresh; migration tool coming in v0.5.1.

## [0.4.4] - 2026-05-07 — dashboard add-fact UI sync fix

### Fixed

- **Dashboard "+ Add" form: post-save state out of sync with what got
  saved.** After clicking Save on a new fact, the dashboard left the
  textarea empty and surfaced a spurious "Content is required" message —
  even though the fact had landed in the store correctly (a refresh
  showed it). Cause: `/api/memory/add` returns
  `{action, fact, [similarity], [previous_id]}` to carry dedup metadata,
  unlike `/api/memory/list` and `/api/memory/update` which return a flat
  fact. The dashboard's `saveNewFact` was treating the wrapper object as
  if it were the fact, so `fact.content` was undefined and `cloneDraft`
  filled the new draft with empty defaults. Fix: unwrap to `result.fact`
  in `saveNewFact` (with a `?? result` fallback so the code still works
  if the API ever flattens). API contract unchanged — `/api/memory/add`
  still returns the wrapped shape, so CLI and other consumers that
  inspect `action` / `previous_id` keep working.

## [0.4.3] - 2026-05-07 — pin fastembed cache to data dir

### Fixed

- **fastembed model cache no longer scattered across CWDs.** Every invocation
  of `ling-mem` from a new working directory used to drop a fresh
  `.fastembed_cache/` (~87 MB MiniLM ONNX) wherever it was launched from,
  because `fastembed-rs` defaults its cache to `./.fastembed_cache/` when
  no `cache_dir` is set. `ling-mem` now sets `FASTEMBED_CACHE_DIR` to
  `<data_dir>/cache/fastembed/` (defaults to `~/.linggen/cache/fastembed/`)
  before any embedder constructs, so there is one cache, regardless of CWD.
  Existing user-set `FASTEMBED_CACHE_DIR` is honored.

## [0.4.2] - 2026-05-07 — CLI rename + `init` + cached upgrade probe in `status`

### Added

- **`ling-mem upgrade`** — primary spelling for the binary-update command,
  matching `apt upgrade` / `brew upgrade` convention. The previous spelling
  `ling-mem self-update` continues to work as a hidden alias for back-compat;
  scripts and skill instructions can be migrated at leisure.
- **`ling-mem edit <id>`** — primary spelling for the row-edit command. The
  previous spelling `ling-mem update <id>` continues to work as a hidden
  alias. Removes the semantic foot-gun where `ling-mem update` looked like a
  binary-update command but was actually a row mutation.
- **`ling-mem init`** — idempotent seeder that mirrors `seed_core_memory`
  from `install.sh`. Creates `<data-dir>/memory/` plus empty `identity.md`
  and `style.md` if missing. Intended for hosts that bypass `install.sh`
  (OpenClaw via ClawHub) and for recovery after a `rm -rf ~/.linggen`. Output
  reports per-file `created: true|false` so callers can tell what changed.
- **`ling-mem status` now includes a cached `update` block** — same shape
  that `start` and `restart` already returned. Reads from the existing 24h
  on-disk cache (no extra network call), so frequent pollers get current-vs-
  latest visibility without a GitHub round-trip per check. Adds a
  `checked_at` unix-seconds timestamp so callers can reason about freshness.

### Internal

- New `update::read_cached(data_dir)` and `update::cache_fetched_at(data_dir)`
  surfaces — pure cache reads with no network fallback. `check_quiet` (used
  by `start` / `restart`) and `check` (used by `upgrade --check`) keep their
  network-on-miss behavior.



### Fixed

- **`origin` filter returning 0 rows** — `Filters::to_sql` no longer
  emits a `"from" = '<value>'` clause. LanceDB / DataFusion mishandles
  the SQL keyword even when double-quoted and returns an empty result
  set. Origin filtering is now applied post-fetch in
  `apply_origin_filter`. `forget` with an origin-only filter falls back
  to list-then-delete-by-id and no longer trips the empty-filter guard.

### Build

- **`Dockerfile.linux`**: install `protobuf-compiler`. `lance-encoding`
  (a LanceDB dep) has a `build.rs` that invokes `protoc`; without it
  the multi-arch Linux build fails. The macOS host picks up `protoc`
  from Homebrew, so this only surfaced in the Docker pipeline.

## [0.3.1] - 2026-04-28 — Linux release pipeline

### Build

- **Cross-build Linux x86_64 + aarch64 via Docker buildx**.
  `release.sh` now drives both the host (cargo build for macOS native)
  and a multi-arch Linux build through a shared `linggen-builder`
  buildx instance. New `scripts/Dockerfile.linux` (rust:bookworm +
  libssl/cmake/clang, dep-cache layer using stub main.rs/lib.rs) and
  `scripts/build-linux.sh` thin wrapper. `release.sh` adds
  `--skip-linux`, signs and checksums every Linux tarball, uploads
  them all to the GitHub release.
- Drop `--locked` from the host `cargo build` so version bumps don't
  fail when `Cargo.lock` hasn't been refreshed yet (an explicit
  `cargo update --offline` runs after `sync_cargo_version`).

### Removed

- Internal-only docs **`DESIGN.md`**, **`RELEASES.md`**, **`SIGNING.md`**
  — content has either landed in `doc/` or moved to repo-level GitHub
  workflow comments. Released artifacts unaffected.

## [0.3.0] - 2026-04-28 — Self-update

- **`ling-mem self-update`** — new subcommand. `--check` prints the latest
  version (24h-cached probe of `linggen/linggen-memory` GitHub releases).
  Without `--check`, downloads the matching `ling-mem-<slug>.tar.gz`,
  verifies SHA-256, stops the daemon, atomic-renames the binary into
  place (keeping `bin/ling-mem.prev` for rollback), and restarts the
  daemon by spawning the new binary explicitly. Requires `--yes` to
  proceed past the check.
- **`ling-mem start` / `restart` JSON** — now embeds an `update` field
  with the cached probe result, so the agent can prompt the user when a
  newer release is available without an extra command.
- **SKILL.md** — added an `## Updates` section telling the agent how to
  surface the prompt and run `self-update --yes` on user confirmation.
- **install.sh** — symlinks the installed binary onto PATH at
  `~/.local/bin/ling-mem` (preferring the Linggen install when both
  hosts are present), so bare `ling-mem` works in any shell. Drops the
  CC CLAUDE.md hint's absolute-path parenthetical now that it's
  redundant.

## [0.1.0] - 2026-04-21 — Memory skill rebuild

**Complete rewrite.** Repo repurposed from a code-indexing tool into a
general-purpose semantic memory store for AI assistants.

See `doc/product-spec.md` for the full product story and `doc/tech-spec.md`
for the implementation contract. The pre-refactor source is preserved at
the `v0-legacy` git tag.

### What's new

- **`ling-mem` binary** — single Rust binary, LanceDB-backed, with all v0.1
  CLI subcommands:
  - `add`, `get`, `search`, `list`, `update`, `delete`, `forget`
  - `collect`, `extract` — session scanning + transcript flattening (Rust
    ports of the prior shell scripts)
- **LanceDB `facts` table** with a 12-field schema: id, content, vector
  (384-dim), contexts, tags, type, outcome, from, cwd, created_at,
  occurred_at, source_session.
- **Seven canonical fact types**: `fact`, `preference`, `decision`,
  `tried`, `fixed`, `learned`, `built`. No `activity` catch-all.
- **Retrieval**: vector-similarity semantic search + metadata filtering on
  contexts / types / origin / outcome / time range.
- **I/O contract** (see `doc/tech-spec.md`): NDJSON on stdout, JSON errors
  on stderr with `code` field, human-readable via `--format text`.
- **Data dir**: respects `$LINGGEN_DATA_DIR`; falls back to `~/.linggen/`.
  Multi-user isolation is path-level — binary is single-user per invocation.
- **Embedding model**: `sentence-transformers/all-MiniLM-L6-v2`
  (local inference via Candle, 384-dim). Configurable via
  `LING_MEM_EMBEDDING_MODEL` env var.

### What's gone

- **Local LLM inference** (was Qwen3 via Candle). Linggen handles chat now;
  this binary is store + search only.
- **Code-specific indexing** — tree-sitter AST, file dependency graph,
  language detection, project-aware file walking.
- **Multi-crate workspace** — flattened to a single crate. Sub-crates
  (api/core/embeddings/storage/ingestion/enhancement/mcp-server/llm/
  architect/context/intent) are gone or folded into `src/<module>/`.
- **Old webpage** — frontend renamed `webui/` with a Phase 8 placeholder;
  the markdown-editor UI for memory rebuilds later.
- **Shell release scripts** — replaced by GitHub Actions workflows
  (`ci.yml` + `release.yml`).
- **MCP HTTP endpoints** — can resurface later if the memory skill needs
  a direct MCP surface.

### Release artifacts

Cross-compiled to 4 targets by the new release workflow:

- `ling-mem-aarch64-apple-darwin.tar.gz`  (macOS Apple Silicon)
- `ling-mem-x86_64-apple-darwin.tar.gz`   (macOS Intel)
- `ling-mem-x86_64-unknown-linux-gnu.tar.gz`
- `ling-mem-aarch64-unknown-linux-gnu.tar.gz`
- `SHA256SUMS.txt` (combined checksums for all tarballs)

Each tarball contains the `ling-mem` binary + `README.md` + `LICENSE`.

---

## [0.7.0] - 2026-02-19

### Added

- **CLI subcommands**: `ling-mem` now has built-in subcommands — `stop`, `status`, `index`, `install`, `update`. No separate CLI binary needed.
  - `ling-mem serve` / `ling-mem serve --daemon` — start server (foreground or background)
  - `ling-mem stop` — stop background daemon
  - `ling-mem status` — show server status
  - `ling-mem index [path]` — index a directory (supports `--mode`, `--name`, `--include`, `--exclude`, `--no-wait`)
  - `ling-mem install` / `ling-mem update` — self-install or self-update to latest version
- **Skill split**: The monolithic `linggen` skill is now split into `memory` (RAG, search, indexing) and `skiller` (marketplace, skill install).
- **Memory skill**: 11 shell scripts for semantic search, code search, memory storage/retrieval, prompt enhancement, indexing, and server management.

### Changed

- **Renamed binary**: `linggen-server` → `ling-mem`. Single binary with embedded Web UI, HTTP API, and MCP server.
- **Standalone release**: Releases independently to `linggen/linggen-memory` GitHub repo (previously part of `linggen/linggen`).
- **Removed linggen-cli**: The CLI (`linggen`) has been moved to [linggen](https://github.com/linggen/linggen) as the `ling` binary.
- **Removed cf-worker**: Cloudflare worker code moved out of this repo.
- **Updated build scripts**: All scripts produce a single `ling-mem` binary per platform. Artifact names changed from `linggen-cli-*` + `linggen-server-*` to `ling-mem-*`.
- **Updated install script**: Fetches from `linggen/linggen-memory` releases and installs `ling-mem`.
- **Updated Dockerfile**: Linux builds produce `ling-mem-linux-{arch}.tar.gz` instead of separate CLI and server tarballs.
- **Updated documentation**: All docs updated for new binary name, port 8787, standalone service identity.

### Release Artifacts

- `ling-mem-macos-aarch64.tar.gz`
- `ling-mem-macos-x86_64.tar.gz`
- `ling-mem-linux-x86_64.tar.gz`
- `ling-mem-linux-aarch64.tar.gz`
- `manifest.json`

## [0.6.5] - 2026-02-03

- Version alignment release for linggen integration.
- Last release under the old `linggen/linggen` repo with dual `linggen-cli` + `linggen-server` artifacts.

## [0.6.3] - 2026-01-29

- Enhanced skills, more skills in online registry.
- Bootstrap Linggen by skill.

## [0.6.2] - 2026-01-27

### Added

- **Online Skills Registry**: Introduced `linggen skills add` command to install skills from GitHub repositories directly.
- **CLI Server Management**: Added `linggen stop` and `linggen restart` commands for better server lifecycle control.

### Changed

- **macOS Distribution**: Removed Tauri desktop app for macOS. Users should now use the web UI at `http://localhost:8787` instead.
- **Skills Installation**: Shifted from local file-based skills to online registry with automatic version tracking and installation recording.

### Fixed

- **CLI Terminology**: Updated `linggen check` and `linggen doctor` commands to refer to "Server" instead of "App" for clarity.
- **Graceful Shutdown**: Server now supports graceful shutdown via HTTP endpoint for reliable cross-platform operation.

## [0.5.0] - 2026-01-13

### Added

- **Library System**: Introduced a new library template system with predefined skills (linggen, code-simplifier, react-pack) and policies.
- **Library View**: Added a dedicated view for exploring and managing library packs.
- **MCP Support**: Implemented Model Context Protocol (MCP) handlers in the backend.
- **Activity View Enhancements**: Improved activity monitoring and logging for better visibility into background tasks.

### Changed

- **Tailwind CSS Migration**: Major frontend refactor migrating from custom CSS files to Tailwind CSS for better consistency and performance.
- **Theme Overhaul**: Updated dark and light themes with a more refined, Obsidian-like color palette.
- **Editor Improvements**: Enhanced the CodeMirror 6 editor with better live preview rendering and mermaid diagram support.
- **Sidebar & Navigation**: Redesigned the sidebar for better source management and more intuitive navigation.

### Fixed

- **Editor Visibility**: Fixed a contrast issue where inline code keywords (like `function`) were nearly invisible in dark mode.
- **Rescan Reliability**: Improved path handling and ownership checks in the internal indexer.

## [0.4.0] - 2026-01-02

### Added

- **Multi-Source File Watcher**: Backend now monitors all local sources' `.linggen` directories recursively.
- **Incremental Indexing**: Automatic re-indexing of memories, prompts, and notes when markdown files are created, modified, or deleted.
- **Real-time UI Sync**: New SSE (Server-Sent Events) endpoint `/api/events` to push file change notifications to the frontend.
- **Dynamic Metadata**: Indexer now parses YAML frontmatter in markdown files and stores all fields as searchable metadata in LanceDB.
- **Deterministic Memory Fetching**: New `memory_fetch_by_meta` MCP tool for retrieving memories by ID or other metadata.

### Changed

- **Memory Storage**: Shifted to a filesystem-first approach. Memories are now stored as human-readable `.md` files in `.linggen/memory/`.
- **MCP Tooling**: Removed `memory_create` and `memory_update` in favor of direct file manipulation by the LLM or user.
- **Frontend Refresh**: Replaced 10-second polling with an event-driven model using `EventSource` for instantaneous UI updates.
- **Internal Indexer**: Improved robustness of the rescan process and path handling for cross-platform compatibility.

### Fixed

- Resolved compilation issues with `SourceType` equality and mismatched return types in the rescan handler.
- Fixed a bug where file removals were not correctly detected on certain operating systems (macOS) during renames.
- Corrected path ownership errors in the internal indexer.
