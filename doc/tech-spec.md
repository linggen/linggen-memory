---
type: spec
reader: Coding agent and users
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
│   ├── embed/           # embedding model
│   ├── cli/             # clap subcommand dispatch
│   ├── sessions/        # session scanning / extraction
│   ├── daemon/          # pidfile + lifecycle + serve entry
│   └── http/            # axum router: /api/memory/*, /api/health, UI
├── static/              # Data Browser UI (index.html, app.js, styles.css)
│                        # embedded via rust-embed at compile time
├── doc/
│   ├── product-spec.md  # features and scenarios
│   ├── tech-spec.md     # you are here
│   └── ui-spec.md       # Data Browser layout + interactions
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
    └── memory.lancedb/  # one LanceDB dir; holds the `semantic` + `episodic` tables
```

Multi-user isolation is path-level, not in-row: Linggen sets `LINGGEN_DATA_DIR` per user context before invoking `ling-mem`. The binary is single-user per invocation and has no `user_id` concept.

## Fact schema (13 fields)

LanceDB table name: `semantic` (curated long-term memory; holds both `tier=core` and `tier=semantic` rows).

| Field | Arrow type | Null? | Purpose |
|:--|:--|:--:|:--|
| `id` | Utf8 | no | UUID, fact identity |
| `content` | Utf8 | no | Fact text, self-contained (includes any scoping conditions) |
| `vector` | FixedSizeList<Float32, 1024> | yes | Embedding of `content`. Nullable for brief windows between insert and embed; search filters ignore null-vector rows |
| `contexts` | List<Utf8> | no (may be empty) | Scope tags, hierarchical path-like (`code/linggen`, `music/piano`). Primary filter dimension |
| `tags` | List<Utf8> | no (may be empty) | Secondary metadata. Free-form labels with prefix convention (`intent:learn`, `topic:coding`, `stage:dev`) |
| `type` | Utf8 | no | One of seven canonical values (see below). Utf8 in storage; CLI validates against enum |
| `outcome` | Utf8 | yes | `positive` / `negative` / `neutral`. Only meaningful for action-flavored types |
| `from` | Utf8 | no | `user` / `agent` / `derived`. Defaults to `derived` |
| `tier` | Utf8 | no | `core` / `semantic`. Defaults to `semantic`. Older JSON without it reads as `semantic` |
| `cwd` | Utf8 | yes | Working directory at capture time. Extraction hint and filter |
| `created_at` | Timestamp(Microsecond, UTC) | no | When the fact was added to memory |
| `updated_at` | Timestamp(Microsecond, UTC) | yes | Last-edit time. Doubles as the decay/TTL clock AND the *activity timestamp* `updated_at ?? created_at` that drives list `--sort` and the UI age badge (falls back to `created_at`) |
| `occurred_at` | Timestamp(Microsecond, UTC) | yes | When the thing described happened. Falls back to `created_at` in queries |
| `source_session` | Utf8 | yes | Session id the fact was extracted from. Escape hatch when the fact is later ambiguous |

**Embedding dimension: 1024** (Arrow `FixedSizeList<Float32, 1024>`), determined by the default embedding model (below).

**Not in v0.1** — add with migration when needed: `last_referenced`, `access_count`, `confidence`, `pinned`, explicit `user_id`. (`supersedes` was considered and dropped — conflict resolution is the live `replace_ids` atomic add+delete primitive instead.)

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

- **Default (v0.5+):** `Qwen/Qwen3-Embedding-0.6B`, 1024-dim, multilingual (100+ langs incl. Chinese).
- Local inference via Candle (BF16 weights, ~1.2 GB downloaded on first use). macOS Metal / Linux CUDA / CPU auto-selected.
- Input capped at 512 tokens (`MAX_SEQ_LEN` in `src/embed/mod.rs`) — sized for short atomic facts, not long documents.
- Model cached under the HuggingFace Hub cache dir.
- Queries prefixed `"query: "`, stored passages `"passage: "` per Qwen3's retrieval convention.
- v0.4.x and earlier used `all-MiniLM-L6-v2` (384-dim, English-only); see CHANGELOG for the migration.

If the configured model output dimension doesn't match the table's `FixedSizeList` size, the store refuses to open and prints a migration hint.

## CLI contract

### Subcommands (v0.1)

| Subcommand | Purpose | Flags |
|:--|:--|:--|
| `add` | Insert one or many facts (positional content OR NDJSON on stdin) | `--type`, `--tier` (`core`/`semantic`, default `semantic`), `--context`, `--tag` (repeatable), `--from`, `--outcome`, `--cwd`, `--occurred-at`, `--source-session`, `--stdin`, `--episodic` |
| `get <id>` | Fetch one fact | — |
| `search <query>` | Semantic + filter search | `--context` (repeatable), `--type`, `--tier`, `--from`, `--outcome`, `--since`, `--limit`, `--episodic` |
| `list` | Non-semantic browse | same filters as `search` (incl. `--tier`, `--episodic`), plus `--sort`, `--page`, `--page-size` |
| `update <id>` | Modify fields | `--content`, `--add-context`, `--remove-context`, `--add-tag`, `--remove-tag`, `--type`, `--outcome` |
| `delete <id>` | Hard delete | `--yes` to skip confirmation |
| `forget` | Bulk delete by filter | `--context`, `--type`, `--older-than`; requires `--yes` |
| `evict` | Delete episodic rows older than a cutoff | `--before <rfc3339>` |

In-scope but not listed above (daemon lifecycle): `serve`, `start`, `stop`,
`restart`, `status`. These run the axum HTTP server that hosts both the
REST API and the Data Browser UI. See the [Skill integration](#skill-integration)
section below and `ui-spec.md`.

Deferred: `archive` (soft-delete; may land if `delete` proves insufficient).

**`list` sort order.** `--sort newest|oldest` orders by the *activity
timestamp* (`updated_at ?? created_at`), **not** `occurred_at` — the
consolidator back-dates `occurred_at` to the source session, which would
bury freshly-written rows. The sort runs over the **full filtered set
before** the page window is sliced (no DB-side `limit` pre-sort), so
`list(newest, 1)` and pagination both return the true global order; the
`count` endpoint's `latest_created_at` is the max activity timestamp. Full
scan is acceptable at the v0.1 `<100k`-row scale (same posture as core
reads); revisit with a DB-side sort beyond that.

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

The `semantic` table is created lazily on first write if it doesn't exist. Arrow schema is built from the field list above with explicit null-vs-non-null flags. LanceDB directory: `$LINGGEN_DATA_DIR/memory/memory.lancedb/`.

### Episodic table

A second table `episodic` lives in the same `memory.lancedb` connection, identical schema, holding staged short-term experience awaiting consolidation. Separate table = per-table ANN index isolated from the curated `semantic` index. Past-TTL rows are removed by the `evict` subcommand (the engine owns the TTL policy and passes an absolute cutoff).

### Search

**Hybrid retrieval (Phase 3b)** — each row is scored as **semantic
similarity lifted by an exact-keyword boost**:

```
hybrid = clamp01( cosine + keyword_boost )
keyword_boost = W · (Σ idf of matched query terms) / (Σ idf of all query terms),  W = 0.3
```

Both halves run in-process over the filtered candidate pool: the store is
flat-scanned (no ANN/FTS index), cosine comes from the stored vectors, and
the keyword boost is computed over the same rows as the lexical corpus
(IDF with `+1` smoothing, presence-based, Unicode word tokenizer).
`keyword_boost ∈ [0, W]` is the IDF-weighted *fraction* of the query's words
a row contains — a rare word ("yinyue") dominates the weighting, a common
word ("name", in most rows) has near-zero IDF and barely lifts the score.
Implemented in `src/memory/hybrid.rs`; the cross-table (`both`) path scores
over the combined semantic+episodic pool so the ranking is global, not
per-table-then-merged.

This is deliberately *not* Reciprocal Rank Fusion. RRF ranks by position
only — it has no notion of absolute relevance, so normalizing it makes the
top row always read ~1.0 even for an unrelated query, and it discards the
IDF weighting that keeps common words from inflating matches. The additive
cosine+boost blend keeps an absolute, honest score (and is monotonic with
the row order by construction).

Why in-process, not LanceDB's native FTS index: an FTS index is not updated
on append, so a freshly-written memory would be invisible to keyword search
until a reindex — reintroducing the "my memory isn't findable" failure.
In-process scoring is always current. Revisit at ~100k rows (same flat-scan
threshold the vector path and `list` already flag).

`min_score` gates the **hybrid** score (default `recall_min_score`, 0.6).
A keyword hit whose cosine alone falls under the floor is admitted because
the boost lifts it over the line — e.g. "…male dog named Yinyue…" (cosine
~0.55 to the bare query "dog") clears 0.6 once boosted — while low-cosine
rows with no real keyword match stay filtered. The console passes
`min_score: 0` to show every match for inspection.

SQL metadata filters are applied before scoring:

- `contexts` match: `contexts LIKE '%<tag>%'` OR SQL array-contains, depending on LanceDB version
- `type` match: exact equality
- `occurred_at` range: timestamp comparison with fallback to `created_at` via COALESCE

Rows are returned ordered by hybrid relevance. Each result carries two
non-stored score columns:

- `score` — raw **cosine** similarity (`[0,1]`), the absolute dense-relevance
  signal. Used by the recall hook, CLI text output, and cross-host
  comparisons.
- `hybrid_score` — the blended `cosine + keyword_boost` (`[0,1]`). It is what
  the rows are ordered by (so it is monotonic with rank) and it is absolute
  (an unrelated query shows a low number, not a misleading 1.0). The console
  displays this; cosine moves to the badge tooltip.

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

- `SKILL.md` frontmatter: `provides: [memory]`, `app:` launcher pointing at the daemon's bound port (`http://127.0.0.1:<port>/`), `install: install.sh`, `daemon: { subdir: linggen-memory, port: 9888, healthcheck: /api/health }`.
- Web UI: **served by the daemon itself**. Static HTML/JS/CSS live under `static/` in this repo and are embedded into the binary via `rust-embed`. The skill wrapper does not ship any UI assets — Linggen just opens the daemon URL in an iframe. Calls from the page go to `/api/memory/*` on the same origin; Linggen's `Memory_*` tool dispatch is an alternate entry point that hits the same endpoints.
- `install.sh`: detect platform via `uname`, download matching release binary from GitHub Releases, extract to the skill's `bin/` directory.
- No scripts beyond install — the binary handles everything else.

