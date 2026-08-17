# Linggen for OpenClaw

Durable cross-host memory that recalls itself. The same store your Claude Code and
Codex sessions write to — one `ling-mem` daemon, one LanceDB store, every host
reading and writing the same memory.

```bash
openclaw plugins install clawhub:@linggen/linggen
```

Restart the gateway and it is on. First activation installs the `ling-mem` binary
(range-pinned to 1.x, SHA-256 verified, mirrored for regions where GitHub is
blocked), starts the daemons, and adds two MCP servers to your `openclaw.json`.
Nothing to configure.

## What it does

| | |
|---|---|
| **Core identity** | Who you are — name, role, timezone, standing preferences — added to the *system* prompt once per session, so providers cache it instead of paying for it every turn. |
| **Per-turn recall** | The most relevant memories for what you just asked, scoped to the project you are working in. |
| **Project scoping** | Every `memory_add` records where it came from and every `memory_search` asks what is in scope — stamped by the host, which knows, rather than the model, which would guess. |
| **Memory tools** | `memory_search`, `memory_add` and the rest over MCP from the local `ling-mem` daemon; browser control, X reads and `agent_run` from the Linggen engine. |
| **Commands** | `/linggen-search`, `/linggen-status`, `/linggen-dream`, `/linggen-solve`, `/linggen-scan`, `/linggen-list`, `/linggen-config`. |

## Pointing this host at another machine

Memory can live on one machine and be read from several. Write
`~/.linggen/client.json` on the client:

```json
{ "ling": "http://192.168.1.9:9527", "ling_mem": "http://192.168.1.9:9528", "token": "<paired device token>" }
```

A host pointed off-machine installs nothing and starts no daemon — a second local
daemon would answer from a second store and fork your memory in two.

## Manual setup

Only needed if you removed the servers or moved a daemon:

```bash
openclaw linggen setup
openclaw mcp list
```

## Development

```bash
npm run sync    # refresh skills/, commands/ and install-bin.sh from plugins/linggen
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
