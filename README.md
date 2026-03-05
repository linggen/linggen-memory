<p align="center">
  <img src="frontend/public/logo.svg" width="120" alt="Linggen Logo">
</p>

# Linggen Memory

**Standalone semantic memory and RAG service for AI coding assistants.**

Linggen Memory (`ling-mem`) is a local-first memory engine that provides semantic search, code indexing, vector storage (via LanceDB), and an MCP server. It works as a standalone service for any AI agent — Claude Code, Codex, Cursor, or your own tools.

[Website](https://linggen.dev) | [Documentation](https://linggen.dev/docs)

---

## What It Does

- **Semantic Code Search:** Index your codebase and search by meaning, not just keywords.
- **Design Memory:** Store architectural decisions, ADRs, and tribal knowledge in `.linggen/memory/` as Markdown. AI retrieves them via semantic search.
- **System Map:** Obsidian-like dependency graph visualization of file relationships.
- **Shared Library & Skills:** Pre-defined skills (Software Architect, Senior Developer, etc.) for consistent AI behavior.
- **MCP Server:** Model Context Protocol endpoint at `/mcp/sse` for MCP-enabled IDEs.
- **Local-First:** All indexing and vector search happens on your machine. Nothing leaves your side.

---

## Quick Start

### Install

```bash
curl -fsSL https://linggen.dev/install-mem.sh | bash
```

Or install a specific version:

```bash
curl -fsSL https://linggen.dev/install-mem.sh | bash -s -- --version v0.7.0
```

### Upgrade

```bash
ling-mem update
```

### Run

```bash
# Start the server (default port 8787)
ling-mem serve

# Start as a background daemon
ling-mem serve --daemon

# Index a codebase
ling-mem index .

# Index with options
ling-mem index /path/to/project --mode full --name my-project

# Check status
ling-mem status

# Stop daemon
ling-mem stop

# Self-update to latest
ling-mem update
```

### Web UI

Open `http://localhost:8787` in your browser for the built-in Web UI with graph visualization, search, and memory management.

---

## Use With Your AI

Linggen Memory works as a standalone service that any AI can connect to.

### As a Skill (Claude Code / Codex)

Install the memory skill:

```bash
# The linggen skill connects your AI to the memory server
ling init --global
```

Example prompts:

> "Search Linggen memory for architectural decisions about our database choice."

> "Index this project with Linggen and find all authentication-related code."

### As an MCP Server (Cursor / Zed / Claude Code)

Add to your MCP config:

```json
{
  "mcpServers": {
    "linggen-memory": {
      "url": "http://localhost:8787/mcp/sse"
    }
  }
}
```

### As an HTTP API

All functionality is available via REST:

```bash
# Search
curl "http://localhost:8787/api/search?q=authentication&limit=5"

# List indexed sources
curl "http://localhost:8787/api/resources"

# Server status
curl "http://localhost:8787/api/status"
```

---

## Integration with Linggen Agent

If you use [Linggen Agent](https://github.com/linggen/linggen) (`ling`), it manages `ling-mem` for you:

```bash
ling memory start    # starts ling-mem daemon
ling memory stop     # stops it
ling memory status   # checks status
ling memory index .  # indexes via ling-mem
```

You can also install `ling-mem` via:

```bash
ling install --memory
```

---

## The Linggen Ecosystem

| Project | Description |
|---|---|
| [linggen-memory](https://github.com/linggen/linggen-memory) | Semantic memory engine, RAG backend, MCP server (this repo) |
| [linggen](https://github.com/linggen/linggen) | Multi-agent coding assistant with TUI and Web UI |
| [linggen-vscode](https://github.com/linggen/linggen-vscode) | VS Code extension for graph view and MCP setup |

---

## Architecture

Linggen Memory is a Rust workspace with 11 crates:

| Crate | Purpose |
|---|---|
| `api` | HTTP API server, MCP endpoint, embedded Web UI (`ling-mem` binary) |
| `core` | Core domain types |
| `ingestion` | File/codebase indexing pipeline |
| `embeddings` | Local embedding model management |
| `storage` | LanceDB vector storage |
| `context` | Context assembly for prompts |
| `enhancement` | Prompt enhancement |
| `architect` | Architecture analysis |
| `intent` | Intent detection |
| `llm` | LLM provider abstraction |
| `mcp-server` | Legacy stdio MCP server (deprecated) |

Frontend: React 19 + TypeScript + Vite + Tailwind v4 with CodeMirror, Cytoscape graph, and Mermaid diagrams.

---

## Building From Source

```bash
# Build frontend
cd frontend && npm ci && npm run build && cd ..

# Build backend (embeds frontend via rust-embed)
cd backend && cargo build --release --bin ling-mem
```

The binary is at `backend/target/release/ling-mem`.

### Release Build

```bash
# Build for current platform
./scripts/build.sh v0.7.0 --skip-linux

# Full release (build + sign + upload to GitHub)
./scripts/release.sh v0.7.0
```

See [RELEASES.md](RELEASES.md) for the full release process.

---

## License

Linggen is open-source under the [MIT License](LICENSE).

- **Free for individuals:** All personal and open-source use.
- **Commercial support:** For teams (5+ users), see our [Pricing Page](https://linggen.dev/pricing).

MIT (c) 2026 [Linggen](https://linggen.dev)
