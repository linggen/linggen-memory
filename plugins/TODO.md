# Plugin work — parked items

- Switch Linggen engine to consume `ling-mem` via MCP instead of built-in `Memory_*` tools.
- Move dispatch-boundary fixes into ling-mem's MCP layer (host=linggen default, `past_ttl` field stripping, `tier=episodic` → `episodic=true`).
- Decide MCP transport (stdio shim vs SSE on daemon).
- Wire Stop-hook encoder (CC, Codex) to call host CLI for memory extraction.
- Hook prompts shared via `skill/encoder-prompt.md`.
- Submit `linggen` to `claude-plugins-community` once the plugin has real-user mileage.
- ClawHub release of the `linggen` skill via `clawhub skill push plugins/linggen/skills/linggen/` (mcp-spec Phase 3).
- Linggen install path: stop copying the skill into `~/.linggen/skills/` once engine consumes via MCP.
- Replace stale `~/.linggen/skills/shared-memory/` references in `linggen/` (sweep; dir renamed to linggen).
- Update `linggen-vscode` extension to consume memory via the same `/mcp` endpoint once SSE ships.
