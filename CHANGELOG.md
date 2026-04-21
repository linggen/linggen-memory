# Changelog

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
