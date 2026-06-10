# Store schema versioning & upgrade guard

Status: design (v1 scope). Owner: ling-mem binary.

## Why

ling-mem is distributed as a binary pinned by the shared-memory plugin/skill.
To adopt semver auto-update (`^1`: take patch/minor automatically), a binary
upgrade must never silently break or wipe a user's store. Today there is no
store-version concept — only two ad-hoc checks in `store.rs`
(`ensure_late_schema_additions` adds a nullable `host` column;
`check_schema_dim` hard-errors on vector-dim mismatch). The earlier `tier`
incident broke because a *required* column was added with no migration and no
version gate, forcing a wipe-and-fresh.

This guard makes upgrades data-safe in both directions and is the prerequisite
that lets the plugin pin a range instead of an exact version.

## Store schema version

A monotonic `STORE_SCHEMA_VERSION: u32`, **separate from the binary semver**.
Slow-moving — bumped only when the on-disk store layout changes. Persisted as a
sidecar file (decision: sidecar is the source of truth, mirrored into the Arrow
table metadata as a backup check):

```
~/.linggen/memory/
├── memory.lancedb/      # the store
└── SCHEMA_VERSION       # sidecar: a single integer, e.g. "1\n"
```

Sidecar because it is readable *before* opening LanceDB — the open is the step
we are gating — and it does not depend on LanceDB internals.

Resolution rules for the on-disk version:
- file present  → parse the integer.
- file absent, store **non-empty** → infer `0` (legacy, pre-versioning) and enter
  the migration path.
- file absent, store **empty/fresh** → write `STORE_SCHEMA_VERSION`.

On every successful open/migrate, (re)write the sidecar and stamp the Arrow
schema metadata key `schema_version`.

## Binary-declared constants

```rust
const STORE_SCHEMA_VERSION: u32 = 1;   // version this binary writes
const MIN_READABLE_SCHEMA:  u32 = 0;   // oldest version it can open / migrate up from
```

## Open-time guard (folded into `store.rs::open_named`)

```
on_disk = read_schema_version()
match on_disk.cmp(&STORE_SCHEMA_VERSION) {
  Equal   => open normally
  Less    => if on_disk >= MIN_READABLE_SCHEMA {
               run_migrations(on_disk -> CURRENT); write_sidecar(CURRENT)
             } else {
               refuse: "store schema v{on_disk} is older than this build supports;
                        `ling-mem export <file>`, reset, then `ling-mem import`"
             }
  Greater => refuse: "store written by a newer ling-mem (schema v{on_disk}).
                      Upgrade the binary: `ling-mem upgrade`.
                      Refusing to open to protect your memory."
}
```

The `Greater` branch is what protects against multi-channel version skew on one
host (e.g. CC plugin's binary vs Codex plugin's binary opening the same store) —
the older binary refuses rather than writing old-shape rows into a newer store.

The two existing checks stay alongside the guard: `ensure_late_schema_additions`
keeps running idempotently on every open (additive nullable columns don't need
a version bump); `check_schema_dim` (embedding-model/dim change) is a
non-migratable break — a future one ships as a MAJOR with a refused open.

## Migration registry

`schema_version::run_migrations(from)` is the dispatch point, invoked by
`store.rs::open_named` for any `Compat::Migrate` store before stamping. The
registry is **empty at the v1 baseline**: fine-grained additive column changes
(e.g. the nullable `host` column) stay in `ensure_late_schema_additions`,
which runs idempotently on every open. When `STORE_SCHEMA_VERSION` bumps, the
step migrating the previous version must be registered in `run_migrations` —
reaching it without one is a release bug and errors instead of stamping an
unmigrated store.

## Discipline: what a version bump means (the semver contract)

| Store change                                            | Migratable? | semver | Auto-update via `^1`? |
|---------------------------------------------------------|-------------|--------|-------------------------|
| Add **nullable** column (+ shipped migration)           | yes         | minor  | yes — flows free        |
| Add **required** col / rename / type / vector-dim change | no          | major  | no — explicit pin bump  |

Rule: **a non-migratable store change is a MAJOR version bump, no exceptions.**
Majors sit outside `^1`, so auto-update never crosses an incompatible store.
A *manual* major jump still cannot corrupt data — the open-time guard refuses.
Belt and suspenders.

## export / import (in v1 scope)

Schema-agnostic JSONL dump/load — the escape hatch that turns a non-migratable
major break from "data loss" into "one inconvenient round-trip":

```
ling-mem export <file.jsonl>     # one JSON object per fact, all tiers; no vectors required
ling-mem import <file.jsonl>     # re-inserts; vectors re-embedded on import
```

- Export is schema-version-agnostic: it reads whatever columns exist and writes
  the logical fact (id, content, tier, type, tags, contexts, timestamps, …).
- Import targets the current schema, re-embedding `content` to populate
  `vector` (so a model/dim change is recoverable, just lossy on the old vectors).
- Recovery flow for a major break: `export` → `rm -rf memory.lancedb` → upgrade
  binary → `import`.

## Surfaced state

Extend `status` / `upgrade --check` JSON:

```json
{ "version": "1.1.10", "store_schema": 1, "binary_schema": { "writes": 1, "min_readable": 0 } }
```

Feeds the autostart daemon-version reconcile (already restarts on mismatch) and a
future website "store compatible?" indicator.

## Code touchpoints

- `src/memory/schema_version.rs` — constants, sidecar read/write,
  `enum Compat { Current, Migrate, Adopt, TooOld, TooNew }`, `classify()`,
  `refuse_message()`, `run_migrations()` (empty registry at the v1 baseline).
- `src/memory/store.rs::open_named` — classify + refuse before connecting;
  `run_migrations` for `Migrate` stores, then `stamp_current` after a
  successful open. `ensure_late_schema_additions` + `check_schema_dim` stay
  as separate per-open checks.
- `src/cli` — `export` / `import` subcommands; `--schema-version`; extend
  `status` JSON with `store_schema` + `binary_schema`.
- (optional) `src/update` — `upgrade` compares candidate `min_readable` vs
  on-disk as a second gate; the range pin already blocks major jumps.

## Out of scope (v1)

- Automatic re-embedding migrations across models (export/import covers it).
- Cross-device store sync/merge — the store stays local per `~/.linggen`.
