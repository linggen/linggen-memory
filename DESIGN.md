# linggen-memory — v0.1 design

Locked decisions from the Phase 2 design discussions. Use this as the reference for implementation; deviations need explicit discussion.

See also:
- `CLAUDE.md` — purpose + working rules
- `CLEANUP.md` — Phase 1 file-by-file classifications (now complete)
- `~/.claude/plans/memory-system-rebuild.md` — the ten-phase plan
- Main Linggen repo: `linggen/doc/memory-spec.md`, `linggen/doc/skill-spec.md`

## Binary and CLI

- **Binary name:** `ling-mem` (already the name in `backend/api/Cargo.toml`). "linggen-memory" remains the repo / product name.
- **v0.1 is CLI-only.** No daemon / `serve` subcommand. Daemon mode lands in Phase 8 for the webpage.
- **Usage pattern:** single-user per invocation. Linggen (or Claude Code) invokes `ling-mem` as a subprocess; the binary opens LanceDB, runs the op, writes JSON to stdout, closes, exits.

## Storage

- **Fresh LanceDB table: `facts`.** Do not reuse `chunks` or `internal_chunks` (both were shaped for code indexing).
- **Data root:** `$LINGGEN_DATA_DIR` (existing convention).
- **Store path:** `$LINGGEN_DATA_DIR/memory/facts.lancedb/`.
- **No `user_id` field, no `--user-id` flag.** Multi-user isolation is done at the filesystem level: Linggen sets `LINGGEN_DATA_DIR` per user (e.g. `~/.linggen/users/<uid>/`). The binary sees one user's data per invocation.

## Schema — 12 fields

| Field | Arrow type | Null? | Purpose |
|:--|:--|:--:|:--|
| `id` | Utf8 | no | UUID, fact identity |
| `content` | Utf8 | no | Fact text, self-contained (includes scoping conditions when present) |
| `vector` | FixedSizeList<Float32, 384> | yes* | Embedding; may be unset briefly between insert and embed |
| `contexts` | List<Utf8> | no (may be empty) | Scope tags, hierarchical path-like (`code/linggen`, `music/piano`) |
| `tags` | List<Utf8> | no (may be empty) | Secondary metadata. Free-form labels; prefix convention for structure (`intent:learn`, `topic:coding`, `stage:dev`) |
| `type` | Utf8 | no | One of the 7 canonical values (below); validated in CLI, Utf8 in storage |
| `outcome` | Utf8 | yes | `positive` / `negative` / `neutral`. Nullable — only meaningful for action-flavored types |
| `from` | Utf8 | no | `user` / `agent` / `derived`. Defaults to `derived` if unspecified |
| `cwd` | Utf8 | yes | Working directory when captured. Nullable — not every fact has one |
| `created_at` | Timestamp(Microsecond, UTC) | no | When the fact was added to memory |
| `occurred_at` | Timestamp(Microsecond, UTC) | yes | When the described thing happened. Falls back to `created_at` in queries |
| `source_session` | Utf8 | yes | Session ID the fact was extracted from. Nullable — manual adds have none |

Deferred (add later, not now): `last_referenced`, `access_count`, `supersedes`, `confidence`, `pinned`, explicit `user_id`.

### `type` — seven canonical values

| Value | Meaning |
|:--|:--|
| `fact` | Stable truth about user / world (identity, domain facts, hobbies) |
| `preference` | How the user wants the agent to work (cross-project behavioral rules) |
| `decision` | A choice plus its reasoning |
| `tried` | An attempt (pair with `outcome`) — failed attempts prevent dead-end repeats |
| `fixed` | A bug + symptoms + fix in `content`; pair with `outcome` |
| `learned` | Cross-project env/tool gotcha |
| `built` | A specific thing shipped (narrow — not a catch-all activity log) |

No `activity` catch-all. Forces extraction toward specific sub-types.

CLI validates the type at arg parse time; unknown values from upstream sources coerce to `fact` with a warning on stderr.

## Embedding model

