---
type: design
reader: Coding agent and Liang
guide: |
  Design for the `shared-memory` skill. Says what to build and the open
  decisions that gate it. Aligns to — never duplicates — the canonical
  contract in linggen/doc/memory-spec.md and the binary in linggen-memory.
  Brief. No justification prose beyond what an open decision needs.
---

# shared-memory — design

> **Status: content workstreams B/A/C/D/E LANDED (2026-05-20).**
> Workstream F (distribution migration: rename roll-out + version-skew
> handshake) is the remaining work and ships separately. Canonical
> memory contract is still `linggen/doc/memory-spec.md`; binary
> contract is `linggen-memory/doc/tech-spec.md` + `DESIGN.md`. This
> doc only covers the *third-party host bridge*. §7 resolved; §8
> version-skew still gates the release.

## 1. What this is

The cross-agent bridge skill for **non-Linggen hosts** (Claude Code,
Codex, OpenClaw). One user memory, shared across every AI tool.

Three distinct names — keep them straight everywhere:

- **`ling-mem`** — the binary. LLM-free mechanical store (semantic +
  episodic tables, `search_scored` / `insert_with_dedup` / `--supersedes`
  / `evict`). Name unchanged by this rename.
- **built-in memory** — the Linggen engine's encoder + `dream` mission.
  Linggen-only.
- **`shared-memory`** — this skill. A host adapter, **not** a memory
  system and **not** a separate store. Same `~/.linggen/memory/
  memory.lancedb` as Linggen.

## 2. Coexistence model (affirmed — do not re-litigate)

- **One store**: `~/.linggen/memory/memory.lancedb` (semantic +
  episodic), owned by the binary, outside the skill bundle.
- **Per-host wake-encode**: each host encodes **its own** sessions —
  the Linggen engine for Linggen sessions, this skill for CC / Codex /
  OpenClaw sessions.
- **Two dreams, one consolidate/evict contract.** The *Linggen* `dream`
  mission is **RAG-only** — consolidate + evict over the shared store;
  it doesn't scan because the engine already holds Linggen's live
  exchange and encodes it directly (`f915e6b`). The *shared-memory
  skill* `dream` does **scan + process**: a non-Linggen host hands the
  skill no live exchange (it doesn't own the host's agent loop), so the
  skill's per-host in-host encode reads *its own host's* session files
  (CC/Codex/OpenClaw) → extract → judge/write → then the *same*
  consolidate + evict. Same back-half; only the skill adds the scan
  front-half.
- **This is the coexistence model, not an exception.** `f915e6b` scopes
  "no log-scraping" to the *engine* (it has a live exchange) and to
  Linggen not reaching into other tools. A skill reading *its own
  host's* sessions is the sanctioned **per-host in-host encode** —
  cross-tool memory still emerges from the one shared store; no host
  reads another tool's logs.
- **Binary/judgment split unchanged**: binary = mechanical; judgment
  (encode filter, reconcile, consolidate) = the host LLM. On Linggen
  that is the engine; on other hosts the host's own model drives the
  skill's prompts.

## 3. Rename (`ling-mem` skill → `shared-memory`)

Distribution-level change — **needs its own migration plan** (separate
artifact). Surfaces touched: skills repo dir, `vendor/skills` submodule,
`~/.linggen/skills`, `install.sh`, install URLs, ClawHub listing
(currently soft-deleted), `linggen-vscode` (consumes the bundle via
`install-ling-mem.sh`), docs, existing installs.

- Binary name and `provides: [memory]` **unchanged** — only the skill
  renames.
- Old slug keeps a redirect where the channel supports it (e.g.
  `clawhub rename`).
- Existing installs: `install.sh` does an idempotent in-place rename of
  `~/.linggen/skills/ling-mem` → `shared-memory`. Memory data is under
  `~/.linggen/memory` (separate) — never touched.

## 4. `/shared-memory dream` — scan + process

The skill is the **per-host in-host encoder** for non-Linggen hosts. It
isn't handed a live exchange (it doesn't own the host's agent loop), so
it encodes by reading *that host's own* session files. This is the
coexistence model's per-host wake-encode — not Linggen reaching into
other tools, and not in tension with `f915e6b` (engine-scoped).

`/shared-memory dream` = scan → extract → judge/write → consolidate +
evict.