For Claude Code compatibility, the SKILL.md body documents the CLI so a model invoking the skill via Bash can use it directly (the `Memory_*` tool namespace is a Linggen-only convenience). A CC user gets the same Data Browser at the same URL — all they need is to run `ling-mem serve`.

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

Semver, and the contract is enforced — the binary semver is what plugins/skills range-pin (`^1`), so a wrong bump corrupts user stores.

**Store schema is the hard line.** A monotonic `STORE_SCHEMA_VERSION` (in `schema_version.rs`, recorded in the `<data_dir>/memory/SCHEMA_VERSION` sidecar — see `doc/schema-versioning.md`) is decoupled from the binary semver and gates every open:

- **Nullable column add, with a shipped migration** → migratable → **MINOR** bump. Safe for `^1` auto-update; `STORE_SCHEMA_VERSION` increments and a registered migration runs on open.
- **Required column, rename, type change, vector-dim/model change** → not migratable → **MAJOR** bump. `STORE_SCHEMA_VERSION` increments; the open-time guard refuses an incompatible store rather than corrupting it.

The rule that makes `^1` auto-update safe: **a non-migratable store change is always a MAJOR release — no exceptions.** Majors sit outside the range and are never auto-installed; a manual major jump is caught by the guard, not silently applied.

`1.0.0` baseline: `STORE_SCHEMA_VERSION = 1`. A pre-1.0 (`0.7.x`) store carries no sidecar → classified `Adopt` → stamped `1` on first 1.x open; the Arrow schema is unchanged across the boundary, so the migration is a no-op. The `--migrate-data` subcommand idea is superseded by `ling-mem export | import` (schema-agnostic JSONL — the escape hatch for the non-migratable/MAJOR case).

## Open technical issues

1. **Embedding model bundling vs download.** For v0.1 the model downloads from HuggingFace Hub on first use. For release-grade distribution, bundling the model as a release asset (one more tarball) avoids network at first-run.
2. **macOS Gatekeeper.** Resolved enough to ship: `release.sh` ad-hoc-signs the binary (`codesign --force --sign -`) before tarballing, which clears the Sequoia 26.x "Code Signature Invalid" SIGKILL on downloaded tarball binaries. Apple Developer ID signing + notarization via CI is still the polish item (removes the first-run Gatekeeper prompt entirely); not a 1.0 blocker.

### Resolved

- ~~**lance × rustc 1.94 recursion-limit error.**~~ Occurred with `lancedb = "0.22"` through `"0.20"` on rustc 1.94. Resolved by upgrading to `lancedb = "0.27"` / `lance = "4.x"`.
