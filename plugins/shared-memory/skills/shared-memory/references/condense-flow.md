# Condense flow — collapse stale chains (canonical runbook)

Stage 4 of the memory pipeline: **semantic-at-rest maintenance**, the
only pass whose input is old long-term rows. Every other merge point
gates entry (write-time dedup, the dream's promotion judgment) or works
a recall window; condense cures what no recall ever touches.

- **Linggen** — the built-in `condense` mission under the `memory`
  agent (ships cron-disabled, monthly once enabled). Trigger from the
  memory app / mission API.
- **Claude Code / Codex / OpenClaw** — no mission runtime; the host
  agent runs the steps below via the `ling-mem` CLI (or the
  `memory_chains` / `memory_add` MCP tools), on demand.

## Before the first run — back up

```bash
ling-mem export ~/condense-backup-$(date +%F).ndjson
```

Condense retires rows (atomically, via `replace_ids`), and the first
runs should be supervised: watch the `MERGE` lines, spot-check a few
survivors, keep the export until you trust the pass.

## The scan — `chains`

```bash
ling-mem chains --derived-only --limit 3                  # cited chains
ling-mem chains --kind marker --derived-only --limit 5    # marker candidates
ling-mem chains --kind subject --derived-only --limit 2   # subject clusters (v2)
```

(MCP: `memory_chains {"kind":"cited","derived_only":true,"limit":3}`.)

Three kinds, one law:

- **`cited`** — rows citing another row's id verbatim, grouped into
  chains. Pre-confirmed: an id citation is proof of reference;
  collapse without re-litigating.
- **`marker`** — rows with provisional-state language ("OPEN:",
  "uncommitted", …) plus nearest-neighbor rows. Guesses: collapse only
  after confirming a neighbor is the same subject AND one row
  completes or obsoletes the other; otherwise skip.
- **`subject`** (v2 digests) — same-subject vector clusters, 3+ rows.
  Parallel notes on one subject, not a newest-wins chain: write one
  focused per-subject **digest** row. Vector neighbors carry boundary
  noise — digest the largest genuinely-one-subject subset
  (`replace_ids` only its ids), leave outliers untouched; never one
  mega state row.

**Always pass `derived_only`** on an unattended or semi-attended pass —
it filters to clusters that are entirely the agent's own notes
(`from=derived`, `tier=semantic`), which the merge law allows merging
without the user. A user-voice cluster is the user's to resolve
(surface it in chat; never auto-merge).

## The collapse — one current-truth row per chain

One atomic write per chain (MCP/HTTP):

```json
memory_add {
  "content": "<current state first; history as a short dated span; keep lessons, drop dead provisional markers>",
  "type": "<most current member's type>",
  "contexts": [<union of members'>],
  "replace_ids": ["<every member id>"]
}
```

CLI hosts (no atomic replace verb): `ling-mem add` the survivor first,
then `ling-mem delete <member-id> --yes` each member — write before
delete.

Drafting rules (same as the memory agent's):

- Lead with the current state; carry history as a dated narrative
  span. Keep re-hit lessons and decision reasoning; drop per-event
  noise and provisional markers that no longer hold.
- Never invent — every claim must come from a member row. On conflict,
  keep the newest claim and note the change.
- **Never cite raw row ids in the new content** — members are being
  deleted; a dangling id re-chains the survivor on the next scan.
- `replace_ids` may list only `from=derived, tier=semantic` rows.
  Never a user-voice row, a core row, or an episodic id — one in the
  cluster means skip the whole cluster.

## Loop shape

Cited chains: re-fetch at offset 0 after each batch — merged chains
vanish from the next scan, so the front of the list is always fresh
work; stop at `total: 0`. Marker candidates and subject clusters:
page by offset; skipped ones linger (next month re-examines them). A
partial pass is fine — oldest-first keeps progress monotone.

## Status lines

Same audit-trail contract as dream: `MERGE <new-id> replaces=<k>
"<gist>"` per collapsed chain, `SKIP <id> unrelated` per rejected
marker candidate, and never print a line for a call you didn't make.

## Order of passes

Cited first (provable), then markers (confirm supersession), then
subject digests (v2) — chains should collapse before the digest pass
sees their subjects. The `subject` scan itself excludes rows still in
cited chains for the same reason.
