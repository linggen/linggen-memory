---
description: Linggen status — binary versions + updates, memory store size, upkeep (scan/dream/solve), last dream run
---

Invoke the linggen skill (Skill tool) and treat this message as `/linggen status` — Status mode. Gather, then render ONE glanceable block. Two fetches, run them in parallel:

1. `memory_dream_status` (MCP) — firsts, `total_days`/`scanned_days`/`dreamed_days`, `open_issues`, `last_run`, `last_run_error`. Fallback when MCP is unavailable: `ling-mem days --format json | jq -c 'del(.vector)'`.
2. One Bash call combining, all failure-tolerant (`|| true`, `--max-time 2`):
   - `ling-mem status --format json` — daemon health, version, cached `update` probe
   - `ling-mem stats --format json` — per-tier row counts + `disk_bytes.total`
   - `curl -s --max-time 2 'http://127.0.0.1:9898/api/status?project_root=.'` — engine version
   - `curl -s --max-time 2 http://127.0.0.1:9898/api/bridge/status` — browser bridge
   - engine latest, 24h file cache (NEVER a fresh network hit when the cache is warm):
     `c=~/.linggen/cache/engine-latest.json; mkdir -p ~/.linggen/cache; if [ ! -s "$c" ] || [ $(( $(date +%s) - $(stat -f %m "$c" 2>/dev/null || stat -c %Y "$c") )) -gt 86400 ]; then curl -sL --max-time 5 https://github.com/linggen/linggen/releases/latest/download/manifest.json -o "$c" || true; fi; cat "$c" 2>/dev/null`

Render exactly this shape (omit what a dead source can't fill; `—` for null firsts):

```
linggen: ling-mem <v> · engine <v> · bridge ok (ext <v>)
memory: <total> rows · core <n> · semantic <n> · episodic <n> · <disk_bytes.total as MB> MB
upkeep: <scanned_days>/<total_days> scanned · <dreamed_days>/<total_days> dreamed · first unscanned <date|—> · first undreamed <date|—> · <open_issues> to solve · last dream <status>
next: <verbs>
```

- Update available → `ling-mem 1.3.0 → 1.4.0 available` (from the cached `update` field) and/or `engine 1.4.0 → 1.5.0 available` (running engine version vs cached manifest `version`).
- `next:` names only what's due, comma-separated: `/linggen:scan <first_unscanned>`, `/linggen:dream`, `/linggen:solve`, `ling-mem upgrade --yes` (ling-mem update), `curl -fsSL https://linggen.dev/install.sh | bash` (engine update). Nothing due → `next: all caught up`.
- ling-mem daemon down → first line says `ling-mem down — ling-mem start`; skip memory/upkeep lines. Engine down → `engine —`. Bridge disconnected → `bridge —`.
- If `last_run_error` is set, show it verbatim on its own line.
- Do not list every day; the calendar lives in the Linggen memory app and the ling-mem console.
