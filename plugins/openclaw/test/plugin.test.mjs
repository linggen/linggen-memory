// Runs on `node --test` with no dependencies to install — the plugin itself is
// Node-builtins-only, and its tests should not be the thing that drags a
// toolchain into a Rust repo.
//
// Covers the pure decisions only: which field a tool gets stamped, which
// directories refuse to become a scope, whether MCP config is left alone the
// second time, how the one address is resolved. The Linggen-facing paths are
// exercised against a scratch engine by scripts/live-check.sh, not here.

import assert from "node:assert/strict";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";

import { renderBody } from "../src/commands.mjs";
import { pick, resolveClient } from "../src/config.mjs";
import { configureLinggenMcp } from "../src/mcp-config.mjs";
import { scopeOf } from "../src/recall.mjs";
import { isProjectDir } from "../src/scope.mjs";
import { stampCwd } from "../src/stamp-cwd.mjs";

const CLIENT = {
  linggenUrl: "http://127.0.0.1:9527",
  memoryUrl: "http://127.0.0.1:9527/mcp/memory",
  local: true,
  token: "",
};

test("scopeOf hands the daemon the session root", () => {
  assert.equal(scopeOf("/nonexistent/workspace/repo"), "/nonexistent/workspace/repo");
  // The daemon reads $HOME / ~/.linggen / temp as person-rows-only; the host
  // passes the path through.
  assert.equal(scopeOf(homedir()), homedir());
  assert.equal(scopeOf(join(homedir(), ".linggen", "skills", "cfo", "data")), join(homedir(), ".linggen", "skills", "cfo"));
  assert.equal(scopeOf(""), "");
});

test("isProjectDir refuses $HOME, ~/.linggen and temp, admits a skill's dir", () => {
  assert.equal(isProjectDir("/nonexistent/repo"), true);
  assert.equal(isProjectDir(homedir()), false);
  assert.equal(isProjectDir(join(homedir(), ".linggen")), false);
  assert.equal(isProjectDir(join(homedir(), ".linggen", "activity")), false);
  assert.equal(isProjectDir(join(homedir(), ".linggen", "skills", "dj")), true);
  assert.equal(isProjectDir(join(tmpdir(), "scratch")), false);
  assert.equal(isProjectDir(""), false);
});

test("stampCwd stamps by verb and ignores other tools", () => {
  const base = { cwd: "/nonexistent/repo", sessionId: "s1" };
  assert.deepEqual(
    stampCwd({ ...base, toolName: "mcp__plugin_linggen_memory__memory_add", params: { content: "x", scope: "repo/sub" } }),
    {
      content: "x",
      scope: "repo/sub",
      source_session: "s1",
      cwd: "/nonexistent/repo",
      root: "/nonexistent/repo",
      host: "openclaw",
    },
  );
  assert.deepEqual(
    stampCwd({ ...base, toolName: "memory__memory_search", params: { query: "q" } }),
    { query: "q", scope_root: "/nonexistent/repo" },
  );
  assert.deepEqual(
    stampCwd({ ...base, toolName: "memory__memory_session_start", params: {} }),
    { cwd: "/nonexistent/repo", root: "/nonexistent/repo" },
  );
  assert.equal(stampCwd({ ...base, toolName: "memory__memory_delete", params: { id: "a" } }), null);
});

test("stampCwd never overwrites, and never rescopes another session's row", () => {
  const base = { cwd: "/nonexistent/repo", sessionId: "s1" };
  // A promote pass carries the ORIGINAL row's origin; the dream knows where a
  // memory came from and this hook does not.
  assert.equal(
    stampCwd({ ...base, toolName: "memory_add", params: { cwd: "/elsewhere", source_session: "old", host: "linggen" } }),
    null,
  );
  assert.equal(
    stampCwd({ ...base, toolName: "memory_add", params: { content: "x", source_session: "other", host: "codex" } }),
    null,
  );
});

test("a model's own search at home stays a whole-store lookup", () => {
  assert.equal(stampCwd({ toolName: "memory_search", params: { query: "q" }, cwd: homedir(), sessionId: "s1" }), null);
});

