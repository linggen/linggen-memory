// Stamp the project scope onto memory calls — the OpenClaw port of
// `plugins/linggen/hooks/stamp-cwd.sh`.
//
// The host knows where the session is working; the model does not, and a model
// guessing a scope is how rows end up labelled with an IP address. Claude Code
// rewrites the tool input from `PreToolUse`; OpenClaw's equivalent is
// `before_tool_call`, whose result may return replacement `params`.
//
// One difference forced by the host: OpenClaw's tool context carries no
// workspace dir, so the caller supplies the one observed for this session in
// `before_prompt_build`. Same value, one hop later.
//
// Bails silently on anything unexpected — a memory write must never fail
// because attribution could not be worked out.

import { readSettings } from "./config.mjs";
import { scopeOf } from "./recall.mjs";

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

  // A cwd that is not a project must never become one. Stamping `$HOME`, the
  // engine's own `~/.linggen`, or a temp dir onto a write HIDES the row from
  // every project search — a scope that is not a project is worse than no
  // scope. Same rule the read side applies in recall.
  const scope = scopeOf(cwd);
  if (!scope) return null;

  // Which field this tool wants. A write records where it came from; a read
  // asks what is in scope. Same value, opposite direction. Suffix match,
  // because the tool arrives namespaced by server and that prefix is the
  // user's to choose.
  let field = "";
  if (toolName.endsWith("memory_add")) field = "cwd";
  else if (toolName.endsWith("memory_search")) field = "cwd_scope";
  else return null;

  // Never overwrite a value the caller set deliberately. The one legitimate
  // case is a promote pass carrying the ORIGINAL row's origin forward — the
  // dream knows where a memory came from and this hook does not.
  if (params[field]) return null;

  // A write that names ANOTHER session's row is not this session's authorship.
  // The dream's promote and the scan's backfill carry the original row's
  // source_session — and its cwd, when it had one, rides in the same call. When
  // the original had none, this session's cwd stamped over the gap would
  // rescope someone else's memory to wherever the dream happened to run.
  if (field === "cwd") {
    const source = params.source_session;
    if (source && sessionId && source !== sessionId) return null;
  }

  return { ...params, [field]: scope };
}
