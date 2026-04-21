# Claude Code Instructions

## Purpose

linggen-memory is the **default memory-skill backend for Linggen** — a standalone Rust binary that implements the `Memory.*` tool family (add, search, list, update, archive, delete, forget) over a LanceDB store with semantic retrieval.

See `linggen/doc/memory-spec.md` and `linggen/doc/skill-spec.md` in the main Linggen repo for the contract this binary must satisfy.

## Status

Under active refactor (branch `memory-refactor`). The repo's prior purpose was a code-indexing tool; the current purpose is general-purpose semantic memory. The full-featured legacy state is preserved at the `v0-legacy` tag.

Refactor plan: see `CLEANUP.md` at repo root and `~/.claude/plans/memory-system-rebuild.md` in the user's Claude workspace.

## Scope

**In scope for this repo:**
- LanceDB store + embedding pipeline
- CLI subcommands (add / get / search / list / update / archive / delete / forget / collect / extract / serve)
- HTTP daemon serving an embedded markdown-editor webpage
- Platform-specific release binaries (cross-compiled via GitHub Actions)

**Out of scope:**
- Local LLM inference (Linggen handles chat; this binary is store + search only)
- Code-specific indexing (tree-sitter, AST, language detection) — legacy feature
- Project-based scoping (contexts are now N:M tags on facts, not 1:1 dirs)

## Working rules

- **Branch discipline:** ongoing work lives on `memory-refactor` until Phase 1–5 of the plan complete, then merges to `main`.
- **Commit per phase step:** each item in the plan's execution order gets its own commit. Easier to revert if something turns out to be load-bearing.
- **Don't edit `v0-legacy`:** tag is immutable; legacy source is accessible via `git checkout v0-legacy` when needed.
- **Preserve release tooling:** `scripts/build-*.sh`, signing config, and CI matrix stay intact — they're reused verbatim for the new binary.

## Related

- Main Linggen repo: `../linggen/` (or `~/workspace/linggen/linggen/`)
- Thin skill wrapper: `~/workspace/linggen/skills/memory/` (downloads this binary via install.sh)
