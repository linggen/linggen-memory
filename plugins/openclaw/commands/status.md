---
description: Linggen status — version + updates, memory size, upkeep (scan/dream/solve), last dream run
---

Invoke the linggen skill (Skill tool) and treat this message as `/linggen status` — Status mode. Gather, then render ONE glanceable block. Two fetches, run them in parallel:

1. `memory_dream_status` (MCP, `linggen` server) — firsts, `total_days`/`scanned_days`/`dreamed_days`, `open_issues`, `last_run`, `last_run_error`. Fallback when that server is unavailable: `memory_days` (MCP, `memory` server).
2. One Bash call combining, all failure-tolerant (`|| true`, `--max-time 2`). Where Linggen lives (the address `/linggen:config` prints on its `linggen` line; by hand: `A="$(jq -r '.ling // empty' ~/.linggen/client.json 2>/dev/null)"; A="${A:-http://127.0.0.1:9527}"` — an old file that also names `ling_mem` on another machine means that machine, port 9527).
   - `curl -s --max-time 2 "$A/api/health"` — Linggen up, its `version`
   - only when `$A` is this machine (127.0.0.1 / localhost): `ling mem stats --format json` — per-tier row counts + `disk_bytes.total`, and `curl -s --max-time 2 "$A/api/bridge/status"` — is the browser extension connected (`connected`)
   - Linggen latest, 24h file cache (NEVER a fresh network hit when the cache is warm):
     `c=~/.linggen/cache/engine-latest.json; mkdir -p ~/.linggen/cache; if [ ! -s "$c" ] || [ $(( $(date +%s) - $(stat -f %m "$c" 2>/dev/null || stat -c %Y "$c") )) -gt 86400 ]; then curl -sL --max-time 5 https://github.com/linggen/linggen/releases/latest/download/manifest.json -o "$c" || true; fi; cat "$c" 2>/dev/null`

Render exactly this shape (omit what a dead source can't fill; `—` for null firsts):

```
linggen: <v> on <this machine|host> · browser ok
memory: <total> rows · core <n> · semantic <n> · episodic <n> · <disk_bytes.total as MB> MB
upkeep: <scanned_days>/<total_days> scanned · <dreamed_days>/<total_days> dreamed · first unscanned <date|—> · first undreamed <date|—> · <open_issues> to solve · last dream <status>
next: <verbs>
```

- Update available (this machine only) → `linggen 2.3.0 → 2.4.0 available` (running version vs cached manifest `version`).
- `next:` names only what's due, comma-separated: `/linggen:scan <first_unscanned>`, `/linggen:dream`, `/linggen:solve`, `ling update` (Linggen update — ask before running it). Nothing due → `next: all caught up`.
- Linggen down → first line says `linggen down — ling --web (or open Linggen.app)` on this machine, `linggen on <host> not answering — /linggen:config` for another machine; skip memory/upkeep lines. Memory on another machine → omit the `memory:` line (its size is that machine's to show). Browser extension not connected → `browser —`.
- If `last_run_error` is set, show it verbatim on its own line.
- Do not list every day; the calendar lives in Linggen's memory app.
