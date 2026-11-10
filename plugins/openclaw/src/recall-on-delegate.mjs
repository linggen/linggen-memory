// Recall on delegate — the OpenClaw port of `plugins/linggen/hooks/recall-on-delegate.sh`.
//
// When the model hands work to another agent, the brief it writes is all that
// agent sees: the mid-turn agent misses memory, and a stale claim in the brief
// goes uncorrected. So the delegated brief's text is searched in ling-mem and
// the hits are appended to it; the parent model is told the same block.
//
//   Claude Code / Codex  PreToolUse    → updatedInput + additionalContext
//   OpenClaw             before_tool_call → { params }  + a note to the parent
//
// Tools covered, by base name (the part after the last `__`, because MCP tools
// arrive namespaced by server — `linggen__agent_run`):
//   sessions_spawn  → params.task     OpenClaw's own sub-agent / ACP spawn
//   agent_run       → params.prompt   the Linggen engine's MCP tool
//
// Never blocks the host: `before_tool_call` BLOCKS the call on a throw or a
// timeout, so every failure here is silence (null), and the search is bounded
// by the recall timeout, well under the hook's 15 s budget.

import { mcpCall } from "./rpc.mjs";
import { memoryRoot } from "./scope.mjs";

/** One heading, shared by the block, the already-recalled check and the shell
 *  hook (`plugins/linggen/hooks/recall-on-delegate.sh` carries the same text). */
export const DELEGATE_HEADING =
  "From memory (ling-mem) — recalled for this task; newer rows win over what the brief says:";

/** Delegation tool (base name) → the parameter that carries the brief. */
const BRIEF_FIELD = {
  sessions_spawn: "task",
  agent_run: "prompt",
};

const MIN_BRIEF_CHARS = 40;
const QUERY_CHARS = 2000;
const ROW_CHARS = 400;

const chars = (text) => Array.from(text);
const cut = (text, n) => chars(text).slice(0, n).join("");

/** Which param holds the brief for this tool, or "" when it is not a delegation. */
export function briefField(toolName) {
  if (typeof toolName !== "string") return "";
  const base = toolName.split("__").pop();
  return Object.hasOwn(BRIEF_FIELD, base) ? BRIEF_FIELD[base] : "";
}

/** Worth searching on? Too short, a single token (ciphertext, an id) or already
 *  carrying the block: no. */
function searchable(brief) {
  return (
    typeof brief === "string" &&
    chars(brief).length >= MIN_BRIEF_CHARS &&
    /\s/.test(brief) &&
    !brief.includes(DELEGATE_HEADING)
  );
}

function renderRow(row) {
  const date = String(row.created_at ?? "").slice(0, 10);
  const content = cut(String(row.content ?? ""), ROW_CHARS).replace(/[\r\n]+/g, " ");
  return `- (${row.type}, ${date}, id=${row.id}) ${content}`;
}

/** The block for a brief, or "" when no row is worth adding. A row whose id the
 *  brief already carries is skipped — the brief has seen it. */
export function renderBlock(rows, brief) {
  if (!Array.isArray(rows)) return "";
  const lines = rows
    .filter((row) => typeof row?.id === "string" && row.id.length > 0 && !brief.includes(row.id))
    .map(renderRow);
  return lines.length ? `${DELEGATE_HEADING}\n${lines.join("\n")}` : "";
}

/**
 * Recall for a delegated brief.
 *
 * @returns {Promise<{params?: object, note: string} | null>}
 *   `params` is the rewritten call (absent when rewriting is off); `note` is
 *   what the parent model should be told. null = leave the call alone.
 */
export async function recallOnDelegate({ toolName, params, cwd, client, settings, search = mcpCall } = {}) {
  try {
    if (settings.delegateDisabled) return null;
    const field = briefField(toolName);
    const brief = field && params ? params[field] : undefined;
    if (!searchable(brief)) return null;

    const args = { query: cut(brief, QUERY_CHARS), limit: settings.delegateLimit };
    const scope = memoryRoot(cwd);
    if (scope) args.scope_root = scope;

    const block = renderBlock(await search(client, "memory_search", args, settings.recallTimeoutMs), brief);
    if (!block) return null;

    const note = `Recalled for the task you just delegated: ${block}`;
    if (!settings.delegateRewrite) return { note };
    return { params: { ...params, [field]: `${brief}\n\n${block}` }, note };
  } catch {
    return null;
  }
}

/**
 * Tell the parent model. `before_tool_call` has no field for context, so this
 * goes through the session API: `steerActiveRun` (same turn, newer OpenClaw),
 * else `enqueueNextTurnInjection` (next turn; also on 2026.5.x). Feature-
 * detected, never assumed; a failure here must not cost the rewrite.
 *
 * @returns {Promise<"steered" | "queued" | "">}
 */
export async function tellParent(api, { sessionKey, agentId, toolCallId }, text) {
  if (!sessionKey || !text) return "";
  const key = `ling-mem-delegate:${toolCallId ?? Date.now()}`;
  const workflow = api?.session?.workflow;
  try {
    if (typeof workflow?.steerActiveRun === "function") {
      const sent = await workflow.steerActiveRun({ sessionKey, text, idempotencyKey: key });
      if (sent?.steered) return "steered";
    }
    const enqueue = workflow?.enqueueNextTurnInjection ?? api?.enqueueNextTurnInjection;
    if (typeof enqueue !== "function") return "";
    const queued = await enqueue.call(workflow ?? api, {
      sessionKey,
      ...(agentId ? { agentId } : {}),
      text,
      idempotencyKey: key,
      ttlMs: 60 * 60 * 1000,
    });
    // 2026.5.x resolved with nothing at all (live run): count only a receipt.
    return queued?.enqueued === true ? "queued" : "";
  } catch {
    return "";
  }
}