**Scan + extract (script, token-cheap).** A script parses the host's
on-disk transcripts, strips tool noise, hash-dedups, secret-filters —
no LLM, so no token cost on raw logs. Verified sources:

| Host | Path | Format |
|:--|:--|:--|
| Claude Code | `~/.claude/projects/<enc-cwd>/<uuid>.jsonl` | JSONL |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (+ `archived_sessions/`, `history.jsonl`) | JSONL |
| OpenClaw | `~/.openclaw/logs/` (markdown memory under `~/.openclaw/memory/`) | probe at impl |

Per-source watermark (mtime/offset) → a re-run never re-processes a
handled transcript.

**Judge + write (host LLM) → then consolidate + evict.** The host LLM
applies the engine contract verbatim: memory-spec §2/§4 exclusions +
write-time usefulness bar + salience routing (explicit → semantic,
incidental → episodic). Writes go through the daemon; dedup is
exact-content only (binary `88da2ae`); no `supersedes` (CRUD-only,
`bfa1bd5`). Then the same consolidate + evict the Linggen `dream` runs
(§7b — one shared contract).

**Floors**: secrets stripped in the script before the LLM sees them
(memory-spec §3 r6); never store file-derivable content (§4 r1).

## 5. Interface

Primary surface = **chat slash commands**, thin wrappers over the one
daemon: `/shared-memory add | search | dream | delete | update`.
- `add | search | delete | update` — daemon passthrough.
- `dream` — scan + extract + judge/write + consolidate + evict (§4).
  User-invoked: the user is the scheduler on hosts with no mission
  system. **Capture happens here (the scan)** — not via a continuous
  encode hook.

One *optional* hook: `SessionStart` → recall-inject (query the daemon,
token-budgeted) so memory surfaces without the user asking. Installed
by `install.sh`, never hand-edited; per-host wiring verified at impl.

## 6. UI duality

Keep the current web app + workflow. One codebase, host-detected mode:

- **On Linggen** — AI-native app (skill app surface, dashboard /
  `PageUpdate`).
- **On other hosts** — standard skill (CLI / chat, no `PageUpdate`
  canvas) + the daemon-served data browser at `127.0.0.1:9888`.

Restripe today's Linggen-coupled bits (the `implements:` block,
dashboard mode) to **host-detected**, not hardwired.

## 7. Resolved decisions

**(a) No-Linggen consolidation — RESOLVED.** `dream` is one contract,
two triggers: an autonomous mission on Linggen; the manual
`/shared-memory dream` on hosts with no scheduler (the user is the
scheduler). The skill's `dream` runs the full pass — scan → host-LLM
judge → consolidate + evict — so third-party hosts get real promotion
and eviction, not a degraded cache. No autonomous skill scheduler, no
second memory system.

**(b) Reconcile reach — RESOLVED.** The consolidate/evict + Reconcile
contract lives in one shared place — `agents/ling-mem.md` (memory-spec
§2) — reused verbatim by both the Linggen mission and the skill
`dream`. Judgment is whatever LLM hosts the run (engine LLM on Linggen,
host LLM on CC/Codex/OpenClaw); the binary stays mechanical. Single
source ⇒ no drift.

## 8. Ops risks — de-risk before ship (blocking)

- **Concurrent writers — NOT a risk (already handled).** There are no
  independent `ling-mem` processes. One `ling-mem` **daemon** is the
  sole writer; every host (CC, Codex, Linggen, the skill) talks to that
  single server, which serializes writes. The skill **already checks
  for a running `ling-mem` server and uses it** — implemented, not a
  TODO. Nothing to verify here.
- **Version / schema skew.** Two installers (Linggen vs skill
  `install.sh`) updating `ling-mem` independently → schema skew on one
  shared store under the **no-forward-migration** policy → wipe-and-
  fresh data loss. Need a single source of truth for the installed
  binary version, or a version handshake that refuses rather than
  corrupts.

## 9. Implementation pointers

- **Do not duplicate** the contract. Reference `linggen/doc/
  memory-spec.md` (memory rules) and `linggen-memory/doc/tech-spec.md`
  + `DESIGN.md` (CLI/schema).
