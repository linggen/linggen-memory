---
description: List memory rows (optionally filtered by type or tier)
argument-hint: "[type|tier filter]"
---

Invoke the linggen skill (Skill tool) and treat this message as `/linggen list $ARGUMENTS` — Chat mode, single list recipe. Make exactly one list call (`memory_list`, or the `ling-mem list` CLI fallback piped through `jq -c 'del(.vector)'`): no filters when no arguments are given; add only the one filter the arguments name. Render results as a compact list with type, content (truncate ~80 chars), and relative timestamp.
