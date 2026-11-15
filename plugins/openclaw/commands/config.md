---
description: Point this host at a Linggen — show or set where Linggen lives (this machine, or another machine on your LAN)
---

Run the plugin's config script with the user's arguments, with a 3-minute
timeout (asking another machine waits up to two minutes for a click there):

```bash
bash "${CLAUDE_PLUGIN_ROOT}/scripts/config.sh" $ARGUMENTS
```

Show its output verbatim — the address line, the `Next:` / `All set` line and
the common commands are the guidance; do not reword or summarize them.

The script does the work — read, probe, ask, write, mirror. Do not
re-implement any of it, and do not edit `client.json`, `settings.json` or
Codex's `config.toml` yourself: a config write is mechanical, and a model
improvising a JSON or TOML edit is how a settings file gets mangled.

After a run that changed the address, add one line: **restart this host**
(Claude Code or Codex) — MCP server URLs are resolved at startup.

What its states mean, if the user asks:

- `ok` — reachable and this machine may use it.
- `not approved` — reachable, but that machine has not allowed this one.
  `--linggen <addr>` (or `--primary <addr>`) asks; someone clicks Allow in
  Linggen on that machine, and the token is kept here without anyone copying it.
- `replica of … · offline` — the primary is unreachable; reads work from
  the local copy, and writes queue and replay when it is back.
- `no answer` — nothing listens there: Linggen is not running, that machine
  shares nothing on the LAN, or a firewall blocks the port.

Usage:

- `/linggen:config` — where Linggen is, where memory is (primary or replica),
  and the next step
- `/linggen:config --primary 192.168.1.5` — share another machine's memory:
  this machine's Linggen keeps a synced copy and works offline (click Allow on
  that machine)
- `/linggen:config --primary local` — this machine holds the memory
- `/linggen:config --linggen 192.168.1.5` — this machine runs no Linggen and
  uses that one's directly (no local copy, no offline); nothing is installed
  or started here
- `/linggen:config --local` — back to the Linggen on this machine
