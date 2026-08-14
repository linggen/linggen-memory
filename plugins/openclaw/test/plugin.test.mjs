// Runs on `node --test` with no dependencies to install — the plugin itself is
// Node-builtins-only, and its tests should not be the thing that drags a
// toolchain into a Rust repo.
//
// Covers the pure decisions only: which field a tool gets stamped, which
// directories refuse to become a scope, whether MCP config is left alone the
// second time. The daemon-facing paths are exercised against a live ling-mem
// during install verification, not here.

import assert from "node:assert/strict";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { renderBody } from "../src/commands.mjs";
import { configureLinggenMcp } from "../src/mcp-config.mjs";
import { scopeOf } from "../src/recall.mjs";
import { stampCwd } from "../src/stamp-cwd.mjs";

const CLIENT = {
  linggenUrl: "http://127.0.0.1:9527",
  lingMemUrl: "http://127.0.0.1:9528/mcp",
  linggenLocal: true,
  lingMemLocal: true,
  token: "",
};

test("scopeOf keeps a project and refuses a non-project", () => {
  assert.equal(scopeOf("/Users/x/workspace/repo"), "/Users/x/workspace/repo");
  assert.equal(scopeOf(homedir()), "");
  assert.equal(scopeOf(join(homedir(), ".linggen")), "");
  assert.equal(scopeOf(join(homedir(), ".linggen", "activity")), "");
  assert.equal(scopeOf(join(tmpdir(), "scratch")), "");
  assert.equal(scopeOf("/private/tmp/anything"), "");
  assert.equal(scopeOf(""), "");
});

test("stampCwd picks the field by direction and ignores other tools", () => {
  const base = { cwd: "/repo", sessionId: "s1" };
  assert.deepEqual(
    stampCwd({ ...base, toolName: "mcp__plugin_linggen_ling-mem__memory_add", params: { content: "x" } }),
    { content: "x", cwd: "/repo" },
  );
  assert.deepEqual(
    stampCwd({ ...base, toolName: "ling-mem__memory_search", params: { query: "q" } }),
    { query: "q", cwd_scope: "/repo" },
  );
  assert.equal(stampCwd({ ...base, toolName: "ling-mem__memory_delete", params: { id: "a" } }), null);
});

test("stampCwd never overwrites, and never rescopes another session's row", () => {
  const base = { cwd: "/repo", sessionId: "s1" };
  // A promote pass carries the ORIGINAL row's origin; the dream knows where a
  // memory came from and this hook does not.
  assert.equal(stampCwd({ ...base, toolName: "memory_add", params: { cwd: "/elsewhere" } }), null);
  assert.equal(
    stampCwd({ ...base, toolName: "memory_add", params: { content: "x", source_session: "other" } }),
    null,
  );
  // This session's own write still gets stamped.
  assert.deepEqual(
    stampCwd({ ...base, toolName: "memory_add", params: { content: "x", source_session: "s1" } }),
    { content: "x", source_session: "s1", cwd: "/repo" },
  );
});

test("stampCwd refuses a non-project cwd rather than hiding the row", () => {
  assert.equal(
    stampCwd({ toolName: "memory_add", params: { content: "x" }, cwd: homedir(), sessionId: "s1" }),
    null,
  );
});

test("MCP config is additive once and idempotent after", () => {
  const draft = {};
  assert.deepEqual(configureLinggenMcp(draft, CLIENT), ["linggen", "ling-mem"]);
  assert.equal(draft.mcp.servers.linggen.url, "http://127.0.0.1:9527/mcp");
  assert.equal(draft.mcp.servers["ling-mem"].transport, "streamable-http");
  assert.deepEqual(configureLinggenMcp(draft, CLIENT), []);
});

test("MCP config leaves a user's own entry untouched", () => {
  const draft = { mcp: { servers: { "ling-mem": { url: "http://192.168.1.9:9528/mcp", transport: "sse" } } } };
  assert.deepEqual(configureLinggenMcp(draft, CLIENT), ["linggen"]);
  assert.equal(draft.mcp.servers["ling-mem"].url, "http://192.168.1.9:9528/mcp");
  assert.equal(draft.mcp.servers["ling-mem"].transport, "sse");
});

test("a remote ling-mem carries the device token as a header", () => {
  const draft = {};
  configureLinggenMcp(draft, {
    ...CLIENT,
    lingMemUrl: "http://192.168.1.9:9528/mcp",
    lingMemLocal: false,
    token: "abc123",
  });
  assert.deepEqual(draft.mcp.servers["ling-mem"].headers, { "x-linggen-device": "abc123" });
  // Loopback needs no token, so a normal single-machine install sets no header.
  const local = {};
  configureLinggenMcp(local, { ...CLIENT, token: "abc123" });
  assert.equal(local.mcp.servers["ling-mem"].headers, undefined);
});

test("renderBody substitutes both placeholders", () => {
  assert.equal(
    renderBody("scan $ARGUMENTS via ${CLAUDE_PLUGIN_ROOT}/scripts", { args: "2026-08-14", pluginRoot: "/p" }),
    "scan 2026-08-14 via /p/scripts",
  );
});
