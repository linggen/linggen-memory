---
type: spec
audience: implementation — schema, internals, contracts
---

# linggen-memory — tech spec (v0.1)

Locked technical decisions for the v0.1 rebuild. Deviations need explicit discussion; update this file when decisions change. Newer product-level context lives in `product-spec.md`; the rolling "locked decisions log" with the history of how we got here lives in `DESIGN.md` at the repo root.

## Repo layout

Single crate, flat tree:

```
linggen-memory/
├── Cargo.toml           # single crate (no workspace)
├── src/
│   ├── main.rs          # ling-mem binary entry
│   ├── lib.rs           # public API
│   ├── facts/           # Fact types, enums, Arrow/LanceDB plumbing
│   ├── embed/           # embedding model (added in store commit)
│   ├── store/           # FactsStore — LanceDB open/insert/search/...
│   ├── cli/             # clap subcommand dispatch
│   └── server/          # axum scaffold for webpage (Phase 8 active use)
├── webui/               # Vite + TypeScript shell (Phase 8 rebuilds)
├── doc/
│   ├── product-spec.md  # this file's sibling
│   └── tech-spec.md     # you are here
├── scripts/             # build + release (cross-compile)
├── assets/              # icon.icns etc.
├── DESIGN.md            # rolling locked-decisions log
├── CLAUDE.md            # repo-level agent instructions
└── README.md
```

Binary name: **`ling-mem`**. "linggen-memory" is the repo / product name; the executable is always the short form.

## Data directory

Respect `LINGGEN_DATA_DIR` env var (convention shared with the main Linggen binary).

```
$LINGGEN_DATA_DIR/
└── memory/
    └── facts.lancedb/   # LanceDB table directory
```

Multi-user isolation is path-level, not in-row: Linggen sets `LINGGEN_DATA_DIR` per user context before invoking `ling-mem`. The binary is single-user per invocation and has no `user_id` concept.

## Fact schema (12 fields)

LanceDB table name: `facts`.

| Field | Arrow type | Null? | Purpose |
|:--|:--|:--:|:--|
| `id` | Utf8 | no | UUID, fact identity |
| `content` | Utf8 | no | Fact text, self-contained (includes any scoping conditions) |
| `vector` | FixedSizeList<Float32, 384> | yes | Embedding of `content`. Nullable for brief windows between insert and embed; search filters ignore null-vector rows |
| `contexts` | List<Utf8> | no (may be empty) | Scope tags, hierarchical path-like (`code/linggen`, `music/piano`). Primary filter dimension |
| `tags` | List<Utf8> | no (may be empty) | Secondary metadata. Free-form labels with prefix convention (`intent:learn`, `topic:coding`, `stage:dev`) |
| `type` | Utf8 | no | One of seven canonical values (see below). Utf8 in storage; CLI validates against enum |
| `outcome` | Utf8 | yes | `positive` / `negative` / `neutral`. Only meaningful for action-flavored types |
| `from` | Utf8 | no | `user` / `agent` / `derived`. Defaults to `derived` |
| `cwd` | Utf8 | yes | Working directory at capture time. Extraction hint and filter |
| `created_at` | Timestamp(Microsecond, UTC) | no | When the fact was added to memory |
| `occurred_at` | Timestamp(Microsecond, UTC) | yes | When the thing described happened. Falls back to `created_at` in queries |
| `source_session` | Utf8 | yes | Session id the fact was extracted from. Escape hatch when the fact is later ambiguous |

**Embedding dimension: 384** (Arrow `FixedSizeList<Float32, 384>`), determined by the default embedding model (below).

**Not in v0.1** — add with migration when needed: `last_referenced`, `access_count`, `supersedes`, `confidence`, `pinned`, explicit `user_id`.

## Canonical `type` values

Seven values, validated at the CLI boundary with clap's `ValueEnum`. Storage keeps Utf8 — unknown values from upstream extractors coerce to `fact` with a stderr warning.

| Value | Meaning |
|:--|:--|
| `fact` | Stable truth about user / world (identity, domain facts, hobbies) |
| `preference` | How the user wants the agent to work — cross-project behavioral rules |
| `decision` | A choice plus its reasoning |
| `tried` | An attempt (pair with `outcome`) |
| `fixed` | A bug + symptoms + fix (pair with `outcome`) |
| `learned` | Cross-project env / tool gotcha |
| `built` | A specific thing shipped (narrow — not an activity catch-all) |

No `activity` catch-all. Forcing specificity prevents drift we saw in v0's markdown files.

## Embedding model

- **v0.1 default:** `sentence-transformers/all-MiniLM-L6-v2`, 384-dim.
- Local inference via Candle + HuggingFace Hub.
- Model cached under `$LINGGEN_DATA_DIR/hf/hub/` (HuggingFace convention).
- Configurable via `--embedding-model <hf-id>` flag or `LING_MEM_EMBEDDING_MODEL` env var.
- Planned v0.2 default flip: `BAAI/bge-small-en-v1.5` (same 384-dim, drop-in quality upgrade).

If the configured model output dimension doesn't match the table's `FixedSizeList` size, the store refuses to open and prints a migration hint.

## CLI contract

### Subcommands (v0.1)

