# Linggen for OpenClaw

Durable cross-host memory that recalls itself. The same store your Claude Code and
Codex sessions write to — Linggen's memory, served by one local program (`ling`,
the Linggen engine), every host reading and writing the same memory.

```bash
openclaw plugins install clawhub:@linggen/linggen
```

Restart the gateway and it is on. First activation adds the two MCP servers to
your `openclaw.json` — `memory` (`http://127.0.0.1:9527/mcp/memory`) and
`linggen` (`http://127.0.0.1:9527/mcp`), one address; Linggen itself installs on
first use, **after the agent asks you** — the installer ships inside this plugin
and refuses remote-script fallbacks, so no remotely fetched script is ever
executed. The release binary is SHA-256 verified.

## What it can touch

Full disclosure up front — this is a capable bundle, and you should know its
reach before installing:

- **Durable memory** in `~/.linggen` that outlives every session, with
  relevant rows injected into future prompts sent to your configured model.
- **One local program**: the Linggen engine (`ling`, ~100MB, SHA-256-verified),
  installed to `/usr/local/bin` (or `~/.local/bin`) on your explicit yes.
- **Browser and X tools** (via the engine): agent control of your own Chrome
  with per-site permission prompts, and reads of your logged-in X session.
- **Session-log backfill** (`/linggen-scan`, user-triggered only): reads local
  agent transcripts to stage durable memories; secret-filtered, never
  automatic.
- **LAN reach needs approval**: Linggen answers another machine's memory calls
  only once that machine is approved with Allow on this one (`/linggen-config`
  asks); a phone's token never reaches memory over the network.

Nothing phones home: telemetry is anonymous counts with strict no-content
rules ([privacy](https://linggen.dev/privacy)), and `LINGGEN_NO_TELEMETRY=1`
turns it off.

## What it does

| | |
|---|---|
| **Core identity** | Who you are — name, role, timezone — added to the *system* prompt once per session, so providers cache it instead of paying for it every turn. |
| **Per-turn recall** | The most relevant memories for what you just asked, scoped to the project you are working in. |
| **Project scoping** | Every `memory_add` records where it came from and every `memory_search` asks what is in scope — stamped by the host, which knows, rather than the model, which would guess. |
| **Recall on delegate** | When the agent hands work to a sub-agent (`sessions_spawn`) or the engine (`agent_run`), the memory that matches the brief is appended to it and shown to the parent. |
| **Memory tools** | `memory_search`, `memory_add` and the rest over MCP from Linggen's memory (`/mcp/memory`); browser control, X reads and `agent_run` from the Linggen engine (`/mcp`). |
| **Commands** | `/linggen-search`, `/linggen-status`, `/linggen-dream`, `/linggen-solve`, `/linggen-scan`, `/linggen-list`, `/linggen-config`. |

Recall on delegate: `LINGGEN_DELEGATE=0` turns it off, `LINGGEN_DELEGATE_REWRITE=0` tells the parent without touching the brief, `LINGGEN_DELEGATE_LIMIT` sets the rows (default 3). Telling the parent the same turn needs OpenClaw 2026.11+ and `hooks.allowPromptInjection` not `false`; the brief itself is rewritten on 2026.5.6 too.

## Pointing this host at another machine

Memory can live on one machine and be read from several. On a machine with only
the plugin, `/linggen-config --linggen 192.168.1.9` asks that machine (click
Allow in Linggen there) and writes `~/.linggen/client.json`:

```json
{ "ling": "http://192.168.1.9:9527", "token": "<that machine's token>" }
```

A host pointed off-machine installs nothing and starts nothing — a local Linggen
would open a second store and fork your memory in two.

## Manual setup

Only needed if you removed the servers or moved Linggen:

```bash
openclaw linggen setup
openclaw mcp list
```

## Development

```bash
npm run sync    # refresh skills/ and commands/ from plugins/linggen
npm run build   # syntax check
npm test        # unit tests, no dependencies to install
openclaw plugins install -l .
```

Then restart the gateway **twice** on a first install: the plugin loads on the
first restart, and its skill is symlinked into `~/.openclaw/plugin-skills/` on
the second, once the plugin registry has been refreshed.

Publishing always passes the family explicitly, because a `skills/` or
`commands/` folder beside the manifest looks like a Claude bundle layout to
ClawHub's pre-scan:

```bash
clawhub package publish . --family code-plugin --dry-run
clawhub package publish . --family code-plugin
```

Two constraints worth knowing before changing the source: OpenClaw's installer
**blocks** any plugin that spawns processes, or that reads `process.env` within
eight lines of a network call. That is why installation is the skill's job here
and why `config.mjs` (which reads) is separate from `rpc.mjs` (which sends).

MIT. Part of [Linggen](https://linggen.dev).
