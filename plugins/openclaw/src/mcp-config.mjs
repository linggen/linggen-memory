// Wire the two Linggen MCP servers into the host's own `openclaw.json`.
//
// A native OpenClaw plugin cannot ship MCP servers — there is no registration
// API and no manifest field, and a directory holding `openclaw.plugin.json`
// alongside bundle markers is loaded as native with the bundle side ignored. So
// the servers are written into the user's config, once, the same two addresses
// the Claude Code plugin declares in `.mcp.json`.
//
// Each tool is served in exactly one place: memory comes from ling-mem, browser
// and X and agent tools from the engine. The engine does not proxy memory.
//
// Idempotent and non-destructive: an id already present is left exactly as it
// is. A user who moved a daemon, added headers, or disabled a server keeps their
// edit — this only ever fills a gap.

export const SERVER_IDS = { engine: "linggen", memory: "ling-mem" };

/**
 * Mutate an `openclaw.json` draft in place. Returns the ids actually added, so
 * the caller can log precisely what it changed rather than claiming credit for
 * config the user already had.
 */
export function configureLinggenMcp(draft, client) {
  const wanted = {
    [SERVER_IDS.engine]: { url: `${client.linggenUrl}/mcp` },
    [SERVER_IDS.memory]: { url: client.lingMemUrl },
  };

  const mcp = draft.mcp && typeof draft.mcp === "object" ? draft.mcp : {};
  const servers = mcp.servers && typeof mcp.servers === "object" ? mcp.servers : {};
  const added = [];

  for (const [id, spec] of Object.entries(wanted)) {
    if (servers[id]) continue;
    const entry = { url: spec.url, transport: "streamable-http", enabled: true };
    // Off-machine, ling-mem's LAN gate wants a paired device's token; loopback
    // needs none, so a normal single-machine install sets no header.
    if (id === SERVER_IDS.memory && client.token && !client.lingMemLocal) {
      entry.headers = { "x-linggen-device": client.token };
    }
    servers[id] = entry;
    added.push(id);
  }

  mcp.servers = servers;
  draft.mcp = mcp;
  return added;
}

/**
 * Add the servers if either is missing. Returns the ids added ([] when the
 * config already had both, which is the steady state after the first run).
 */
export async function ensureMcpServers({ api, client, logger }) {
  // Look before writing. `mutateConfigFile` rewrites the file even when the
  // mutation changes nothing, and rewriting a user's config on every gateway
  // start — churning its hash, its backup, and its audit trail — is not
  // nothing. After the first run this returns immediately.
  try {
    const current = api.runtime.config.current?.();
    const servers = current?.mcp?.servers ?? {};
    if (Object.values(SERVER_IDS).every((id) => servers[id])) return [];
  } catch {
    // Unreadable current config: fall through and let the mutation decide.
  }

  let added = [];
  await api.runtime.config.mutateConfigFile({
    afterWrite: { mode: "auto" },
    mutate: (draft) => {
      added = configureLinggenMcp(draft, client);
    },
  });
  if (added.length) {
    logger?.info?.(
      `linggen: added MCP server${added.length > 1 ? "s" : ""} ${added.join(", ")} to openclaw.json — restart the gateway to connect`,
    );
  }
  return added;
}
