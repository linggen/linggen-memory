// The /linggen-* slash commands.
//
// The bodies are the same markdown runbooks the Claude Code plugin ships in
// `commands/` — one source, read at registration rather than transcribed into
// JavaScript, so a change to a runbook reaches every host.
//
// Where a Claude Code slash command becomes the user's prompt, an OpenClaw
// plugin command returns a *reply* that is sent to the channel. Printing a
// runbook into the chat would be noise the user never asked to read, so the
// body goes to the model instead — `enqueueNextTurnInjection` puts it in the
// next turn's context exactly once — and the handler returns `continueAgent`
// so that turn happens immediately.

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

/** Split `---\nkey: value\n---\nbody` without taking on a YAML dependency:
 *  these files use flat scalar frontmatter and nothing else. */
function parseDoc(text) {
  const match = /^---\n([\s\S]*?)\n---\n?([\s\S]*)$/.exec(text);
  if (!match) return { meta: {}, body: text.trim() };
  const meta = {};
  for (const line of match[1].split("\n")) {
    const pair = /^([A-Za-z-]+):\s*(.*)$/.exec(line.trim());
    if (!pair) continue;
    meta[pair[1]] = pair[2].replace(/^["']|["']$/g, "").trim();
  }
  return { meta, body: match[2].trim() };
}

/** Read every command runbook shipped with the plugin. */
export function loadCommandDocs(pluginRoot) {
  const dir = join(pluginRoot, "commands");
  let entries = [];
  try {
    entries = readdirSync(dir).filter((file) => file.endsWith(".md")).sort();
  } catch {
    return [];
  }
  const docs = [];
  for (const file of entries) {
    try {
      const { meta, body } = parseDoc(readFileSync(join(dir, file), "utf8"));
      docs.push({
        verb: file.replace(/\.md$/, ""),
        description: meta.description ?? `Linggen ${file.replace(/\.md$/, "")}`,
        argumentHint: meta["argument-hint"] ?? "",
        body,
      });
    } catch {
      /* an unreadable runbook is one missing command, not a broken plugin */
    }
  }
  return docs;
}

/** Substitute the two placeholders the runbooks use. */
export function renderBody(body, { args, pluginRoot }) {
  return body
    .replaceAll("$ARGUMENTS", args)
    .replaceAll("${CLAUDE_PLUGIN_ROOT}", pluginRoot)
    .replaceAll("${PLUGIN_ROOT}", pluginRoot);
}

function argsOf(ctx) {
  if (Array.isArray(ctx?.args)) return ctx.args.join(" ").trim();
  if (typeof ctx?.args === "string") return ctx.args.trim();
  if (typeof ctx?.commandBody === "string") return ctx.commandBody.trim();
  return "";
}

/**
 * Register one command per runbook, named `linggen-<verb>`. Returns the names
 * registered so the caller can log them.
 */
export function registerCommands({ api, pluginRoot, logger }) {
  const registered = [];
  for (const doc of loadCommandDocs(pluginRoot)) {
    const name = `linggen-${doc.verb}`;
    try {
      api.registerCommand({
        name,
        description: doc.description,
        acceptsArgs: true,
        handler: async (ctx) => {
          const text = renderBody(doc.body, { args: argsOf(ctx), pluginRoot });
          const sessionKey = ctx?.sessionKey;
          if (!sessionKey) return { text, continueAgent: true };
          await api.enqueueNextTurnInjection({
            sessionKey,
            text,
            idempotencyKey: `linggen-${doc.verb}-${Date.now()}`,
            ttlMs: 300000,
          });
          return { continueAgent: true };
        },
      });
      registered.push(name);
    } catch (error) {
      logger?.warn?.(`linggen: could not register /${name} (${error?.message ?? "error"})`);
    }
  }
  return registered;
}
