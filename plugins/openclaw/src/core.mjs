// What a session loads at start — the OpenClaw port of the block that
// `plugins/linggen/hooks/autostart.sh` emits as `additionalContext` on
// SessionStart: core identity, the "Memory scopes here" candidates line and
// the index of standing rows filed for this directory and its parents.
//
// OpenClaw's `session_start` hook is observation-only and cannot inject, so the
// block rides `before_prompt_build` instead and is returned as
// `prependSystemContext`: it joins the *system* prompt, which providers cache,
// so an always-on block costs its tokens once rather than once per turn.
//
// Read over MCP, like recall — so a host whose memory is on another machine gets
// the same core identity as one whose memory is local, with no binary of its own.

import { mcpCall } from "./rpc.mjs";
import { memoryRoot } from "./scope.mjs";

/**
 * Build the session-start block — core identity, plus the scope candidates and
 * the index for `cwd` — or "" when the store has nothing to say (a fresh
 * install gets a normal session with no injected block).
 *
 * One `memory_session_start` call: Linggen renders the block, so every host
 * injects the same text. Everything else surfaces through per-turn recall.
 *
 * A slightly longer budget than a per-turn recall: this runs once per session,
 * and an engine that just started may still be opening the store.
 */
export async function buildCoreContext(client, timeoutMs = 5000, cwd = "") {
  const args = cwd ? { cwd, root: memoryRoot(cwd) } : {};
  const start = await mcpCall(client, "memory_session_start", args, timeoutMs);
  if (start && typeof start === "object" && !Array.isArray(start)) {
    return typeof start.block === "string" ? start.block : "";
  }
  return "";
}
