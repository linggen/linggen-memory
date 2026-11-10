import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";

import { readSettings } from "../src/config.mjs";
import { DELEGATE_HEADING, briefField, recallOnDelegate, tellParent } from "../src/recall-on-delegate.mjs";
import { stampCwd } from "../src/stamp-cwd.mjs";

const SHELL_HOOK = new URL("../../linggen/hooks/recall-on-delegate.sh", import.meta.url);
const CLIENT = { lingMemUrl: "http://127.0.0.1:9528/mcp", token: "" };
const BRIEF = "Refactor the sync module so it retries on a flaky network, then run the tests.";
const ROW = { id: "abc123", type: "decision", created_at: "2026-11-02T10:00:00Z", content: "ZEBRA-MARKER retries are capped at 3" };

const settings = (env = {}) => readSettings(env);
const run = (over = {}) =>
  recallOnDelegate({
    toolName: "sessions_spawn",
    params: { task: BRIEF, agentId: "main" },
    cwd: "/nonexistent/repo",
    client: CLIENT,
    settings: settings(),
    search: async () => [ROW],
    ...over,
  });

test("a hit appends the block to the task and touches nothing else", async () => {
  const out = await run();
  assert.equal(out.params.agentId, "main");
  assert.equal(
    out.params.task,
    `${BRIEF}\n\n${DELEGATE_HEADING}\n- (decision, 2026-11-02, id=abc123) ZEBRA-MARKER retries are capped at 3`,
  );
  assert.ok(out.note.startsWith("Recalled for the task you just delegated: " + DELEGATE_HEADING));
});

test("the search carries the brief, the limit and the session root", async () => {
  let seen;
  await run({ search: async (_c, name, args) => ((seen = { name, args }), [ROW]) });
  assert.equal(seen.name, "memory_search");
  assert.deepEqual(seen.args, { query: BRIEF, limit: 3, scope_root: "/nonexistent/repo" });
});

test("agent_run (namespaced MCP tool) rewrites prompt", async () => {
  for (const toolName of ["agent_run", "linggen__agent_run", "mcp__plugin_linggen_linggen__agent_run"]) {
    const out = await run({ toolName, params: { prompt: BRIEF, write_dir: "/x" } });
    assert.ok(out.params.prompt.endsWith("ZEBRA-MARKER retries are capped at 3"), toolName);
    assert.equal(out.params.write_dir, "/x");
  }
  assert.equal(briefField("spawn_agent"), "");
  assert.equal(briefField("memory_search"), "");
  assert.equal(briefField(undefined), "");
});

test("nothing to add leaves the call alone", async () => {
  assert.equal(await run({ search: async () => [] }), null);
  assert.equal(await run({ search: async () => null }), null);
  assert.equal(await run({ search: async () => "nope" }), null);
  assert.equal(await run({ search: async () => [{ type: "x", content: "no id" }] }), null);
  assert.equal(await run({ search: async () => { throw new Error("down"); } }), null);
  assert.equal(await run({ params: { task: "too short to search" } }), null);
  assert.equal(await run({ params: { task: "x".repeat(80) } }), null, "no whitespace: ciphertext, an id");
  assert.equal(await run({ params: { task: 42 } }), null);
  assert.equal(await run({ params: { task: `${BRIEF}\n\n${DELEGATE_HEADING}\n- already` } }), null);
  assert.equal(await run({ toolName: "web_fetch", params: { task: BRIEF } }), null);
});

test("a row the brief already carries is skipped", async () => {
  const seen = { ...ROW, id: "seen01" };
  const out = await run({ params: { task: `${BRIEF} (see id=seen01)` }, search: async () => [seen, ROW] });
  assert.ok(!out.params.task.includes("- (decision, 2026-11-02, id=seen01)"));
  assert.ok(out.params.task.includes("id=abc123"));
  assert.equal(await run({ params: { task: `${BRIEF} (see id=abc123)` } }), null);
});

test("rows are cut to 400 chars and flattened to one line", async () => {
  const long = { ...ROW, content: `${"a".repeat(398)}\n\nb${"c".repeat(100)}` };
  const out = await run({ search: async () => [long] });
  const line = out.params.task.split("\n").at(-1);
  assert.equal(line, `- (decision, 2026-11-02, id=abc123) ${"a".repeat(398)} `);
});

test("env switches", async () => {
  assert.equal(await run({ settings: settings({ LING_MEM_DELEGATE: "0" }) }), null);
  const noteOnly = await run({ settings: settings({ LING_MEM_DELEGATE_REWRITE: "0" }) });
  assert.equal(noteOnly.params, undefined);
  assert.ok(noteOnly.note.includes("ZEBRA-MARKER"));
  let limit;
  await run({ settings: settings({ LING_MEM_DELEGATE_LIMIT: "5" }), search: async (_c, _n, a) => ((limit = a.limit), [ROW]) });
  assert.equal(limit, 5);
});

test("stamp-cwd is unchanged by the delegate path", () => {
  assert.deepEqual(
    stampCwd({ toolName: "ling-mem__memory_search", params: { query: "q" }, cwd: "/nonexistent/repo", sessionId: "s1" }),
    { query: "q", scope_root: "/nonexistent/repo" },
  );
  assert.equal(stampCwd({ toolName: "sessions_spawn", params: { task: BRIEF }, cwd: "/nonexistent/repo", sessionId: "s1" }), null);
});

test("tellParent steers when it can, queues when it cannot, stays quiet otherwise", async () => {
  const target = { sessionKey: "k", agentId: "main", toolCallId: "t1" };
  const calls = [];
  const steering = {
    session: { workflow: { steerActiveRun: async (p) => (calls.push(["steer", p]), { steered: true }) } },
  };
  assert.equal(await tellParent(steering, target, "note"), "steered");
  assert.equal(calls[0][1].idempotencyKey, "ling-mem-delegate:t1");

  const notSteered = {
    session: {
      workflow: {
        steerActiveRun: async () => ({ steered: false, reason: "idle" }),
        enqueueNextTurnInjection: async (p) => (calls.push(["queue", p]), { enqueued: true, id: "i", sessionKey: "k" }),
      },
    },
  };
  assert.equal(await tellParent(notSteered, target, "note"), "queued");
  assert.equal(calls.at(-1)[1].sessionKey, "k");

  const oldHost = { enqueueNextTurnInjection: async (p) => (calls.push(["alias", p]), { enqueued: true, id: "i", sessionKey: "k" }) };
  assert.equal(await tellParent(oldHost, target, "note"), "queued");
  assert.equal(calls.at(-1)[0], "alias");

  assert.equal(await tellParent({}, target, "note"), "");
  assert.equal(await tellParent(oldHost, { ...target, sessionKey: "" }, "note"), "");
  const silent = { enqueueNextTurnInjection: async () => undefined };
  assert.equal(await tellParent(silent, target, "note"), "", "no receipt, no claim");
  const broken = { enqueueNextTurnInjection: async () => { throw new Error("x"); } };
  assert.equal(await tellParent(broken, target, "note"), "");
});

test("the heading matches the Claude Code / Codex hook's", { skip: !existsSync(SHELL_HOOK) }, () => {
  assert.ok(readFileSync(SHELL_HOOK, "utf8").includes(`heading='${DELEGATE_HEADING}'`));
});
