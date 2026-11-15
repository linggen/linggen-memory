# linggen

> One plugin, one local program, three capabilities: durable cross-session
> **memory**, agent **browser control** in your own Chrome, and structured
> reads of your **logged-in X session** — all served by Linggen (`ling`), at
> one address. Each tool served in exactly one place.

Replaces the `shared-memory` plugin — same memory store, same recall hook,
plus everything else Linggen offers. Never run both.

## Install

```text
/plugin marketplace add linggen/linggen-memory
/plugin install linggen@linggen-memory
```

No git (a fresh Mac)? `curl -fsSL https://linggen.dev/install-plugin.sh | bash`
installs the release bundle into Claude Code and Codex instead.

That's the whole install. **The plugin's session-start hook installs and
starts Linggen** (`ling`, ~100MB — installed in the background on your first
session with the installer this plugin ships, disclosed in the session context;
log at `~/.linggen/engine-install.log`). Memory lives inside Linggen, so it
comes online when Linggen does. Set `LINGGEN_NO_ENGINE_INSTALL=1` to opt out of
the install and do it yourself: `curl -fsSL https://linggen.dev/install.sh | bash`.

Session start connects **two** MCP servers on one address — `memory`
(`http://127.0.0.1:9527/mcp/memory`: Linggen's memory) and `linggen`
(`http://127.0.0.1:9527/mcp`: browser, x, agents) — starts Linggen when it is
installed but not running, and injects your core memory (who you are) into
context, plus — in a project — the directories a memory can be filed under and
the index of standing rules filed for that directory and its parents;
everything else comes back through per-turn recall, scoped to where you work.

**A machine with only the plugin** can use another machine's Linggen and its
memory without installing anything:

```
/linggen:config --linggen 192.168.1.5
```

It asks that machine to connect; click Allow in Linggen there and the token
arrives on its own. That writes `~/.linggen/client.json` — where *this* host
goes looking — probes the address, and mirrors it into Claude Code's
`settings.json` (and Codex's `config.toml`) so the MCP servers follow. Restart
Claude Code afterwards; MCP URLs resolve at startup. Nothing is installed or
started locally: the hooks speak MCP over HTTP, so there is one store, not two.
`/linggen:config --local` goes back. To keep a synced copy that works offline,
run Linggen on this machine too and use `/linggen:config --primary 192.168.1.5`.

## What you get

- **Memory** — `memory_search / add / get / update / delete / list` and the
  upkeep tools over the one store (shared by Claude Code, Codex, OpenClaw,
  Linggen). Auto-recall on every prompt via a `UserPromptSubmit` hook; core
  identity injected at session start.
  Claude Code also recalls memory when delegating to Agent, Task, or `agent_run`.
  Set `LINGGEN_DELEGATE=0` to disable, or `LINGGEN_DELEGATE_REWRITE=0` for context only.
  Delegations are not auto-approved unless `LINGGEN_DELEGATE_DECISION=allow`.
  Codex (0.167+) does the same for `agent_run`, and stamps memory calls with the directory, but its
  rewrite needs `allow`, so that delegation is auto-approved. Its built-in `spawn_agent` is not covered
  (the brief is encrypted). After an update, review the plugin's hooks again with `/hooks`.
  Delete is by-id only; rewrites of user-stated facts require the
  user's explicit direction — memory enforces it.
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

- Linggen (`ling`) on `127.0.0.1:9527` — installed by the session-start hook,
  or another machine's Linggen set with `/linggen:config --linggen`.
- For browser tools: the linggen-browser Chrome extension.

## Uninstall

```text
/plugin uninstall linggen@linggen-memory
```

The memory store under `~/.linggen/memory/` is yours and stays.
