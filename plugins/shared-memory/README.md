# shared-memory

Cross-host durable memory plugin for Claude Code and Codex CLI, powered by the
`ling-mem` daemon. One memory store, three tiers (core + long-term semantic +
episodic staging), shared with Linggen and OpenClaw.

Single-source tree — both hosts load it directly:

- **Claude Code** reads `.claude-plugin/plugin.json` and auto-discovers
  `hooks/hooks.json`, `skills/`, and `.mcp.json`.
- **Codex** reads `plugin.json` at the root, pointing to
  `hooks/codex.hooks.json`, `skills/`, and `.mcp.json`.

Hook scripts and the skill bundle (`skills/shared-memory/`) are shared.

## Install

**Claude Code:**

```text
/plugin marketplace add linggen/linggen-memory
/plugin install shared-memory@linggen-memory
```

The plugin's `SessionStart` hook bootstraps the `ling-mem` binary into
`${CLAUDE_PLUGIN_DATA}/bin/ling-mem` on first session and symlinks it to
`~/.local/bin/ling-mem` so the agent's Bash subshells find it on PATH.
The version is pinned by the `VERSION` file at the plugin root.

**Codex:** equivalent commands once Codex marketplace lands. Until then,
copy the plugin tree to a Codex plugin directory and ensure
`scripts/install-bin.sh` runs at session start via `hooks/codex.hooks.json`.

**Standalone (OpenClaw / Linggen native / no plugin host):**

```bash
curl -fsSL https://linggen.dev/install-ling-mem.sh | bash
```

This wraps `scripts/install-bin.sh` — binary download + SHA-256 verify only,
no host wiring.

## Release

Bump version:

```bash
./scripts/build-plugin.sh   # stamps Cargo.toml version into both manifests + VERSION
```

`scripts/install-bin.sh` reads the pinned release tag from `--version` and
falls back to the bundled `VERSION` when called from the SessionStart hook.

## ClawHub / standalone skill

The bare skill bundle for ClawHub publish lives at `skills/shared-memory/`:

```bash
clawhub skill push plugins/shared-memory/skills/shared-memory/
```
