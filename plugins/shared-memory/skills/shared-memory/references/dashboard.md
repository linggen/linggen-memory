# Dashboard mode — JS-driven now

The memory skill's web page (`scripts/memory.html`) renders its
dashboard from JS **without an agent round-trip**. On open,
`memory-app.js` does in parallel:

1. `POST /api/memory/count` three times (`tier=core`, `tier=semantic`,
   `episodic=true`) against the local `ling-mem` daemon (proxied
   through `/api/bash` curl).
2. `cat ~/.linggen/memory/.dream-state.json` for the last-hippocampus
   stamp.
3. `head -n 1 ~/.linggen/memory/.scan-output.jsonl` for the last-scan
   header (if any).

It then paints the page deterministically:

- **`top_bar`** — three tier cards (`CORE` / `SEMANTIC` / `EPISODIC`)
  with row counts and `latest Xh ago` subtitles. Episodic card flips
  amber when `count > 50` (staging filling up).
- **`body`** — one `greeting` widget. Title + primary CTA are
  rule-picked by `pickGreeting()` from current state — *"Welcome —
  memory's empty"* on a fresh install, *"N sessions scanned since
  last hippocampus"* when scan is newer than dream, etc. No LLM.
- **`footer`** — *"scan Xh ago · N sessions   ·   hippocampus Yh
  ago"*.

The skill **does not** ask the agent to render the dashboard. The
agent runs in chat-mode for the whole session.

## When the agent IS involved

The header has three action controls, all wired to `_chatSend`:

| Control | Chat message sent | Agent action |
|:---|:---|:---|
| `🔍 Scan` (with period selector: today / 7d / 30d) | `"Scan today"` / `"Scan this week"` / `"Scan this month"` | Run `Bash bash ~/.linggen/skills/shared-memory/scripts/scan.sh <window>`. Summarize the one-line stdout. **No memory writes.** |
| `🧠 Hippocampus` | `"/shared-memory dream"` | Read `~/.linggen/memory/.scan-output.jsonl`, judge, write, promote, evict. Follow `dream-flow.md` end-to-end. Emit a single `PageUpdate` with the report. |
| `Browse ↗` | (link, not a message) | Opens `http://127.0.0.1:9888` in a new tab. |

`/shared-memory dream` is the slash-command form of the hippocampus
button — both route to the same flow.

## Report PageUpdate from hippocampus

See `dream-flow.md` *Report — dashboard mode* for the canonical body
shape. Key constraints:

- **Do NOT emit `top_bar`.** JS owns it and re-fetches counts after
  every PageUpdate (`refreshTierCounts`). Emitting `top_bar` from
  the agent will overwrite the live numbers with a snapshot from
  the agent's mid-run view of the world.
- The greeting widget at the top of the body should carry the
  *"Open in ling-mem console ↗"* deep-link as a primary action with
  `href: http://127.0.0.1:9888/?session=<sid>&since=<run_started>`.
  The dashboard accepts those query params and renders a filtered
  view of just the rows this run wrote.
- Cap `fact-list` items at ~10 per section with a trailing
  *"+N more"* placeholder; the user goes to the console for the
  long tail.

## Row-level UI actions

When the user clicks ✎ / × on a fact-list row, the dashboard sends
the agent a plain chat message — direct user action, no dedup
search:

| Incoming message pattern | Action |
|:---|:---|
| `Delete the <type> fact with id="<id>" and re-render the dashboard. The fact says: "<content>"` | `ling-mem delete <id> --yes`. Then re-emit the report (or omit if the dashboard's JS already removed the row visually). |
| `Update the <type> fact id="<id>" to content: "<new>". Re-render the dashboard.` | `ling-mem edit <id> --content "<new>"`. |

Don't second-guess: apply the single write and move on.

## Session resume — disk-backed cache

Per-session page state lives at
`~/.linggen/skills/shared-memory/data/<sid>/page.json`. When the user
clicks a past session in the sidebar:

1. The page reloads with `?session=<sid>`.
2. `memory-app.js` reads the cached page from disk via `/api/bash`
   `cat ...`.
3. If found and non-empty, `restorePage()` paints it — **no
   boot prompt fires.**
4. Tier counts refresh in the background so the user sees current
   numbers even on resume.

Pulse pattern — survives across browsers, private windows, and
WebRTC remote-access clients.

## What used to be here

This file used to spec a five-state dashboard flow (Open / Waiting /
Scanning / Report) with verbose A+B+C agent rendering instructions
and 7× parallel `Memory_query({verb:"list", type:T})` calls. That
whole flow was retired in the rebuild — JS does the on-open render,
and dream produces the post-action report. If you find references
to the old states elsewhere, treat this file as the source of truth.
