---
description: Backfill-stage one day's session logs into episodic memory
argument-hint: "<YYYY-MM-DD>"
---

Invoke the linggen skill (Skill tool) and treat this message as `/linggen scan $ARGUMENTS` — Scan mode. Follow the skill's Scan procedure (`references/dream-flow.md`, Scan section): run `scripts/scan.sh` for the given date, skip sessions already contributing to that day's rows, encode the remaining keepers into episodic, then stamp with `ling-mem harvest-day`. A date argument is required.
