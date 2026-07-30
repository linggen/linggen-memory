---
description: Point this host at a Linggen — show or set where ling and ling-mem live (loopback, or another machine on your LAN)
---

Run the plugin's config script with the user's arguments and show its output verbatim:

```bash
bash "${CLAUDE_PLUGIN_ROOT}/scripts/config.sh" $ARGUMENTS
```

The script does the work — read, probe, write, report. Do not re-implement any
of it, and do not edit `client.json` or `settings.json` yourself: a config write
is mechanical, and a model improvising a JSON edit is how a settings file gets
mangled.

Then, in one or two lines:

- If addresses changed, say **restart Claude Code** — MCP server URLs are
  resolved at startup, so this session still talks to the old ones.
- If a probe says `refused — needs a paired device token`, the daemon is
  reachable but this machine isn't paired with it. Pair through Linggen on the
  machine that holds the store (its screen shows a code), then re-run with
  `--token <token>`.
- If a probe says `no answer`, nothing is listening there — on the local
  machine, the daemon may simply be down; on another machine, `ling-mem` must
  have been started with `--host` to accept anything but loopback.

Usage the user may not know:

- `/linggen:config` — show what is configured and probe it
- `/linggen:config --ling-mem 192.168.1.5:9528 --token <t>` — memory on another machine
- `/linggen:config --local` — back to this machine
