# Claude Code Instructions

## Purpose

linggen-memory is the **default memory-skill backend for Linggen** — a standalone Rust binary (`ling-mem`) that implements the `Memory_*` tool family (add, search, list, update, delete, forget) over a LanceDB store with semantic retrieval.

Specs for this repo:
- `doc/product-spec.md` — features, user-facing behavior, scenarios
- `doc/tech-spec.md` — schema, storage, CLI contract, release process
- `DESIGN.md` — rolling locked-decisions log (what was chosen and why)

The main Linggen repo owns the integration side (see `../linggen/doc/memory-spec.md` and `../linggen/doc/skill-spec.md` for the contract this binary must satisfy).

## Status

- **Branch:** `main` — commit & push directly on `main`, never branch. The archived code-indexing tool is preserved at the `v0-legacy` tag.

## Scope

**In scope for this repo:**
- LanceDB store + embedding pipeline
- CLI subcommands on `ling-mem`
- (out-of-scope: HTTP daemon + webpage — lives in the memory skill wrapper)
- Platform release binaries (cross-compile via GitHub Actions)
- All docs under `doc/` about the binary itself

**Out of scope (lives in `../linggen/`):**
- Linggen core's `provides:` field and Memory_* tool dispatch
- Core-prompt injection (the engine loads `tier=core` rows at session start — there is no `identity.md`/`style.md` markdown substrate; that cutover shipped)
- Spec docs for the Linggen-memory integration contract (`linggen/doc/memory-spec.md`)

## Working rules

- **Single-crate discipline.** Don't re-introduce a workspace or sub-crates. Modules under `src/<name>/` are the unit of organization.
- **Commit per sub-step.** Each checklist item / sub-step in `~/.claude/plans/memory-system-rebuild.md` gets its own commit (e.g. the Phase 1a substeps each landed as one commit: add `updated_at` field, bump on update, episodic evict + tier-core query, retune dedup threshold). Build + relevant `cargo test` green before each commit.
- **Don't edit `v0-legacy`.** Tag is immutable; legacy source is `git checkout v0-legacy` away.
- **Preserve release tooling.** `scripts/build-*.sh`, signing config, and CI live through the refactor.
- **Dependency policy.** Use current major versions; `thiserror` is 2.x. LanceDB + Arrow must be a compatible triple — track the lance 0.39 × rustc 1.94 recursion-limit issue separately.
- **CLI is primary.** All features must be reachable via `ling-mem` subcommands. The `Memory_*` tool-namespace dispatch in Linggen is sugar; the CLI must be complete on its own so Claude Code (Bash-only) can use it.

## Related

- Main Linggen repo: `../linggen/` (or `~/workspace/linggen/linggen/`)
- Thin skill wrapper (lives in the main repo): `../skills/memory/` — downloads ling-mem binary via `install.sh`
- Plan: `~/.claude/plans/memory-system-rebuild.md`
