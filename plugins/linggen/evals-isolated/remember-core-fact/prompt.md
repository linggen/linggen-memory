---
description: A fact about who the user is (a pet) lands in core, search-first, in the user's voice.
tags: [capture]
max_turns: 12
allowed_tools: [Skill]
append_system_prompt: |
  # MCP Server Instructions

  ## plugin:linggen:memory
  ling-mem is this user's memory, shared by Claude Code, Codex, OpenClaw and Linggen. Save what makes a future session predict the user better — the person, not the task.

  # Tiers
  - core — who they are, always loaded: name, role/job, location, timezone, languages, family/pets. One short fact per row.
  - semantic — durable, recalled on demand: goals, preferences ("always X / never Y" → type=preference, from=user; never core), decisions with their why, gotchas.
  - episodic — the default capture (omit tier): events, milestones, run learnings. No search first; the nightly dream promotes what lasts.

  # Saving
  - Before a core/semantic add, memory_search the subject. Same subject or claim in other words = conflict; when unsure, treat it as one.
  - Your own rows (from=derived): merge into one current-truth row via memory_add + replace_ids, never add then delete.
  - The user's rows (from=user): ask first, showing each row's full text and date; then replace_ids + user_directed:true. Never from your own inference.
  - What the user said is from=user. A replacement keeps the loser's tier and scope. A new status (shipped/fixed/dropped) replaces the old status row.
  - "remember / forget / update X", in any language: search, act, user_directed:true.
  - Anchor relative time to dates ("last month" → "2026-08"). occurred_at dates days and TTL; it never ranks.
  - A row's scope is the directory it is about (default: the host's cwd). Pass scope (one of "Memory scopes here") when it isn't where you stand; global:true when it is about the person.
  - A standing rule the user states ("always…", "以后都…") gets indexed:true and a summary (one line ≤ 80 chars) — the index loads it at session start under its scope.
  - Never save secrets or file bodies you can re-read. Project internals stay episodic.

  # Recall
  Search when the question may touch past preferences, decisions or gotchas. Show each fact you use: "From memory (3 months ago): …". Never pass type, from or outcome to memory_search unless asked.

  Full rules and examples: the memory skill.
---

Remember this about me: I have a dog called Mochi, a three-year-old shiba inu.
