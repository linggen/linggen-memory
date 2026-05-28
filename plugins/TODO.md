# Plugin work — parked items

- Switch Linggen engine to consume `ling-mem` via MCP instead of built-in `Memory_*` tools.
- Move dispatch-boundary fixes into ling-mem's MCP layer (host=linggen default, `past_ttl` field stripping, `tier=episodic` → `episodic=true`).
- Decide MCP transport (stdio shim vs SSE on daemon).
- Wire Stop-hook encoder (CC, Codex) to call host CLI for memory extraction.
- Hook prompts shared via `skill/encoder-prompt.md`.
- Submit `shared-memory` to `claude-plugins-community` after MCP shim is e2e validated.
- ClawHub release of the raw skill via `clawhub skill push plugins/shared-memory/skills/shared-memory/`.
- Linggen install path: stop copying `shared-memory` into `~/.linggen/skills/` once engine consumes via MCP.
- Replace `~/.linggen/skills/shared-memory/` references in `linggen/` after move from `skills/shared-memory/` (sweep).
- Update `linggen-vscode` extension to consume memory via the same `/mcp` endpoint once SSE ships.
