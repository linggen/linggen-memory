// Linggen for OpenClaw — durable cross-host memory, recalled automatically.
//
// The Claude Code plugin does this with three shell hooks; OpenClaw has typed
// runtime hooks instead, so the same behaviours land here:
//
//   SessionStart core block  → before_prompt_build → prependSystemContext
//   UserPromptSubmit recall  → before_prompt_build → prependContext
//   PreToolUse stamp-cwd     → before_tool_call    → params
//
// The fourth CC behaviour — installing the binary and starting the daemon —
// deliberately does NOT come across. See health.mjs: this plugin diagnoses and
// the agent acts, through its own shell tool and OpenClaw's approval flow.
//
// Nothing here may block or break a turn. Every hook is wrapped, every failure
// is silence, and a host whose daemon is down simply gets no memory that turn.

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { readSettings, resolveClient } from "./config.mjs";
import { registerCommands } from "./commands.mjs";
import { buildCoreContext } from "./core.mjs";
import { memoryNotice } from "./health.mjs";
import { ensureMcpServers } from "./mcp-config.mjs";
import { buildRecallContext } from "./recall.mjs";
import { stampCwd } from "./stamp-cwd.mjs";

// The real helper is `import { definePluginEntry } from
// "openclaw/plugin-sdk/plugin-entry"`. We do NOT import it directly so this
// module stays Node-builtins-only and passes `node --check` and unit tests
// without the OpenClaw SDK installed. definePluginEntry returns the definition
// object it is given, so an inline fallback is behaviourally equivalent for a
// loaded plugin.
function definePluginEntry(definition) {
  return definition;
}

/** Bounded so a long-lived gateway cannot grow its session bookkeeping without
 *  limit. */
const MAX_TRACKED_SESSIONS = 512;

function remember(store, key, value) {
  store.set ? store.set(key, value) : store.add(key);
  if (store.size <= MAX_TRACKED_SESSIONS) return;
  const oldest = store.keys().next().value;
  if (oldest !== undefined) store.delete(oldest);
}

function resolvePluginRoot(api) {
  if (api?.rootDir) return api.rootDir;
  return join(dirname(fileURLToPath(import.meta.url)), "..");
}

function registerSetupCli(api, logger) {
  if (typeof api?.registerCli !== "function") return;
  api.registerCli(
    ({ program }) => {
      program
        .command("linggen")
        .description("Configure and inspect the Linggen plugin")
        .command("setup")
        .description("Point this host's MCP config at the Linggen daemons")
        .action(async () => {
          const client = resolveClient();
          const added = await ensureMcpServers({ api, client, logger });
          process.stdout.write(
            [
              `  linggen   ${client.linggenUrl}/mcp`,
              `  ling-mem  ${client.lingMemUrl}`,
              added.length ? `Added: ${added.join(", ")}` : "Already configured — nothing changed.",
              "Restart OpenClaw, then: openclaw mcp list",
              "",
            ].join("\n"),
          );
        });
    },
    {
      descriptors: [
        { name: "linggen", description: "Configure and inspect the Linggen plugin", hasSubcommands: true },
      ],
    },
  );
}

const plugin = definePluginEntry({
  id: "linggen",
  name: "Linggen",
  description:
    "Durable cross-host memory that recalls itself: core identity in the system prompt, relevant memories every turn, and the project scope stamped on every write.",
  register(api) {
    const logger = api?.logger;
    const pluginRoot = resolvePluginRoot(api);

    // Commands are metadata — register them in every mode so a discovery pass
    // still reports what this plugin offers.
    registerCommands({ api, pluginRoot, logger });
    registerSetupCli(api, logger);

    const activatesRuntime = !api?.registrationMode || api.registrationMode === "full";
    if (!activatesRuntime) return;

    // Fire-and-forget: a config write that fails leaves the user exactly where
    // they were, and the hooks below work regardless.
    Promise.resolve()
      .then(() => ensureMcpServers({ api, client: resolveClient(), logger }))
      .catch((error) => logger?.warn?.(`linggen: MCP config not written (${error?.message ?? "error"})`));

    const coreSent = new Set();
    const workspaceBySession = new Map();

    api.on(
      "before_prompt_build",
      async (event, ctx) => {
        try {
          const client = resolveClient();
          const settings = readSettings();
          const sessionKey = ctx?.sessionKey ?? ctx?.sessionId ?? "";
          if (ctx?.workspaceDir) remember(workspaceBySession, sessionKey, ctx.workspaceDir);

          const result = {};

          // Core identity goes in ONCE per session, as system context: providers
          // cache the system prompt, so an always-on block costs its tokens once
          // rather than once per turn. A dead daemon yields instructions for
          // bringing it back instead.
          if (!coreSent.has(sessionKey)) {
            remember(coreSent, sessionKey);
            const notice = await memoryNotice(client, pluginRoot);
            const system = notice || (await buildCoreContext(client, settings.coreTimeoutMs));
            if (system) result.prependSystemContext = system;
          }

          const recall = await buildRecallContext({
            client,
            settings,
            prompt: event?.prompt,
            cwd: ctx?.workspaceDir,
            sessionId: ctx?.sessionId ?? sessionKey,
          });
          if (recall) result.prependContext = recall;

          return Object.keys(result).length ? result : undefined;
        } catch (error) {
          logger?.warn?.(`linggen: recall skipped (${error?.name ?? "error"})`);
          return undefined;
        }
      },
      { timeoutMs: 10000 },
    );

    api.on("before_tool_call", async (event, ctx) => {
      try {
        const sessionKey = ctx?.sessionKey ?? ctx?.sessionId ?? "";
        const params = stampCwd({
          toolName: event?.toolName,
          params: event?.params,
          cwd: workspaceBySession.get(sessionKey),
          sessionId: ctx?.sessionId ?? sessionKey,
        });
        return params ? { params } : undefined;
      } catch {
        return undefined;
      }
    });

    logger?.info?.("linggen: memory hooks registered (recall + core identity + cwd stamping)");
  },
});

export default plugin;
