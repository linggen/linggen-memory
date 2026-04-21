# Cleanup Classification — Memory-Skill Refactor

Scratchpad for Phase 1 (strip to RAG kernel). Captures per-file disposition at
the time of the `v0-legacy` tag. Delete this file once Phase 1 lands.

## Context

linggen-memory was a code-indexing tool (tree-sitter AST + LanceDB RAG + local
LLM + webpage). Refactoring it into a general-purpose semantic memory store
backed by LanceDB. After the refactor the binary will:

- Manage facts as LanceDB rows (add/search/list/update/archive/delete/forget)
- Generate embeddings via the existing pipeline
- Run an HTTP daemon with an embedded markdown-editor webpage
- NOT do local LLM inference (Linggen handles that)
- NOT index code projects (old use case)
- NOT have project-based scoping (contexts become N:M tags on facts)

See `~/.claude/plans/memory-system-rebuild.md` for the ten-phase plan.

## Classification rules

- **KEEP** — directly reusable for the new memory store.
- **REMOVE** — dead to the new use case.
- **REDESIGN** — structural ideas survive but semantics differ.
- **UNKNOWN** — nonobvious; flag for human decision before touching.

## Classification table

| Path | Size (LOC) | Purpose | Class | Notes |
|------|-----------:|---------|-------|-------|
| **Workspace + foundational** | | | | |
| `backend/Cargo.toml` | 21 | Workspace with 11 member crates | KEEP | Adjust as crates are removed |
| `backend/core` | 261 | Core types: SourceConfig, Document, Chunk, Job, FileIndexInfo | KEEP | Adapt SourceConfig → Contexts; trim types not used by fact model |
| `backend/storage` | 1,852 | LanceDB wrapper, metadata/preferences/profile stores, redb internals | KEEP | Central. VectorStore + MetadataStore reused directly |
| `backend/embeddings` | 427 | TextChunker + EmbeddingModel (Candle/HuggingFace) | KEEP | Required for new vector store |
| **Storage / server layer** | | | | |
| `backend/api` | 10,435 | Axum HTTP server, CLI dispatch, job manager, 25+ routes | REDESIGN | Keep HTTP scaffold, job manager, auth/CORS. Strip code-indexing routes (graph, project handlers). Add fact CRUD routes |
| `backend/mcp-server` | 596 | MCP protocol server with enhance/search/resources tools | KEEP | Refactor tools to fact-oriented ops |
| **Ingestion pipeline** | | | | |
| `backend/ingestion` | 912 | Multi-source ingestors (Git, Local, Web), file walkers, watchers, text extraction (PDF, DOCX) | REDESIGN | Keep: text extraction (PDF/DOCX), HTTP fetch. Remove: git-aware walker, project watchers, local file scanner |
| `backend/enhancement` | 697 | Prompt enhancement, intent detection, context retrieval | REDESIGN | Repurpose as generic context retrieval over facts. Strip LLM-coupling |
| `backend/context` | 158 | Context analyzer using LLM for relevance ranking | REMOVE | Depends on llm. Linggen handles LLM now |
| `backend/intent` | 232 | Intent classification (FixBug/ExplainCode/DebugError/…) | REMOVE | Code-focused classes; not reusable |
| **Code-indexing only** | | | | |
| `backend/llm` | 1,107 | MiniLLM, Qwen3 inference via Candle, model download/caching, KV cache | REMOVE | Local LLM inference out of scope |
| `backend/architect` | 4,092 | Tree-sitter AST for Rust/TS/Python/Go/Java, dependency graph, language detection | REMOVE | Pure code-semantics; nothing reusable for facts |
| **Frontend (React)** | | | | |
| `frontend/src` | 7,727 | React UI: 6 views (Workspace, Sources, Activity, Settings, Assistant, Library), CodeMirror, graph | REDESIGN | Replace project-centric UI with fact-review/edit UI. Keep React+Vite scaffold, component patterns |
| `frontend/src/views/WorkspaceView.tsx` | ~300 | File-level code editor + dependency graph | REMOVE | Replace with fact editor |
| `frontend/src/views/SourcesView.tsx` | ~100 | Project listing & selector | REMOVE | Replace with context-tag filter |
| `frontend/src/views/GraphView.tsx` | ~400 | Force-directed file dep graph (react-force-graph-2d) | REMOVE | Not needed for facts |
| `frontend/src/views/ActivityView.tsx` | — | Job/index activity | UNKNOWN | Read before deciding — may adapt to memory operations log |
| `frontend/src/views/AssistantView.tsx` | — | Chat with local LLM | REMOVE | Linggen handles chat |
| `frontend/src/views/SettingsView.tsx` | — | App settings | REDESIGN | Reuse for embedding-model / data-dir / storage config |
| `frontend/src/views/LibraryView.tsx` | — | Library/browse | UNKNOWN | Worth reviewing — may adapt to fact library |
| `frontend/public/` | — | Static assets (logo, favicon) | KEEP | Branding reusable |
| **Build / release** | | | | |
| `scripts/build.sh` | 76 | Build orchestrator | KEEP | Not indexer-specific |
| `scripts/build-mac.sh` | 59 | macOS binary + notarization | KEEP | Needed for prebuilt releases |
| `scripts/build-linux.sh` | 64 | Multi-arch Linux via Docker | KEEP | Needed for prebuilt releases |
| `scripts/lib-common.sh` | 78 | Shared shell utilities | KEEP | |
| `scripts/release.sh` | 136 | GitHub release automation | KEEP | |
| `scripts/sync-version.sh` | 46 | Cross-file version sync | KEEP | |
| **Docs / branding** | | | | |
| `assets/icon.icns` | 3 MB | App icon | KEEP | |
| `doc/` | — | Product / integration / features / vision docs | REDESIGN | Rewrite per memory-spec focus |
| `README.md` | — | Project description | REDESIGN | Rewrite for fact-store positioning |
| `CHANGELOG.md` | — | Release history | KEEP | Append a "reset for memory skill" entry |
| `SIGNING.md` | — | Code-signing notes | KEEP | Still applies |
| `LICENSE` | — | License | KEEP | Unchanged |
| **IDE / meta** | | | | |
| `.github/` | — | CI/CD workflows | KEEP | Adjust matrix to memory-skill targets |
| `.claude/`, `.cursor/`, `.vscode/` | — | IDE configs | KEEP | Cosmetic |
| **Artifacts (dev-time only)** | | | | |
| `dist/`, `frontend/dist/`, `build.log`, `server_debug_path.log`, `server.json` | — | Generated artifacts | REMOVE | Gitignore if not already; regenerated each build |
| `test_enhancement.sh`, `test.toml`, `uninstall-cli.sh` | — | Ad-hoc test/helper scripts | UNKNOWN | Read each before keeping — likely tied to old use cases |

