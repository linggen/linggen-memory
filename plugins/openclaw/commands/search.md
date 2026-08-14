---
description: Semantic search across the user's durable memory
argument-hint: "<query>"
---

Invoke the linggen skill (Skill tool) and treat this message as `/linggen search $ARGUMENTS` — Chat mode, single search recipe. Make exactly one search call (`memory_search`, or `ling-mem search "$ARGUMENTS" --limit 10 --format json | jq -c 'del(.vector)'` as fallback) and render the results as a compact list with type, content, and relative timestamp. No speculative filters.