- **v0.1 default:** `sentence-transformers/all-MiniLM-L6-v2`, 384-dim (unchanged from the archived code).
- Configurable via `--embedding-model <hf-id>` flag or `LING_MEM_EMBEDDING_MODEL` env var. Default flip in v0.2 to `BAAI/bge-small-en-v1.5` (same 384-dim, drop-in).
- Model cache location follows existing logic in `backend/embeddings/` (HF_HOME under `$LINGGEN_DATA_DIR/hf/hub/`).

## CLI I/O contract

- **Primary output: NDJSON on stdout**, one object per row for list results, single object for single-row results.
- **Errors: JSON on stderr + non-zero exit.** `{"error": "...", "code": "NOT_FOUND"}`.
- **Human-readable format:** `--format=text` or `--format=table` renders a friendly view.
- **Bulk input:** `add` and `update` accept NDJSON on stdin when no positional content is given.
- **Quiet / verbose:** `--quiet` suppresses progress; `-v` prints debug to stderr.

### Subcommands (v0.1)

| Subcommand | Purpose |
|:--|:--|
| `add` | Insert one or many facts (positional content OR stdin NDJSON) |
| `get <id>` | Fetch one fact by id |
| `search <query>` | Semantic + filter search. Flags: `--context`, `--type`, `--from`, `--outcome`, `--since`, `--limit` |
| `list` | Non-semantic browse with same filters, sortable by `--sort=created_at\|occurred_at`, paginated |
| `update <id>` | Modify content, tags, contexts, type, outcome |
| `delete <id>` | Hard delete with tombstone |
| `forget` | Bulk delete by filter (`--context`, `--type`, `--older-than`) — confirmation required |
| `archive <id>` | Deferred — skip in v0.1 (rely on `delete` for now) |

`archive` is deferred since we explicitly skipped soft-delete for v0.1. `collect` and `extract` subcommands are Phase 3.

## Skill shape (for reference)

`ling-mem` must be usable in three modes simultaneously — Linggen web app, Linggen standard skill, Claude Code standard skill. This is settled (see `~/.claude/projects/-Users-lianghuang-workspace-linggen/memory/project_linggen_memory_skill_shape.md`) and constrains the CLI as the primary interface — capability routing in Linggen is syntactic sugar on top.

## What "useful" memory looks like

Every fact must have a **retrieval trigger** that fires in future unrelated sessions. If you can't imagine when a future session would recall it, it's not memory.

Useful categories (map directly to the 7 types above):
- Identity · preferences · symptom-indexed fixes · failed attempts · decisions with reasoning · domain facts · env/tool gotchas · specific deliverables shipped

Drift-prone categories (do not store as-is):
- Weekly status · current state · conversation micro-details

## Open items for v0.2+ (reference only)

- `access_count` + `last_referenced` (frecency)
- `pinned` boolean for user curation
- `supersedes` for edit history
- Daemon / `serve` subcommand (Phase 8)
- `archive` subcommand for soft-delete
- `collect` + `extract` subcommands (Phase 3)
- Embedding-model upgrade to `bge-small-en-v1.5`
- User-level isolation handled entirely via `$LINGGEN_DATA_DIR` (no in-binary logic)

---

## Phase 2 implementation starting point

Single-crate layout now (no sub-workspaces). Everything lives in `src/`:

1. `src/facts/types.rs` — `Fact` struct + `FactType`/`Outcome`/`Origin` enums. **Done** — 9 unit tests passing.
2. `src/facts/schema.rs` — Arrow/LanceDB schema for the `facts` table (12-column Arrow schema, `FixedSizeList<Float32, 384>` vector).
3. `src/facts/store.rs` — `FactsStore` wrapping a LanceDB connection with `open()`, `insert()`, `get()`, `search()`, `list()`, `update()`, `delete()`, `forget()`.
4. `src/cli/` — clap subcommand dispatch, wired from `src/main.rs`.
5. Integration tests using `tempfile::TempDir` to spin up an isolated LanceDB per test.

### Dependency resolution log

- `lancedb = "0.22"` (first attempt) → tripped lance 0.39 recursion-limit error on rustc 1.94.
- `lancedb = "0.20"` (tried older) → tripped lance 0.29 with the same error.
- **`lancedb = "0.27"` + `arrow = "56"`** (the resolution) → pulls `lance = "4.x"`, builds clean on rustc 1.94. The lance 4.0 major bump fixed the recursion-depth issue upstream.
