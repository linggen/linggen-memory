// Stamp where the session stands onto memory calls — the OpenClaw port of
// `plugins/linggen/hooks/stamp-cwd.sh` (linggen-memory doc/scope-index-spec.md):
//
//   memory_add           cwd (the row's default scope), root (what a model's
//                        `scope` resolves against), source_session, host
//   memory_search        scope_root = root, inside a project only
//   memory_session_start cwd, root
//
// The host knows where the session is working; the model does not. Claude
// Code rewrites the tool input from `PreToolUse`; OpenClaw's equivalent is
// `before_tool_call`, whose result may return replacement `params`.
//
// One difference forced by the host: OpenClaw's tool context carries no
// workspace dir, so the caller supplies the one observed for this session in
// `before_prompt_build`. Same value, one hop later.
//
// Bails silently on anything unexpected — a memory write must never fail
// because attribution could not be worked out.

import { readSettings } from "./config.mjs";
import { isProjectDir, memoryRoot } from "./scope.mjs";

/**
 * Decide the rewritten params for a memory call, or null to leave it alone.
 *
 * @param {string} toolName   namespaced tool name as the host reports it
 * @param {object} params     the call's arguments
 * @param {string} cwd        workspace dir observed for this session
 * @param {string} sessionId  this session's id
 */
export function stampCwd({ toolName, params, cwd, sessionId, settings } = {}) {
  if ((settings ?? readSettings()).stampDisabled) return null;
  if (!toolName || !params || typeof params !== "object") return null;

  // Suffix match, because the tool arrives namespaced by server and that
  // prefix is the user's to choose.
  let verb = "";
  if (toolName.endsWith("memory_add")) verb = "add";
  else if (toolName.endsWith("memory_search")) verb = "search";
  else if (toolName.endsWith("memory_session_start")) verb = "session_start";
  else return null;

  const root = memoryRoot(cwd);
  const stamp = {};
  // A field the caller set wins — the one legitimate case is a promote pass
  // carrying the ORIGINAL row's session, cwd and host forward.
  const fill = (field, value) => {
    if (value && !params[field]) stamp[field] = value;
  };

  if (verb === "add") {
    // A write that names ANOTHER session's row is not this session's
    // authorship: the dream's promote and the scan's backfill carry the
    // original row's source_session — and its cwd, when it had one. This
    // session's paths stamped over the gap would rescope someone else's
    // memory to wherever the dream happened to run.
    const source = params.source_session;
    const foreign = source && sessionId && source !== sessionId;
    if (!foreign) {
      fill("source_session", sessionId);
      fill("cwd", cwd);
      fill("root", root);
    }
    fill("host", "openclaw");
  } else if (verb === "search") {
    // A search the model makes itself is scoped only inside a project; at
    // $HOME it is a deliberate whole-store lookup. Per-turn recall is the one
    // that narrows those sessions to rows about the person.
    if (isProjectDir(root)) fill("scope_root", root);
  } else {
    fill("cwd", cwd);
    fill("root", root);
  }

  if (!Object.keys(stamp).length) return null;
  return { ...params, ...stamp };
}
