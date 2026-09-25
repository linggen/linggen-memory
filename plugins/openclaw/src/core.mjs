// Always-on core identity + standing rules — the OpenClaw port of the core-memory block that
// `plugins/linggen/hooks/autostart.sh` emits as `additionalContext` on
// SessionStart.
//
// OpenClaw's `session_start` hook is observation-only and cannot inject, so the
// block rides `before_prompt_build` instead and is returned as
// `prependSystemContext`: it joins the *system* prompt, which providers cache,
// so an always-on block costs its tokens once rather than once per turn.
//
// Read over MCP, like recall — so a host whose store is on another machine gets
// the same core identity as one whose store is local, with no binary of its own.

import { mcpCall } from "./rpc.mjs";

/**
 * Build the session-start block — core identity plus the standing rules that
 * apply at `cwd` — or "" when the store is empty (a fresh install gets a
 * normal session with no injected block).
 *
 * One `memory_session_start` call: the daemon picks the rules (type=preference,
 * global or written at this project or a parent of it), applies the char
 * budget and renders the block, so every host injects the same text. A cwd
 * that is not a project is sent anyway — the daemon loads globals only.
 *
 * A slightly longer budget than a per-turn recall: this runs once per session,
 * and a cold daemon has just been asked to open LanceDB.
 */
export async function buildCoreContext(client, timeoutMs = 5000, cwd = "") {
  const args = cwd ? { cwd } : {};
  const start = await mcpCall(client, "memory_session_start", args, timeoutMs);
  if (start && typeof start === "object" && !Array.isArray(start)) {
    return typeof start.block === "string" ? start.block : "";
  }

  // A daemon older than session_start (ling-mem < 1.9) answers with an error:
  // fall back to the core rows alone, as before.
  const rows = await mcpCall(client, "memory_list", { tier: "core", limit: 100 }, timeoutMs);
  if (!Array.isArray(rows)) return "";
  const lines = rows.filter((row) => row?.content).map((row) => `- ${row.content} (id=${row.id})`);
  if (!lines.length) return "";
  return `## Core memory — who the user is\n\n${lines.join("\n")}`;
}
