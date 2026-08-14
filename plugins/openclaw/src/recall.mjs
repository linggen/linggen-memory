// Per-turn recall — the OpenClaw port of `plugins/linggen/hooks/recall.sh`.
//
// Claude Code runs that script on `UserPromptSubmit` and treats its stdout as
// added context. OpenClaw has no such event; the equivalent seam is the
// `before_prompt_build` hook, which returns `{ prependContext }`. Same payload,
// same order, same failure policy: any failure is silence, never a blocked turn.
//
// Talks to ling-mem over MCP, not the CLI — so this needs a URL and (off-machine)
// a token, and **no `ling-mem` binary at all**. The CLI resolves the daemon
// through `daemon.json`, which can only ever describe a *local* daemon, so a
// second machine on CLI recall would need its own binary, its own daemon and its
// own store: memory forked in two.

import { readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir, userInfo } from "node:os";
import { join } from "node:path";

import { readSettings } from "./config.mjs";
import { mcpCall } from "./rpc.mjs";

const CAPTURE_NUDGE =
  "Memory capture: before finishing this turn, recognize anything worth remembering and write it at the right tier per the memory protocol (core/semantic = search-first; episodic = incidental); anchor relative time to absolute dates (\"last month\" → \"2026-06\"). Nothing worth keeping? Skip silently.";

// Mirrors linggen/src/engine/prompt/core_block.rs:RECONCILE_FOOTER, with the
// ask primitive named for THIS host: OpenClaw has no structured ask widget, so
// the resolution question is asked in plain chat.
const RECONCILE_NOTE =
  "Note: If a recalled row above duplicates or conflicts with another row or with what the user just said AND the user's current turn is unrelated to memory itself (incidental recall hit), resolve it on the side — merge authority follows voice: memory_delete for exact dups; rows that are all your own notes (from=derived — built/fixed/tried/learned) merge freely into one current-truth row via memory_add with replace_ids listing the losers (atomic insert + delete), no ask; if any row is in the user's voice (from=user — preference/decision/identity), ask first — on this host that is a plain chat question, not a widget — then write the winner via memory_add with replace_ids AND user_directed:true — the daemon BLOCKS user-voice replaces without that flag, and a hedged reflection (\"X feels about right to me\") never justifies it (never separate add + delete). If the user IS explicitly steering memory (\"clean up\", \"remember X\", \"what's in memory\", \"ignore the hits\"), follow their instruction and do NOT side-quest into dedup. Either way, keep memory in good shape.";

/**
 * A cwd that is not a project must not become a scope. `$HOME` means "no
 * particular work" and would claim every repo underneath; `~/.linggen` is the
 * engine's state dir; a temp dir is nobody's project. Scoping a read to any of
 * these hides every project row from the reader — worse than no scope. Same
 * dirs the write side refuses in stampCwd().
 */
export function scopeOf(cwd) {
  if (!cwd) return "";
  const home = homedir();
  const linggen = join(home, ".linggen");
  const tmp = tmpdir().replace(/\/$/, "");
  const under = (base) => cwd === base || cwd.startsWith(`${base}/`);
  if (cwd === home) return "";
  if (under(linggen)) return "";
  if (under(tmp) || under("/tmp") || under("/private/tmp")) return "";
  return cwd;
}

function renderHit(row) {
  const score = row?.hybrid_score ?? row?.score ?? 0;
  const rounded = Math.floor(score * 100) / 100;
  const date = String(row?.created_at ?? "").slice(0, 10);
  const host = row?.host ?? "unknown";
  return `From memory (${row?.type}, ${host}, ${date}, score=${rounded}, id=${row?.id}): ${row?.content}`;
}

/**
 * Memory-upkeep nudge — undreamed days + open review items, from ONE
 * `memory_days` call. Cached (default 30 min) so the recall path stays fast;
 * upkeep state moves slowly. Thresholds: ≥2 undreamed days (the engine's own
 * 3am cron usually covers yesterday; nagging about one day is noise) and ≥1
 * open review item.
 */
