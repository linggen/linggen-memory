---
# memory_dream_status counts DAYS; row counts come from `ling mem stats` (Bash),
# which this run can't reach — a "12 rows" memory line is a misread.
type: regex
target: last_message
pattern: 'memory:\s*12 rows'
match: not_contains
---
