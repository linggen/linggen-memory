// Where this host goes to find Linggen.
//
// Port of the address half of `plugins/linggen/hooks/mcp.sh`. Kept apart from
// the transport half (`rpc.mjs`) so that reading configuration and sending a
// request are never the same file — which is both cleaner and what OpenClaw's
// install scanner wants to see.
//
// **This is the CLIENT declaration.** It says where to *connect*, which on a
// second machine is somewhere else entirely — and it is deliberately NOT the
// engine's `[server].url`. That file says where the daemon on THAT machine
// binds; reading it to find a daemon on another host is how the two facts get
// confused. A client that has no local daemon still needs an address.
//
// Authored in ONE file, `~/.linggen/client.json`, written by
// `scripts/config.sh` (`/linggen:config`):
//
//   { "ling": "http://…:9527", "ling_mem": "http://…:9528", "token": "…" }
//
// Precedence is env > file > default.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const ENV = process.env;

const DEFAULT_LING_HOST = "127.0.0.1";
const DEFAULT_LING_PORT = "9527";
const DEFAULT_LING_MEM_HOST = "127.0.0.1";
const DEFAULT_LING_MEM_PORT = "9528";

/** One key out of the client config, or "". Absent file, bad JSON and missing
 *  key are all "nothing configured" — this runs in a hook that must never fail. */
function clientGet(env, key) {
  const dir = env.LINGGEN_DATA_DIR || join(homedir(), ".linggen");
  try {
    const parsed = JSON.parse(readFileSync(join(dir, "client.json"), "utf8"));
    const value = parsed?.[key];
    return typeof value === "string" ? value : "";
  } catch {
    return "";
  }
}

/** Split `http://host:port` into host and port, tolerating a missing scheme. */
function hostOf(url) {
  const rest = url.includes("://") ? url.slice(url.indexOf("://") + 3) : url;
  return rest.split(":")[0] ?? "";
}

function portOf(url) {
  const rest = url.includes("://") ? url.slice(url.indexOf("://") + 3) : url;
  const afterColon = rest.includes(":") ? rest.slice(rest.lastIndexOf(":") + 1) : "";
  return afterColon.split("/")[0] ?? "";
}

function resolveOne(hostEnv, portEnv, fromFile, defaultHost, defaultPort) {
  let host = hostEnv || "";
  let port = portEnv || "";
  if (!host && fromFile) host = hostOf(fromFile);
  if (!port && fromFile) port = portOf(fromFile);
  return { host: host || defaultHost, port: port || defaultPort };
}

/** Is this daemon on THIS machine? Installing a binary or starting a daemon only
 *  makes sense if it is: pointed at another host, a local daemon on the same port
 *  would answer from a DIFFERENT store and silently fork the user's memory in two
 *  — the one outcome this whole arrangement exists to prevent. */
function isLocal(host) {
  return host === "127.0.0.1" || host === "localhost" || host === "::1" || host === "";
}

/**
 * Read the client declaration. Re-read per call rather than cached at import:
 * the shell hooks re-source `mcp.sh` every invocation, so `/linggen:config`
 * takes effect without restarting the host. The file is a few hundred bytes.
 */
export function resolveClient(env = ENV) {
  const ling = resolveOne(
    env.LINGGEN_HOST,
    env.LINGGEN_PORT,
    clientGet(env, "ling"),
    DEFAULT_LING_HOST,
    DEFAULT_LING_PORT,
  );
  const lingMem = resolveOne(
    env.LING_MEM_HOST,
    env.LING_MEM_PORT,
    clientGet(env, "ling_mem"),
    DEFAULT_LING_MEM_HOST,
    DEFAULT_LING_MEM_PORT,
  );
  // Off-machine, ling-mem's LAN gate wants a paired device's token; loopback
  // needs none, so a normal single-machine install sets nothing.
  const token = env.LING_MEM_TOKEN || clientGet(env, "token");
  return {
    // No `/mcp` on the engine URL — it is also the base for `/api/health`.
    linggenUrl: `http://${ling.host}:${ling.port}`,
    lingMemUrl: `http://${lingMem.host}:${lingMem.port}/mcp`,
    linggenLocal: isLocal(ling.host),
    lingMemLocal: isLocal(lingMem.host),
    token,
  };
}

/** Read-only knobs, resolved in one place so no request-building code reads the
 *  environment itself. */
export function readSettings(env = ENV) {
  return {
    recallDisabled: env.LING_MEM_RECALL_DISABLE === "1",
    stampDisabled: env.LING_MEM_STAMP_CWD_DISABLE === "1",
    topK: Number(env.LING_MEM_RECALL_TOPK ?? 3),
    limit: Number(env.LING_MEM_RECALL_LIMIT ?? 8),
    recallTimeoutMs: Number(env.LING_MEM_RECALL_TIMEOUT ?? 3) * 1000,
    coreTimeoutMs: Number(env.LING_MEM_CORE_TIMEOUT ?? 5) * 1000,
    minScore: Number(env.LING_MEM_RECALL_MIN_SCORE ?? 0),
    upkeepCacheMinutes: Number(env.LING_MEM_UPKEEP_CACHE_MIN ?? 30),
  };
}
