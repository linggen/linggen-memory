# Linggen Memory Features

## 1. Universal Ingestion Engine
- [ ] **Git Integration**: Index full repositories, tracking commits and branches.
- [x] **Local Filesystem Watcher**: Real-time indexing of local folders (Obsidian vaults, project docs).
- [ ] **Web Crawler**: Index documentation sites (e.g., Rust docs, React docs) for offline RAG.

## 2. The "Brain" (Storage & Retrieval)
- **Hybrid Search**: Combine semantic search (vector) with keyword search (BM25) for precision.
- **Code Understanding**: Specialized chunking for code files (keeping functions together).
- **Long-term Memory**: Store TBs of history on disk using LanceDB without RAM bloat.

## 3. Interfaces
- **Web Dashboard**: Manage sources, view stats, manual query (served by `ling-mem serve`).
- **MCP Server**: Standard MCP protocol for Cursor, Zed, Claude Desktop integration.
- **AI Skill**: Shell scripts + SKILL.md for Claude Code, Codex, Linggen Agent.
- **REST API**: HTTP endpoints for programmatic access.
- **IDE Bridge**: VS Code extension to "Chat with your codebase".

## 4. Privacy & Performance
- **100% Local**: No data leaves the machine.
- **BYO-Model**: Support local models (Llama3, Bert) or API-based (OpenAI/Claude) if user chooses.