| Subcommand | Purpose | Flags |
|:--|:--|:--|
| `add` | Insert one or many facts (positional content OR NDJSON on stdin) | `--type`, `--context`, `--tag` (repeatable), `--from`, `--outcome`, `--cwd`, `--occurred-at`, `--source-session`, `--stdin` |
| `get <id>` | Fetch one fact | — |
| `search <query>` | Semantic + filter search | `--context` (repeatable), `--type`, `--from`, `--outcome`, `--since`, `--limit` |
| `list` | Non-semantic browse | same filters as `search`, plus `--sort`, `--page`, `--page-size` |
| `update <id>` | Modify fields | `--content`, `--add-context`, `--remove-context`, `--add-tag`, `--remove-tag`, `--type`, `--outcome` |
| `delete <id>` | Hard delete | `--yes` to skip confirmation |
| `forget` | Bulk delete by filter | `--context`, `--type`, `--older-than`; requires `--yes` |

Deferred to later phases (same binary): `archive`, `collect`, `extract`, `serve`.

### I/O contract

- **Default stdout:** NDJSON — one JSON object per line for list-like results; single object for single-row results.
- **Human stdout:** `--format=text` or `--format=table` renders friendly output.
- **Stderr:** JSON errors — `{"error":"...","code":"NOT_FOUND"}` — with non-zero exit code.
- **Bulk input:** `add` and `update` accept NDJSON on stdin when no positional content is given (one fact per line, fields match the serde JSON shape).
- **Verbosity:** `--quiet` suppresses progress; `-v` prints debug context to stderr.

### JSON shape on the wire

Fact serialization matches the Rust `Fact` struct with one rename: the Rust field `origin` serializes as `"from"` (the keyword clash is an implementation detail). Null-valued optional fields are omitted entirely from JSON output.

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "content": "Prefers debug-level logging during dev stage",
  "contexts": ["code/linggen"],
  "tags": ["topic:logging", "stage:dev"],
  "type": "preference",
  "from": "user",
  "created_at": "2026-04-21T10:30:00.000Z"
}
```

## Storage internals

### LanceDB table creation

The `facts` table is created lazily on first write if it doesn't exist. Arrow schema is built from the field list above with explicit null-vs-non-null flags. Table directory: `$LINGGEN_DATA_DIR/memory/facts.lancedb/`.

### Search

Vector similarity via LanceDB's native nearest-neighbor, filtered in SQL against metadata columns. Filters:

- `contexts` match: `contexts LIKE '%<tag>%'` OR SQL array-contains, depending on LanceDB version
- `type` match: exact equality
- `occurred_at` range: timestamp comparison with fallback to `created_at` via COALESCE

Returns top-`limit` rows ordered by similarity score, including the score as a non-stored result column.

### Deletion

Hard delete only in v0.1. LanceDB's `delete_by_id` semantics + table rewrite if fragmentation grows.

## Dependency policy

Pinned in `Cargo.toml`:

- `lancedb = "0.27"` paired with `arrow = "56"` (matching triple for `arrow-array` / `arrow-schema`). This combination pulls in `lance = "4.x"`, which resolves the recursion-limit issue that older lance versions tripped on rustc 1.94.
- `tokio` full features.
- `thiserror` 2.x (upgraded from the archived 1.x).
- `clap` 4.x derive.
- `serde`, `serde_json`, `uuid`, `chrono`, `tracing`, `tracing-subscriber`, `futures` — current stable, caret-ranged so cargo picks the latest compatible minor.

Release profile: `strip = true`, `lto = "thin"`, `codegen-units = 1`.

## Skill integration

The **linggen-memory skill** is a thin wrapper in the main Linggen repo's skills tree. Its responsibilities:

- `SKILL.md` frontmatter: `provides: [memory]`, `app:` launcher pointing at `ling-mem serve`, `install: scripts/install.sh`.
- `install.sh`: detect platform via `uname`, download matching release binary from GitHub Releases, extract to the skill's `bin/` directory.
- No scripts beyond install — the binary handles everything else.

For Claude Code compatibility, the SKILL.md body documents the CLI so a model invoking the skill via Bash can use it directly (the `Memory.*` tool namespace is a Linggen-only convenience).

## Release process

GitHub Actions cross-compile on tag push (`v*`):

| Target triple | User platform |
|:--|:--|
| `aarch64-apple-darwin` | macOS Apple Silicon |
| `x86_64-apple-darwin` | macOS Intel |
| `x86_64-unknown-linux-gnu` | Linux x86_64 |
| `aarch64-unknown-linux-gnu` | Linux ARM64 |

Each release asset: `ling-mem-<target>.tar.gz` with the binary + LICENSE + a minimal `README`. Code signing (macOS Developer ID) is optional for v0.1 but recommended — existing `SIGNING.md` has the playbook.

## Versioning

Semver. Breaking schema changes (adding / removing / retyping a LanceDB column) bump the minor version pre-1.0. Add a `--migrate-data` subcommand on the first such bump and keep backwards-read capability for one minor version.

## Open technical issues

1. **Embedding model bundling vs download.** For v0.1 the model downloads from HuggingFace Hub on first use. For release-grade distribution, bundling the model as a release asset (one more tarball) avoids network at first-run.
2. **macOS Gatekeeper on unsigned releases.** v0.1 plan: document the "control-click → Open" workaround. v0.2: Apple Developer ID signing via CI.

### Resolved

- ~~**lance × rustc 1.94 recursion-limit error.**~~ Occurred with `lancedb = "0.22"` through `"0.20"` on rustc 1.94. Resolved by upgrading to `lancedb = "0.27"` / `lance = "4.x"`.
