// Is the memory store reachable, and if not, what should be done about it.
//
// The Claude Code plugin installs the binary and starts the daemon itself, from
// `autostart.sh`. This plugin deliberately does neither: OpenClaw's installer
// blocks any plugin that spawns processes, and a memory plugin that can
// only be installed with `--dangerously-force-unsafe-install` is not a plugin
// anyone should install. Nor should it be — a background process spawning
// installers is exactly the shape that gate exists to catch.
//
// So the plugin diagnoses and the agent acts. The agent already has a shell
// tool, running under OpenClaw's own approval flow, and the skill already ships
// the installer it needs. That keeps the privileged step visible and the
// plugin honest: it reads HTTP and writes config, nothing more.

import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

import { alive } from "./rpc.mjs";

/** ling-mem's health endpoint sits beside its MCP route. */
function baseOf(mcpUrl) {
  return mcpUrl.replace(/\/mcp\/?$/, "");
}

/**
 * Returns "" when memory is healthy, or a block of instructions for the agent
 * when it is not. Delivered once per session as system context, so it must read
 * as guidance rather than an error.
 */
export async function memoryNotice(client, pluginRoot) {
  if (await alive(baseOf(client.lingMemUrl))) return "";

  // A host pointed at another machine installs nothing: a local daemon would
  // answer from a DIFFERENT store and silently fork the user's memory in two.
  if (!client.lingMemLocal) {
    return [
      "## Linggen memory is unreachable",
      "",
      `This host is configured to use the memory store at ${client.lingMemUrl}, and that address is not answering.`,
      "Nothing should be installed here — the store lives on another machine, and a second local daemon would be a second store.",
      "Tell the user their Linggen host looks down or unreachable on the network, and that `~/.linggen/client.json` holds the address this host is trying.",
    ].join("\n");
  }

  const installed = existsSync(join(homedir(), ".local", "bin", "ling-mem"));
  const bootstrap = join(pluginRoot, "skills", "linggen", "scripts", "bootstrap.sh");

  const fix = installed
    ? "The `ling-mem` binary is installed but its daemon is not running. Start it with your shell tool: `ling-mem start`"
    : `The \`ling-mem\` binary is not installed yet. Install it with your shell tool: \`bash ${bootstrap}\` — one-time, SHA-256 verified, and it also fetches the Linggen engine (~100MB) for the browser and X tools. Say what you are running before you run it.`;

  return [
    "## Linggen memory is offline this session",
    "",
    `Nothing is answering at ${client.lingMemUrl}, so automatic recall and the \`memory_*\` tools are unavailable right now.`,
    fix,
    "Once it is up, the memory tools appear after the next gateway restart.",
  ].join("\n");
}