## Rough totals

- **Backend:** ~20k LOC. Projected removable in Phase 1: ~7k LOC (llm, architect, context, intent, most of ingestion/enhancement). Keeps ~10k LOC on storage/api/embeddings/mcp-server.
- **Frontend:** ~7.7k LOC. Projected removable/redesigned: most of it. Keep scaffold + component patterns.

## Phase 1 execution order (suggested)

1. Delete clearly-dead code: `backend/llm`, `backend/architect`, `backend/context`, `backend/intent`. Biggest LOC win, unambiguous.
2. Remove workspace members from `backend/Cargo.toml`. Build should still pass because they had no inbound deps from the survivors (verify).
3. Strip code-indexing routes from `backend/api`. Keep server scaffold, job manager, auth/CORS.
4. Strip git-aware walker + project watchers from `backend/ingestion`. Keep text extraction (PDF/DOCX) + HTTP fetch as `backend/extractors` or merge into `core`.
5. Strip code-routing paths from `backend/enhancement`. Keep context retrieval skeleton.
6. Delete frontend views that are pure code-UI: Workspace, Sources, Graph, Assistant. Flag Activity/Library for redesign in Phase 8.
7. Clean generated artifacts (`dist/`, logs, `server.json`).

Commit each step separately — easy to `git revert` if something turns out to be load-bearing.

## Open classification questions

- `frontend/src/views/ActivityView.tsx` — is this a generic "operations log" or tied to indexing jobs? Read before deciding.
- `frontend/src/views/LibraryView.tsx` — browse UI. Possibly adaptable to a fact browser. Read first.
- `test_enhancement.sh`, `test.toml`, `uninstall-cli.sh` — ad-hoc; read each, keep only what applies after refactor.
- Exact MCP tool surface after refactor — mcp-server stays but the tools it exposes need to align with `Memory.*` contract. Decide in Phase 6 when core integration happens.
