# Claude Code Instructions

## Purpose

linggen-memory is the **default memory-skill backend for Linggen** — a standalone Rust binary (`ling-mem`) that implements the `Memory.*` tool family (add, search, list, update, delete, forget) over a LanceDB store with semantic retrieval.

Specs for this repo:
- `doc/product-spec.md` — features, user-facing behavior, scenarios
- `doc/tech-spec.md` — schema, storage, CLI contract, release process
- `DESIGN.md` — rolling locked-decisions log (what was chosen and why)

The main Linggen repo owns the integration side (see `../linggen/doc/memory-spec.md` and `../linggen/doc/skill-spec.md` for the contract this binary must satisfy).

## Status

- **Branch:** `memory-refactor` (active); `main` still reflects the archived code-indexing tool.
- **Phase:** Phase 2 — memory-flavored schema + CRUD (see `~/.claude/plans/memory-system-rebuild.md`).
- **Legacy tree preserved at:** `v0-legacy` git tag.
- **Parallel work:** The Linggen built-in side (core markdown, `provides:` field, `Memory.*` dispatch, migration) is being done in a parallel Claude session on the main `linggen/` repo. Defer questions about Linggen-core internals to that session.

## Repo layout (single-crate, flat)

```
linggen-memory/
├── Cargo.toml       # single crate at root
├── src/             # all Rust code
├── webui/           # Vite + TS scaffold (Phase 8 rebuilds)
├── doc/             # product-spec.md + tech-spec.md
├── scripts/         # build + release
├── assets/          # icon etc.
├── DESIGN.md        # locked decisions
├── README.md
└── CLAUDE.md        # you are here
```

No workspace, no sub-crates. Anything new goes in `src/<module>/`.

## Scope

**In scope for this repo:**
- LanceDB store + embedding pipeline
- CLI subcommands on `ling-mem`
- HTTP daemon + webpage (Phase 8)
- Platform release binaries (cross-compile via GitHub Actions)
- All docs under `doc/` about the binary itself

**Out of scope (lives in `../linggen/`):**
- Linggen core's `provides:` field and Memory.* tool dispatch
- `~/.linggen/core/identity.md` + `style.md` markdown scaffolding
- Migration from the old 5-file markdown memory
- Spec docs for the Linggen-memory integration contract (`linggen/doc/memory-spec.md`)

## Working rules

- **Single-crate discipline.** Don't re-introduce a workspace or sub-crates. Modules under `src/<name>/` are the unit of organization.
- **Commit per sub-step.** Each item in `~/.claude/plans/memory-system-rebuild.md` Phase 2 gets its own commit: facts types (done), Arrow schema, FactsStore basic ops, search + filter, update/delete/forget, CLI dispatch.
- **Don't edit `v0-legacy`.** Tag is immutable; legacy source is `git checkout v0-legacy` away.
- **Preserve release tooling.** `scripts/build-*.sh`, signing config, and CI live through the refactor.
- **Dependency policy.** Use current major versions; `thiserror` is 2.x. LanceDB + Arrow must be a compatible triple — track the lance 0.39 × rustc 1.94 recursion-limit issue separately.
- **CLI is primary.** All features must be reachable via `ling-mem` subcommands. The `Memory.*` tool-namespace dispatch in Linggen is sugar; the CLI must be complete on its own so Claude Code (Bash-only) can use it.

## Related

- Main Linggen repo: `../linggen/` (or `~/workspace/linggen/linggen/`)
- Thin skill wrapper (lives in the main repo): `../skills/memory/` — downloads ling-mem binary via `install.sh`
- Plan: `~/.claude/plans/memory-system-rebuild.md`
