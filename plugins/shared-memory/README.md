# shared-memory

> Durable cross-session memory for Claude Code. The agent remembers who
> you are — your name, role, preferences, decisions, gotchas — across
> every conversation, every project, every restart.

## Install

```text
/plugin marketplace add linggen/linggen-memory
/plugin install shared-memory@linggen-memory
```

That's it. First session start downloads the `ling-mem` binary
(~7 MB), starts the local daemon on `127.0.0.1:9888`, and the agent
gains five memory tools (`memory_search`, `memory_add`, `memory_list`,
`memory_get`, `memory_delete`) plus a session-start primer teaching it
when and how to use them.

## What it does

- **Auto-recall on every turn.** Before answering, the agent searches
  past memory for relevant facts and chips them into its reply
  (*"From memory: you prefer X…"*).
- **Auto-save when you state HIGH-SIGNAL info.** *"My cat is Mochi"*,
  *"I live in Shanghai"*, *"always use TypeScript"* — saved silently,
  retrieved months later in any project.
- **Three tiers.**
  - **core** — narrow universals (name, role, location, family) injected
    into every session's system prompt.
  - **semantic** — long-term facts, preferences, decisions, gotchas
    retrieved on demand.
  - **episodic** — recent observations staged for promotion by the
    `/shared-memory dream` consolidation pass.
- **Browser dashboard** — open `http://127.0.0.1:9888` to view, edit,
  and bulk-delete rows.
- **Cross-host.** Same store works with Codex CLI, OpenClaw, and the
  Linggen native agent. One memory, every tool.

## What data leaves the machine

**Memory content: never.** Everything lives at `~/.linggen/memory/` in
a local LanceDB store. No content, no embeddings, no queries are sent
anywhere off-host.

**Anonymous usage pings:** the `ling-mem` binary sends an install ping
and a daily-active ping (no content, no identity, just "someone ran
ling-mem today"). Disable any time:

```bash
touch ~/.linggen/no-telemetry
```

## Uninstall

```text
/plugin uninstall shared-memory@linggen-memory
```

This removes the plugin and stops the daemon. To also delete the
memory store and binary:

```bash
rm -rf ~/.linggen
rm -f  ~/.local/bin/ling-mem
```

## Requirements

- macOS (Apple Silicon or Intel) or Linux (x86_64 or aarch64)
- `jq` and `curl` available on `PATH`
- ~7 MB disk for the binary, ~100 MB+ for a long-running memory store

## Troubleshooting

**No memories surface in recall.**
Daemon may not be running. Run `ling-mem status` — if it's down,
`ling-mem start` brings it up. The next prompt will see fresh recall.

**`ling-mem: command not found` after install.**
The plugin symlinks the binary into `~/.local/bin/`. If that directory
isn't on your shell's PATH, add `export PATH="$HOME/.local/bin:$PATH"`
to your `.zshrc` / `.bashrc`.

**Agent doesn't auto-save my preferences.**
Save explicitly: *"remember: I prefer concise responses"*. The agent
replies *"Saved."* and the row lands in `tier=core`.

**See what's stored.**
Open `http://127.0.0.1:9888` in any browser, or run
`ling-mem list --format json | jq -c 'del(.vector)'`.

## License

Apache-2.0. Source: <https://github.com/linggen/linggen-memory>.

Part of the [Linggen](https://linggen.dev) personal-agent platform.
