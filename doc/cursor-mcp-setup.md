# Cursor MCP Setup for Linggen Memory

This guide explains how to configure Cursor to use Linggen Memory's MCP tools for code search and prompt enhancement.

## Quick Setup (Recommended)

### 1. Start Linggen Memory Server

The `ling-mem` server includes the MCP endpoint. No separate server needed!

```bash
# If installed via install script
ling-mem serve

# Or run from source
cd backend
cargo run -p api --release
```

This starts:

- **API server** on `http://localhost:8787/api/*`
- **MCP endpoint** on `http://localhost:8787/mcp/*`
- **Frontend** (if built) on `http://localhost:8787/`

### 2. Global Configuration

Add Linggen Memory to your global Cursor MCP configuration so it's available in all projects.

Create or edit `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "linggen-memory": {
      "url": "http://localhost:8787/mcp/sse"
    }
  }
}
```

### 3. Restart Cursor

After saving the configuration, restart Cursor to load the new MCP server.

### 4. Verify Connection

1. Open Cursor Settings (`Cmd+Shift+J` on Mac, `Ctrl+Shift+J` on Windows/Linux)
2. Go to **Features > Model Context Protocol**
3. You should see "linggen-memory" listed as an available server
4. The tools should appear under "Available Tools" in chat

## Team/LAN Setup

For teams sharing a central Linggen Memory server:

```json
{
  "mcpServers": {
    "linggen-memory": {
      "url": "http://linggen.your-company.internal:8787/mcp/sse"
    }
  }
}
```

Replace `linggen.your-company.internal` with your actual server hostname or IP.

**Note:** Each team member only needs to add this config—no binary installation required!

## Project-Specific Configuration

If you want Linggen Memory only for specific projects, create `.cursor/mcp.json` in the project root:

```json
{
  "mcpServers": {
    "linggen-memory": {
      "url": "http://localhost:8787/mcp/sse"
    }
  }
}
```

This configuration will override global settings for that project.

## Available Tools

Once connected, these tools are available in Cursor chat:

| Tool              | Description                       | Example Usage                            |
| ----------------- | --------------------------------- | ---------------------------------------- |
| `search_codebase` | Search for relevant code snippets | "Search for authentication code"         |
| `enhance_prompt`  | Enhance your prompt with context  | "Enhance: How does the user login work?" |
| `list_sources`    | List indexed codebases            | "What sources are indexed?"              |
| `get_status`      | Check server status               | "What's the server status?"              |

## Access Control (Optional)

For additional security on your LAN, set an access token:

```bash
export LINGGEN_ACCESS_TOKEN="your-secret-token"
ling-mem serve
```

Then add the token to your Cursor config:

```json
{
  "mcpServers": {
    "linggen-memory": {
      "url": "http://linggen.your-company.internal:8787/mcp/sse",
      "headers": {
        "X-Linggen-Token": "your-secret-token"
      }
    }
  }
}
```

## Troubleshooting

### Server not appearing in Cursor

1. Check that the server is running:

   ```bash
   curl http://localhost:8787/mcp/health
   ```

   Should return JSON with status "ok".

2. Verify your `mcp.json` syntax is valid JSON.

3. Restart Cursor completely (not just reload window).

### Tools not working

1. Check that the API is ready:

   ```bash
   curl http://localhost:8787/api/status
   ```

2. Check server logs for errors.

### View MCP Logs

In Cursor:

1. Open the Output panel (`Cmd+Shift+U`)
2. Select "MCP Logs" from the dropdown
3. Look for connection errors or tool call failures

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         Your Machine                            │
│  ┌─────────┐                                                    │
│  │ Cursor  │◄──── SSE ────┐                                     │
│  └─────────┘              │                                     │
│                           ▼                                     │
│                    ┌─────────────────────────────────────────┐  │
│                    │       Linggen Memory Server (ling-mem)  │  │
│                    │  /api/*  - REST API                     │  │
│                    │  /mcp/*  - MCP SSE endpoint             │  │
│                    │  /       - Frontend (if built)          │  │
│                    └─────────────────────────────────────────┘  │
│                              localhost:8787                     │
└─────────────────────────────────────────────────────────────────┘
```

For team setups, the server runs on a shared machine, and each developer's Cursor connects via the network. **No local installation needed!**
