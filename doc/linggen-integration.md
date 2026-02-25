# Linggen Memory Integration

This doc summarizes how Linggen Memory integrates with a repo as an AI skill and how install paths are resolved.

## Integration Methods

Linggen Memory integrates with AI coding tools in two ways:

1. **As an AI Skill** — Shell scripts + SKILL.md that any agent (Claude Code, Codex, Linggen Agent) can use
2. **As an MCP Server** — Standard MCP protocol for tools like Cursor, Zed, and Claude Desktop

## Skill Installation

### Via Linggen Agent CLI

```bash
# Initialize Linggen Agent in a repo (sets up skills including memory)
ling init

# Or add the memory skill specifically
ling skills add memory
```

### Manual Installation

Copy the `skills/memory/` directory into your project:

```bash
cp -r /path/to/skills/memory .claude/skills/memory
```

### Install Path Resolution

Both `ling init` and `ling skills add` use the same root resolution logic:

1) Walk upward from the current working directory to find a `.git` folder.
   - If found, that directory is the repo root.
   - If `.claude/` does not exist at the repo root, it is created.
2) If no `.git` is found, walk upward to find a parent `.claude` folder.
   - If found, use that folder's parent as the root.
3) If neither is found:
   - `ling init` falls back to global install paths.
   - `ling skills add` falls back to `~/.claude/skills/<skill>`.

### Flags

- `ling init --local`: uses the current working directory directly.
- `ling init --global`: uses global install paths (home/CODEX_HOME), no repo lookup.

## Integration Folders and Files

### Skills directories

- `.claude/skills/memory/`
  - Local repo install for Claude/Cursor-style skills.
  - Contains SKILL.md and scripts/ for memory operations.
  - Created at repo root on demand.

- `.codex/skills/memory/`
  - Local repo install for Codex skills.
  - If global and `CODEX_HOME` is set, uses `$CODEX_HOME/skills`.
  - Otherwise uses `~/.codex/skills`.

### Repo entrypoints

When `ling init` runs in a repo (non-global), it bootstraps these files:

- `CLAUDE.md`
  - Ensures it includes a pointer to `.claude/skills/memory/SKILL.md`.

- `AGENTS.md`
  - Mirrors the contents of `CLAUDE.md`.

- `.cursor/rules/linggen-memory.md`
  - Written from `.claude/skills/memory/SKILL.md` if it exists.

### Linggen project knowledge

- `.linggen/`
  - Project-local knowledge store (memory/policy/skills).
  - Not created by the CLI here, but treated as a source of truth by Linggen tooling.

## MCP Integration

For MCP-based tools (Cursor, Zed, Claude Desktop), see [cursor-mcp-setup.md](cursor-mcp-setup.md).

The MCP endpoint is served by `ling-mem serve` at `/mcp/sse` (default port 8787).
