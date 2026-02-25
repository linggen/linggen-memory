Hi HN,

Working with multiple projects, I got tired of re-explaining our complex multi-node system to LLMs. Documentation helped, but plain text is hard to search without indexing and doesn't work across projects. I built Linggen to solve this.

**My Workflow:**
I use the Linggen VS Code extension to "init my day." It calls the Linggen MCP to load memory instantly. **Linggen indexes all my docs like it’s remembering them—it is awesome.** One click loads the full architectural context, removing the "cold start" problem.

**The Tech:**

- **Local-First:** Rust + LanceDB. Code and embeddings stay on your machine. No accounts required.
- **Team Memory:** Index knowledge so teammates' LLMs get context automatically.
- **Visual Map:** See file dependencies and refactor "blast radius."
- **MCP-Native:** Supports Cursor, Zed, and Claude Desktop.

Linggen saves me hours. I’d love to hear how you manage complex system context!

Repo: https://github.com/linggen/linggen-memory
Website: https://linggen.dev