test("MCP config is additive once and idempotent after", () => {
  const draft = {};
  assert.deepEqual(configureLinggenMcp(draft, CLIENT), ["linggen", "memory"]);
  assert.equal(draft.mcp.servers.linggen.url, "http://127.0.0.1:9527/mcp");
  assert.equal(draft.mcp.servers.memory.url, "http://127.0.0.1:9527/mcp/memory");
  assert.equal(draft.mcp.servers.memory.transport, "streamable-http");
  assert.deepEqual(configureLinggenMcp(draft, CLIENT), []);
});

test("MCP config leaves a user's own entry untouched", () => {
  const draft = { mcp: { servers: { memory: { url: "http://192.168.1.9:9527/mcp/memory", transport: "sse" } } } };
  assert.deepEqual(configureLinggenMcp(draft, CLIENT), ["linggen"]);
  assert.equal(draft.mcp.servers.memory.url, "http://192.168.1.9:9527/mcp/memory");
  assert.equal(draft.mcp.servers.memory.transport, "sse");
});

test("another machine's Linggen: both servers carry its token", () => {
  const draft = {};
  configureLinggenMcp(draft, {
    linggenUrl: "http://192.168.1.9:9527",
    memoryUrl: "http://192.168.1.9:9527/mcp/memory",
    local: false,
    token: "abc123",
  });
  assert.deepEqual(draft.mcp.servers.memory.headers, { "x-linggen-device": "abc123" });
  assert.deepEqual(draft.mcp.servers.linggen.headers, { "x-linggen-device": "abc123" });
  // Loopback needs no token, so a normal single-machine install sets no header.
  const local = {};
  configureLinggenMcp(local, { ...CLIENT, token: "abc123" });
  assert.equal(local.mcp.servers.memory.headers, undefined);
});

test("one address: file > env > default, memory at /mcp/memory", () => {
  const dir = mkdtempSync(join(tmpdir(), "linggen-client-"));
  try {
    assert.deepEqual(resolveClient({ LINGGEN_DATA_DIR: dir }), CLIENT);
    assert.equal(resolveClient({ LINGGEN_DATA_DIR: dir, LINGGEN_PORT: "47611" }).memoryUrl, "http://127.0.0.1:47611/mcp/memory");
    writeFileSync(join(dir, "client.json"), JSON.stringify({ ling: "http://192.168.1.9:9527", token: "t1" }));
    const remote = resolveClient({ LINGGEN_DATA_DIR: dir, LINGGEN_PORT: "47611" });
    assert.deepEqual(remote, {
      linggenUrl: "http://192.168.1.9:9527",
      memoryUrl: "http://192.168.1.9:9527/mcp/memory",
      local: false,
      token: "t1",
    });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("an old memory address is read as the one address where unambiguous", () => {
  assert.equal(pick("http://127.0.0.1:9527", ""), "http://127.0.0.1:9527");
  assert.equal(pick("", "http://192.168.1.9:9528"), "http://192.168.1.9:9527");
  assert.equal(pick("http://192.168.1.9:9527", "http://192.168.1.9:9528"), "http://192.168.1.9:9527");
  // Memory came from that machine: never fall back to a local, empty store.
  assert.equal(pick("http://127.0.0.1:9527", "http://192.168.1.9:9528"), "http://192.168.1.9:9527");
  // Two different machines: the engine address stands.
  assert.equal(pick("http://192.168.1.8:9527", "http://192.168.1.9:9528"), "http://192.168.1.8:9527");
  const legacyEnv = resolveClient({ LINGGEN_DATA_DIR: "/nonexistent", LING_MEM_HOST: "192.168.1.9", LING_MEM_TOKEN: "t2" });
  assert.equal(legacyEnv.memoryUrl, "http://192.168.1.9:9527/mcp/memory");
  assert.equal(legacyEnv.token, "t2");
});

test("renderBody substitutes both placeholders", () => {
  assert.equal(
    renderBody("scan $ARGUMENTS via ${CLAUDE_PLUGIN_ROOT}/scripts", { args: "2026-08-14", pluginRoot: "/p" }),
    "scan 2026-08-14 via /p/scripts",
  );
});
