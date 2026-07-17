---
description: Memory status — first unscanned day, first undreamed day, review-queue size, last dream run
---

Invoke the linggen skill (Skill tool) and treat this message as `/linggen status` — Status mode. One call: `memory_dream_status` (or `ling-mem days --format json | jq -c 'del(.vector)'` when MCP is unavailable). Render ONE glanceable line from its fields: `first unscanned <date|—> · first undreamed <date|—> · <N> to solve · last dream <status>`, then one short follow-up line naming the next verb if anything is due (`/linggen:scan <date>`, `/linggen:dream`, `/linggen:solve`) — nothing due, say all caught up. If `last_run_error` is set, show it verbatim. Do not list every day; the calendar lives in the Linggen memory app and the ling-mem console.
