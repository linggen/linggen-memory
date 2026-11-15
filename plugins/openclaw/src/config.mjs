// Where this host goes to find Linggen.
//
// Port of the address half of `plugins/linggen/hooks/mcp.sh`. Kept apart from
// the transport half (`rpc.mjs`) so that reading configuration and sending a
// request are never the same file — which is both cleaner and what OpenClaw's
// install scanner wants to see.
//
// **This is the CLIENT declaration.** It says where to *connect*, which on a
// second machine is somewhere else entirely — and it is deliberately NOT the
// engine's `[server].url`. That file says where the engine on THAT machine
// binds; reading it to find an engine on another host is how the two facts get
// confused. A machine with only the plugin still needs an address.
//
// One address: where Linggen lives (host + port) and, for another machine,
// that machine's token. Memory is `<address>/mcp/memory`, the engine's tools
// `<address>/mcp`.
//
// Authored in ONE file, `~/.linggen/client.json`, written by
// `scripts/config.sh` (`/linggen:config`):
//
//   { "ling": "http://…:9527", "token": "…", "device_id": "…" }
//
// Precedence is file > env (LINGGEN_HOST, LINGGEN_PORT, LINGGEN_TOKEN) >
// default 127.0.0.1:9527 — the same rule as `hooks/mcp.sh`. An older file or
// environment that names the retired memory daemon's address (`ling_mem`,
// the old memory env LING_MEM_HOST / LING_MEM_TOKEN) is read as this address where that is
// unambiguous; see `pick`.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const ENV = process.env;

const DEFAULT_HOST = "127.0.0.1";
const DEFAULT_PORT = "9527";

/** The client config object, or {}. Absent file and bad JSON are both
 *  "nothing configured" — this runs in a hook that must never fail. */
function clientFile(env) {
  const dir = env.LINGGEN_DATA_DIR || join(homedir(), ".linggen");
  try {
    const parsed = JSON.parse(readFileSync(join(dir, "client.json"), "utf8"));
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

const str = (value) => (typeof value === "string" ? value : "");

/** `http://host:port` → host / port, tolerating a missing scheme or path. */
function hostPort(url) {
  let rest = url.includes("://") ? url.slice(url.indexOf("://") + 3) : url;
  rest = rest.split("/")[0] ?? "";
  const colon = rest.lastIndexOf(":");
  return colon < 0 ? { host: rest, port: "" } : { host: rest.slice(0, colon), port: rest.slice(colon + 1) };
}

/** Is Linggen on THIS machine? Installing or starting it — or opening the
 *  store with `ling mem` — only makes sense if it is: pointed at another
 *  machine, a local Linggen would hold a DIFFERENT store and silently fork the
 *  user's memory in two — the one outcome this arrangement exists to prevent. */
export function isLocal(host) {
  return host === "127.0.0.1" || host === "localhost" || host === "::1" || host === "";
}

/**
 * The one address out of the engine address and an old memory address. The
 * old one meant "the machine whose memory this host uses"; that machine's
 * Linggen now serves memory on its engine port (9527).
 *   only one given            → that one (the old one on port 9527)
 *   both on the same host     → the engine address
 *   engine local, memory not  → the memory machine (a local Linggen would
 *                               open a second, empty store)
 *   two different remotes     → the engine address (ambiguous)
 */
export function pick(ling, old) {
  if (!old) return ling;
  const oldHost = hostPort(old).host || DEFAULT_HOST;
  const moved = `http://${oldHost}:${DEFAULT_PORT}`;
  if (!ling) return moved;
  const lingHost = hostPort(ling).host;
  if (lingHost === oldHost) return ling;
  if (isLocal(lingHost) && !isLocal(oldHost)) return moved;
  return ling;
}

/**
 * Read the client declaration. Re-read per call rather than cached at import:
 * the shell hooks re-source `mcp.sh` every invocation, so `/linggen:config`
 * takes effect without restarting the host. The file is a few hundred bytes.
 */
export function resolveClient(env = ENV) {
  const file = clientFile(env);
  let address = pick(str(file.ling), str(file.ling_mem));
  if (!address) {
    const fromEnv =
      env.LINGGEN_HOST || env.LINGGEN_PORT
        ? `http://${env.LINGGEN_HOST || DEFAULT_HOST}:${env.LINGGEN_PORT || DEFAULT_PORT}`
        : "";
    address = pick(fromEnv, env.LING_MEM_HOST ? `http://${env.LING_MEM_HOST}` : "");
  }
  const { host, port } = hostPort(address);
  const linggenUrl = `http://${host || DEFAULT_HOST}:${port || DEFAULT_PORT}`;
  // Off-machine, the engine wants an approved machine's token; loopback needs
  // none, so a normal single-machine install sets nothing.
  const token = str(file.token) || env.LINGGEN_TOKEN || env.LING_MEM_TOKEN || ""; // last: the old memory token
  return {
    // No `/mcp` on the base URL — it is also the base for `/api/health`.
    linggenUrl,
    memoryUrl: `${linggenUrl}/mcp/memory`,
    local: isLocal(host || DEFAULT_HOST),
    token,
  };
}

/** Read-only knobs, resolved in one place so no request-building code reads the
 *  environment itself. */
export function readSettings(env = ENV) {
  return {
    recallDisabled: env.LINGGEN_RECALL_DISABLE === "1",
    stampDisabled: env.LINGGEN_STAMP_CWD_DISABLE === "1",
    topK: Number(env.LINGGEN_RECALL_TOPK ?? 3),
    limit: Number(env.LINGGEN_RECALL_LIMIT ?? 8),
    recallTimeoutMs: Number(env.LINGGEN_RECALL_TIMEOUT ?? 3) * 1000,
    coreTimeoutMs: Number(env.LINGGEN_CORE_TIMEOUT ?? 5) * 1000,
    minScore: Number(env.LINGGEN_RECALL_MIN_SCORE ?? 0),
    upkeepCacheMinutes: Number(env.LINGGEN_UPKEEP_CACHE_MIN ?? 30),
    // Recall on delegate: =0 off; REWRITE=0 tells the parent only.
    delegateDisabled: env.LINGGEN_DELEGATE === "0",
    delegateRewrite: env.LINGGEN_DELEGATE_REWRITE !== "0",
    delegateLimit: Number(env.LINGGEN_DELEGATE_LIMIT ?? 3),
  };
}
