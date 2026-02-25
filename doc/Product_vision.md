## Linggen Memory – Short Product Definition

### 1. Product vision

- **Vision**: Become the **persistent memory layer for AI coding tools** – the service that ensures LLMs remember your project's architecture, decisions, and constraints across sessions and tools.
- **Role in the toolchain**: IDE/LLM (Cursor, Claude, Copilot) are where you **write and edit code**. Linggen Memory is where you **index, search, and retrieve project knowledge** so AI tools have the right context at the right time.
- **Form factor**: A **local-first, standalone memory service** (`ling-mem`) with semantic search, RAG, code indexing, and MCP/skill integration for any AI agent.

---

### 2. Target users & core jobs

- **New engineers on a large codebase**
  - "Show me the shape of this system and where to start."
- **Senior developers / maintainers**
  - "Before I change this module, tell me what depends on it and what might break."
- **Architects / tech leads / system designers**
  - "I want a single workspace to design, document, and enforce architecture, and then drive LLMs from that spec."

Linggen Memory should:

- Shorten **onboarding time** by providing instant project context.
- Reduce **risk of unintended side-effects** by surfacing constraints and decisions.
- Provide a **persistent, searchable knowledge base** that connects docs, structure, and code.

---

### 3. Core concept – Persistent memory layer for AI

- Linggen Memory maintains a **searchable knowledge base** of your system:
  - Indexed code with semantic embeddings (LanceDB).
  - File/module dependency graph.
  - Stored memories: decisions, constraints, architectural notes.
- AI tools retrieve context automatically via **MCP server** or **skill scripts**.
- Context is injected at the right moment — when AI touches relevant code.

LLMs remain the code generator; Linggen Memory becomes the **persistent context source** that ensures AI understands your system across sessions.

---

### 4. Current product (v1) – Memory service + code indexing

**Goal**: Give developers and AI tools a fast, persistent way to **index, search, and retrieve project knowledge**.

- **Semantic Search & RAG**

  - Index codebases and documentation into vector embeddings (LanceDB).
  - Hybrid search: semantic (vector) + keyword (BM25) for precision.
  - Code-aware chunking that keeps functions and structures together.

- **Dependency Graph**

  - Tree-sitter-based file dependency graph for supported languages (Rust today; TS/JS, Go, Python next).
  - Graphs are cached per source and can be rebuilt on demand.

- **Interfaces**
  - `ling-mem serve` — HTTP API + MCP server + web dashboard.
  - AI skill scripts — Shell-based integration for Claude Code, Codex, Linggen Agent.
  - VS Code extension — "Chat with your codebase" via the memory service.

This gives AI tools a **persistent, searchable knowledge base** they can query for context.

---

### 5. Next steps (v2) – Enhanced memory & cross-project context

- **Structured memories**

  - Store decisions, constraints, and architectural notes as first-class objects.
  - Auto-surface relevant memories when AI touches related code.

- **Cross-project memory**

  - Maintain separate project memories per workspace.
  - Reuse personal patterns and preferences across projects.

- **Prompt enhancement**
  - Generate **context-enriched prompts** for specific tasks:
    - Include relevant code chunks + stored memories + project constraints.
  - Works with any AI tool via MCP or skill scripts.

---

### 6. Non-goals (for now)

- Linggen Memory is **not** an AI agent — it's a memory service that agents consume.
- Linggen Memory is **not** a general-purpose wiki or note app.
- Linggen Memory does **not** replace the IDE; it **provides context** to IDE+LLM tools so they work better.

---

### 7. Success criteria

- AI tools **always have the right context** when working on indexed projects.
- Teams report that **returning to old projects** feels seamless because memory persists.
- New engineers **onboard faster** because the memory service surfaces project knowledge automatically.