async function upkeepLine(client, ttlMin, timeoutMs) {
  let uid = "0";
  try {
    uid = String(userInfo().uid);
  } catch {
    /* fall through to the shared name */
  }
  const cache = join(tmpdir(), `ling-mem-upkeep-${uid}`);

  try {
    const age = (Date.now() - statSync(cache).mtimeMs) / 60000;
    if (age < ttlMin) return readFileSync(cache, "utf8");
  } catch {
    /* no usable cache — fetch below */
  }

  const days = await mcpCall(client, "memory_days", { undreamed_only: true }, timeoutMs);
  if (!days) return "";

  const pending = Array.isArray(days.days) ? days.days.length : 0;
  const issues = Number(days.open_issues ?? 0);
  const oldest = days.days?.[0]?.date ?? "";
  const lines = [];
  if (pending >= 2) {
    lines.push(
      `memory upkeep: ${pending} days undreamed (oldest ${oldest}) — offer to run the dream: /linggen:dream runs it with this session's model; memory_dream_run offloads it to the Linggen engine`,
    );
  }
  if (issues >= 1) {
    lines.push(`memory upkeep: ${issues} item(s) awaiting review — offer /linggen:solve (memory_issues has the facts)`);
  }
  const text = lines.join("\n");
  try {
    writeFileSync(cache, text);
  } catch {
    /* an unwritable temp dir only costs us the cache */
  }
  return text;
}

/**
 * Build the per-turn context block. Returns "" when there is nothing to add —
 * the caller then omits `prependContext` entirely rather than injecting an
 * empty string.
 */
export async function buildRecallContext({ client, prompt, cwd, sessionId, settings } = {}) {
  // No hardcoded score floor: with no `min_score` the daemon applies its
  // store-wide `recall_min_score` (one selectivity shared by all hosts). Set
  // LING_MEM_RECALL_MIN_SCORE to tighten it per host — applied client-side,
  // because `min_score` is deliberately absent from the MCP schema (a *model*
  // guessing a threshold narrows recall to zero).
  const { recallDisabled, topK, limit, recallTimeoutMs, minScore, upkeepCacheMinutes } =
    settings ?? readSettings();
  if (recallDisabled) return "";
  if (typeof prompt !== "string" || prompt.length < 8) return "";

  // Scope the search to the work being done here — rows written under this
  // path, plus every row that belongs to no project. Sent to the daemon rather
  // than applied to its answer: the scope has to shape the ranking, because a
  // filter applied afterwards can only shrink a list that was already the
  // wrong N.
  const scope = scopeOf(cwd);
  const args = { query: prompt, limit };
  if (scope) args.cwd_scope = scope;

  const rows = await mcpCall(client, "memory_search", args, recallTimeoutMs);
  if (rows === null) return "";

  const hits = (Array.isArray(rows) ? rows : [])
    .filter((row) => (row?.hybrid_score ?? row?.score ?? 0) >= minScore)
    .slice(0, topK)
    .map(renderHit);

  const sections = [];
  if (hits.length) sections.push(hits.join("\n"));

  const upkeep = await upkeepLine(client, upkeepCacheMinutes, recallTimeoutMs);
  if (upkeep) sections.push(upkeep);

  // Always-on capture nudge — fires EVERY turn, including zero-hit turns
  // (often the very turns that produce new memory).
  sections.push(CAPTURE_NUDGE);

  // Session stamp: pass source_session on every add so a later scan of this
  // day's logs skips sessions that already contributed (idempotent backfill).
  if (sessionId) {
    sections.push(`On every memory_add, pass source_session:"${sessionId}" (this session).`);
  }

  // Fires on ANY hit: a single recalled row can still conflict with the user's
  // current turn, and the daemon's user-voice guard needs the resolved write to
  // carry user_directed:true.
  if (hits.length >= 1) sections.push(RECONCILE_NOTE);

  return sections.join("\n\n");
}
