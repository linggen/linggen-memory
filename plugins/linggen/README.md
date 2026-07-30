# linggen

> One plugin, one local MCP server, three capabilities: durable
> cross-session **memory**, agent **browser control** in your own
> Chrome, and structured reads of your **logged-in X session**.

Replaces the `shared-memory` plugin — same memory store, same recall
hook, plus everything else the Linggen daemon offers. Never run both.

## Install

```text
/plugin marketplace add linggen/linggen-memory
/plugin install linggen@linggen-memory
```

That's the whole install. **The plugin's session-start hook installs
its two required binaries automatically**: `ling-mem` (memory daemon,
to `~/.local/bin`) and the **Linggen engine** (`ling`, ~100MB —
downloaded in the background on your first session, disclosed in the
session context; log at `~/.linggen/engine-install.log`). Set
`LINGGEN_NO_ENGINE_INSTALL=1` to opt out of the engine download and
install it yourself: `curl -fsSL https://linggen.dev/install.sh | bash`.

Session start connects **two** MCP servers — `linggen`
(`http://127.0.0.1:9527/mcp`: browser, x, agents) and `ling-mem`
(`http://127.0.0.1:9528/mcp`: memory) — boots both daemons, and injects
your core memory (identity, standing preferences) into context.

Each tool is served in exactly one place. Memory comes from the memory
daemon, so a machine that only wants memory needs only that one; the
engine no longer proxies `memory_*`.

**A second machine on your LAN** can share the same memory without installing
anything:

```
/linggen:config --ling-mem 192.168.1.5:9528 --token <device-token>
```

That writes `~/.linggen/client.json` — where *this* host goes looking — probes
the address, and mirrors it into Claude Code's `settings.json` so the MCP
servers follow. Restart Claude Code afterwards; MCP URLs resolve at startup.
Nothing is installed locally: the hooks speak MCP over HTTP and the tools come
from the daemon, so one store, not two. `/linggen:config --local` goes back.

The token comes from pairing that machine through Linggen on the Mac that holds
the store — the same screen-confirm flow the phone uses. Loopback needs none.

## What you get

- **Memory** — `memory_search / add / get / update / delete / list`
  over the shared ling-mem store (same store across Claude Code, Codex,
  OpenClaw, Linggen). Auto-recall on every prompt via a
  `UserPromptSubmit` hook; core identity injected at session start.
  Delete is by-id only; rewrites of user-stated facts require the
  user's explicit direction — the daemon enforces it.
- **Browser control** — `browser_navigate / read_page / click / type /
  key / scroll / screenshot / wait / tabs / read_console` drive one
  visible tab in your own Chrome via the
  [linggen-browser](https://github.com/linggen/linggen-browser)
  extension. First action on a new site asks you in the browser;
  payment / credentials / deletes / posting always confirm.
- **X session reads** — `x_search / x_targets / x_following /
  x_whotofollow / x_own` return structured JSON from your logged-in
  x.com session. No API keys.

## Requirements

- The Linggen daemon (`install.sh` above) on `127.0.0.1:9527`.
- For browser tools: the linggen-browser Chrome extension.
- `ling-mem` installs itself on first use (~7 MB).

## Uninstall

```text
/plugin uninstall linggen@linggen-memory
```

The memory store under `~/.linggen/memory/` is yours and stays.
