---
description: Drain the memory review queue — solve each item from evidence, ask the user only when evidence can't settle it
---

First call `memory_role`: when it says `role: replica`, reply exactly `Memory is dreamed on the primary (<upstream>)` and stop — review items are solved on the primary.

Otherwise invoke the linggen skill (Skill tool) and treat this message as `/linggen solve` — Solve mode. Follow the skill's Solve runbook: list open items (`memory_issues`, or `ling-mem issues --format json`), then per item try to solve it YOURSELF first — gather evidence at solve time (full rows, git history, code, docs) and write what the evidence settles via `memory_add` + `replace_ids` — asking the user only when evidence can't settle it or a user-voice row is involved (then ONE simple fact question at a time, plain words, with your recommendation; user-voice fixes need `user_directed:true` after the ask), and close each item via `memory_issue_resolve`. When the evidence sits where you can't read (a declined permission prompt, or outside your grants), don't ask again or try another tool: skip the item, and at the end ask the user ONE plain question covering all skipped items.
