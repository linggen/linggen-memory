// Linggen API client for the memory dashboard.
const API_BASE = '';

export async function fetchModels() {
  const res = await fetch(`${API_BASE}/api/models`);
  if (!res.ok) throw new Error('Failed to fetch models');
  return res.json();
}

export async function fetchDefaultModel() {
  const res = await fetch(`${API_BASE}/api/config`);
  if (!res.ok) return null;
  const config = await res.json();
  const defaults = config.routing?.default_models;
  return defaults && defaults.length > 0 ? defaults[0] : null;
}

export async function createSession(title, skill) {
  const res = await fetch(`${API_BASE}/api/sessions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ title, skill }),
  });
  if (!res.ok) throw new Error(`Failed to create session: ${res.status}`);
  return res.json();
}

export async function listSkillSessions(skill) {
  const res = await fetch(`${API_BASE}/api/skill-sessions?skill=${encodeURIComponent(skill)}`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.sessions || [];
}

export async function removeSkillSession(skill, sessionId) {
  const res = await fetch(`${API_BASE}/api/skill-sessions`, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ skill, session_id: sessionId }),
  });
  if (!res.ok) throw new Error('Failed to delete session');
}

export async function fetchSessionMessages(skill, sessionId) {
  const params = new URLSearchParams({ skill, session_id: sessionId });
  const res = await fetch(`${API_BASE}/api/skill-sessions/state?${params}`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.messages || [];
}

// ── ling-mem daemon proxy ──
// Cross-origin to 127.0.0.1:9888 isn't allowed from this iframe; route
// through Linggen's /api/bash so the JS can call the daemon without
// CORS headache. Returns {count, latest_created_at} per the daemon's
// count endpoint contract.
export async function fetchMemoryCount(filter = {}) {
  const body = JSON.stringify(filter);
  const cmd = `curl -s -X POST http://127.0.0.1:9888/api/memory/count -H 'Content-Type: application/json' -d ${JSON.stringify(body)}`;
  const res = await fetch(`${API_BASE}/api/bash`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ command: cmd }),
  });
  if (!res.ok) return { count: 0, latest_created_at: null };
  const out = await res.json();
  try {
    const parsed = JSON.parse(out.stdout || '{}');
    if (parsed.ok && parsed.data) return parsed.data;
    return { count: 0, latest_created_at: null };
  } catch {
    return { count: 0, latest_created_at: null };
  }
}

// Expand `~` to $HOME on the shell side, since /api/bash receives a
// command string (not pre-expanded). All readers below feed the path
// through `eval echo` so `~` and `$HOME` both work transparently.
function expandPath(path) {
  // Build a shell snippet that prints the expanded path to a holding
  // variable. JSON.stringify quotes path safely — `~` expands because
  // it's at the start of a word, and $HOME expands inside double
  // quotes as usual.
  return `$(eval echo ${JSON.stringify(path)})`;
}

// Read a small JSON file via /api/bash. Missing file → fallback.
// Used for ~/.linggen/memory/.dream-state.json and the .scan-output.jsonl
// header line.
export async function readJsonFile(path, fallback = null) {
  const p = expandPath(path);
  const cmd = `f=${p}; [ -f "$f" ] && cat "$f" || true`;
  const res = await fetch(`${API_BASE}/api/bash`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ command: cmd }),
  });
  if (!res.ok) return fallback;
  const out = await res.json();
  const txt = (out.stdout || '').trim();
  if (!txt) return fallback;
  try { return JSON.parse(txt); } catch { return fallback; }
}

// Write a JSON file via /api/bash. Creates parent dirs. Best-effort —
// quota-equivalent (e.g. disk-full) failures surface as a rejected
// promise so callers can `.catch(() => {})` and move on.
export async function writeJsonFile(path, value) {
  const p = `$(eval echo ${JSON.stringify(path)})`;
  const json = JSON.stringify(value);
  // Base64 the payload to keep arbitrary content (quotes, backticks,
  // newlines) out of the shell command surface.
  const b64 = btoa(unescape(encodeURIComponent(json)));
  const cmd = `f=${p}; mkdir -p "$(dirname "$f")" && printf '%s' ${JSON.stringify(b64)} | base64 --decode > "$f"`;
  const res = await fetch(`${API_BASE}/api/bash`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ command: cmd }),
  });
  if (!res.ok) throw new Error(`writeJsonFile ${path}: ${res.status}`);
}

// Read the first line (the header object) of an NDJSON file.
export async function readJsonlHeader(path, fallback = null) {
  const p = expandPath(path);
  const cmd = `f=${p}; [ -f "$f" ] && head -n 1 "$f" || true`;
  const res = await fetch(`${API_BASE}/api/bash`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ command: cmd }),
  });
  if (!res.ok) return fallback;
  const out = await res.json();
  const txt = (out.stdout || '').trim();
  if (!txt) return fallback;
  try { return JSON.parse(txt); } catch { return fallback; }
}