- **Drift is the sustainability risk**: the skill's durability/salience
  instruction prose must derive from the same spec the engine uses, not
  be hand-restated (a stale skill misrepresents the product — the
  failure mode just cleaned up on ClawHub).
- **Edit order**: standalone `skills/` first → `vendor/skills`
  submodule + `~/.linggen/skills` (never the reverse). Hook/install
  wiring lives in `install.sh` only.

## 10. Sequence

de-risk §8 version-skew only (concurrency already handled — single
daemon) → write the rename distribution plan → implement the skill
`dream` (scan + extract + host-LLM judge + consolidate/evict, contract
from `agents/ling-mem.md`) + the optional recall hook → restripe UI
duality. **Never rename-first.**

## 11. Update plan

Status snapshot 2026-05-20: B/A/C/D/E shipped on `skills/main` (local
working tree, not yet pushed). F deferred to its own distribution plan.

1. **B — Stale store facts** ✅ LANDED.
   - Embedder string: 1024-dim Qwen3-Embedding-0.6B everywhere
     (README.md, SKILL.md, install.sh-written CLAUDE.md).
   - Path: `memory/memory.lancedb/` (semantic + episodic) in README.md.
   - `supersedes` removed from SKILL.md Consolidate section + all
     routing-rules.md tables. Reconcile = append + read-time +
     explicit user delete.
   - Dedup language switched to exact-content-at-write-time (binary
     `insert_with_dedup`); fuzzy moved to `dream` / live agent.

2. **A — Core: markdown → `tier=core`** ✅ LANDED.
   - `identity.md` / `style.md` retired from the two-tier model.
     SKILL.md, routing-rules.md, dashboard.md all switched to
     `ling-mem add … --tier core` / `ling-mem list --tier core`.
   - `permission.warning` frontmatter updated.
   - `install.sh` `seed_core_memory()` no longer touches markdown
     files; `configure_claude_md` block no longer `@`-imports
     identity/style.

3. **C — `dream` flow** ✅ LANDED.
   - SKILL.md Modes: "Scan" mode → "Dream" mode; new slash-command
     table (`add | search | list | delete | update | dream`).
     Consolidate section reworked: automatic in dream, interactive
     destructive edits only with user present.
   - `references/scan-flow.md` deleted; replaced by
     `references/dream-flow.md` (Phases 1–5: scan → script-extract
     → host-LLM judge+write → consolidate+evict → persist+report).
     dashboard.md re-routed to it.
   - `references/extractor-prompt.md` rewritten as a single-source
     pointer to engine `agents/ling-mem.md` ENCODE phase (anti-drift
     marker included).
   - `scripts/collect_sessions.sh` + `extract_session.sh`: added
     Codex + OpenClaw sources, per-source mtime watermark
     (`--watermark <file>`), defence-in-depth secret filter, byte
     cap. Both still syntax-clean.

4. **D — Recall hook** ✅ AUDITED. `install.sh` already wires
   `UserPromptSubmit` → `recall.sh` → `ling-mem search "$prompt"
   --limit 8 --min-score 0.30 --format json` → `head -3`. 3s timeout,
   cwd-aware project filter, silent failure, env-var disable. Hits
   `/api/memory/search` via the daemon. Token-budgeted.

5. **E — Linggen-coupling restripe** ✅ LANDED. SKILL.md body
   opening + dashboard-mode prose now host-detected (`PageUpdate`
   capability gate) rather than naming Linggen / Claude Code
   directly. `implements:` and `permission:` frontmatter blocks
   remain Linggen-only (opt-in by host, harmless to others) — the
   right shape.

6. **F — Distribution migration** ⏳ DEFERRED (own plan, §3).
   `install.sh` existing-install dir migration (`~/.linggen/skills/
   ling-mem` → `shared-memory`), hook-marker idempotency on rename,
   scoped release tags, ClawHub re-publish, `vendor/skills` +
   `~/.linggen/skills` sync. Skill-bundled `assets/mission.md` is now
   dead code (engine ships its own dream mission); removal lives in
   F as well. The `ling-mem` **binary** stays unchanged throughout.

**Ship precondition (still open):** resolve §8 version-skew —
single-source the installed-binary version or refuse-on-mismatch —
before any release; shared-store corruption under no-migration is
unrecoverable. Content workstreams above can land independently of
this; the release that brings them to users is gated on F + §8.
