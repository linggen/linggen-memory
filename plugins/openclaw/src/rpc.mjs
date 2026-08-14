// How this host calls a tool on the Linggen daemons.
//
// Port of the transport half of `plugins/linggen/hooks/mcp.sh`. Deliberately
// reads no files and no environment: it is handed a resolved client by
// `config.mjs` and does nothing but speak JSON-RPC.
//
// ling-mem answers plain JSON-RPC 2.0 over HTTP POST — no handshake, no SSE, no
// session id — which is why a hook can call a tool with nothing but a URL and
// (off-machine) a device token, and needs no `ling-mem` binary at all.

/**
 * One MCP tool call against ling-mem. Returns the tool's parsed payload — MCP
 * double-encodes it, as a JSON string inside a text content block, so this
 * unwraps both layers.
 *
 * `null` on ANY failure: unreachable daemon, gate refusal, malformed reply.
 * Every caller is a hook that must never block a turn, so a failure here is
 * silence, not an error.
 */
export async function mcpCall(client, name, args, timeoutMs = 3000) {
  const headers = { "Content-Type": "application/json" };
  if (client?.token) headers["x-linggen-device"] = client.token;
  try {
    const response = await fetch(client.lingMemUrl, {
      method: "POST",
      headers,
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: { name, arguments: args },
      }),
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!response.ok) return null;
    const envelope = await response.json();
    const text = envelope?.result?.content?.[0]?.text;
    if (typeof text !== "string" || text === "") return null;
    return JSON.parse(text);
  } catch {
    return null;
  }
}

/** Is a daemon answering at this base URL? */
export async function alive(baseUrl, timeoutMs = 2000) {
  try {
    const response = await fetch(`${baseUrl}/api/health`, { signal: AbortSignal.timeout(timeoutMs) });
    return response.ok;
  } catch {
    return false;
  }
}
