// Is Linggen reachable, and if not, what should be done about it.
//
// The Claude Code plugin installs and starts Linggen itself, from
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
import { delimiter, join } from "node:path";

import { alive } from "./rpc.mjs";

/** Is `ling` installed — on PATH or in the installer's fallback dir? */
function lingInstalled(env = process.env) {
  const dirs = [...String(env.PATH ?? "").split(delimiter), join(homedir(), ".local", "bin")];
  return dirs.some((dir) => dir && existsSync(join(dir, "ling")));
}

/**
 * Returns "" when Linggen is reachable, or a block of instructions for the
 * agent when it is not. Delivered once per session as system context, so it
 * must read as guidance rather than an error.
 */
export async function memoryNotice(client, pluginRoot) {
  if (await alive(client.linggenUrl)) return "";

  // A host pointed at another machine installs and starts nothing: a local
  // Linggen would open a DIFFERENT store and silently fork the user's memory.
  if (!client.local) {
    return [
      "## Linggen memory is unreachable",
      "",
      `This host is configured to use the Linggen at ${client.linggenUrl}, and that address is not answering.`,
      "Nothing should be installed or started here — memory lives on that machine, and a local Linggen would be a second store. Do not run `ling mem` here either.",
      "Tell the user their Linggen machine looks down or unreachable on the network, and that `~/.linggen/client.json` holds the address this host is trying.",
    ].join("\n");
  }

  const bootstrap = join(pluginRoot, "skills", "linggen", "scripts", "bootstrap.sh");
  const fix = lingInstalled()
    ? "Linggen (`ling`) is installed but not running. Start it with your shell tool: `ling --web` (in the background), or ask the user to open Linggen.app."
    : `Linggen is not installed yet. ASK the user first — one line: "Linggen needs its local engine (~100MB, SHA-verified, to /usr/local/bin or ~/.local/bin) — install now?" Only on their yes, run with your shell tool: \`bash ${bootstrap}\`. The installer ships inside this plugin; no remotely fetched script is executed.`;

  return [
    "## Linggen memory is offline this session",
    "",
    `Nothing is answering at ${client.linggenUrl}, so automatic recall and the \`memory_*\` tools are unavailable right now.`,
    fix,
    "Once it is up, the memory tools appear after the next gateway restart.",
  ].join("\n");
}
