---
description: Drain the memory review queue — verify each item against the world, fix with the user, close it
---

Invoke the linggen skill (Skill tool) and treat this message as `/linggen solve` — Solve mode. Follow the skill's Solve runbook: list open items (`memory_issues`, or `ling-mem issues --format json`), then per item gather evidence at solve time (git history / files for stale status claims), apply the confidence rule — solve derived-row items you can prove directly, ask the user ONE item at a time when their call is needed — write fixes via `memory_add` + `replace_ids` (user-voice rows also need `user_directed:true` after the ask), and close each item via `memory_issue_resolve`.
