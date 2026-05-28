// Memory App — thin client.
//
// JS responsibilities:
//   1. Mount the sessions iframe (sidebar) and chat panel.
//   2. On open, paint the dashboard deterministically from disk + the
//      ling-mem daemon's count endpoint. No LLM round-trip required.
//      The agent stays passive until the user clicks Scan / Hippocampus
//      or types a chat question.
//   3. Tier-count cards live in top_bar so they persist across any
//      PageUpdate the agent emits later (action reports replace body
//      only).
//   4. Render PageUpdate events from the agent (action reports).
//   5. Forward widget button clicks as plain user messages.
//
// The agent's BOOT_PROMPT is intentionally tiny: JS owns the on-open
// render, so the agent's only job at boot is to be ready in the chat
// panel. Everything heavy moves to the click-driven scan / hippocampus
// flows (SKILL.md slash commands).

import {
  fetchDefaultModel,
  fetchMemoryCount,
  readJsonFile,
  readJsonlHeader,
  writeJsonFile,
} from './api.js';
import { applyPageUpdate, parsePageBlock, getCurrentPage, restorePage } from './page-renderer.js';

const SKILL_NAME = 'ling-mem';

// Tiny boot prompt — the agent waits for user input. JS already drew
// the dashboard before this lands.
const BOOT_PROMPT = `You are Ling inside the memory skill. The dashboard is already painted on the user's screen — the tier counts, greeting, and CTA buttons came from JS reading the ling-mem daemon directly. Don't re-fetch them.

Stay silent until the user clicks an action button or types a message. When they do:
- "Scan ..." (today / week / month) → run \`Bash bash ~/.linggen/skills/shared-memory/scripts/scan.sh <window>\`, then summarize the one-line stdout (sessions found / scanned / candidates) back in chat. Don't write to memory yet — scan is read-only.
- "/shared-memory dream" or "Run hippocampus" → follow references/dream-flow.md (read .scan-output.jsonl, judge, write, consolidate, evict). Emit a final PageUpdate with the report.
- Anything else → answer normally, use Memory_query when relevant.

Do not emit a PageUpdate on this boot — JS already rendered. Don't repeat the greeting visible on screen.`;

const params = new URLSearchParams(window.location.search);
let modelId = params.get('model') || '';
let chat = null;

// Sessions iframe bridge (mirrors pulse-app.js setupSessionsIframe).
// Sidebar is Linggen's BareSessions React component loaded via
// `<iframe src="/sessions?skill=ling-mem&active=<sid>">`. We only point
// the iframe and listen for select / create postMessages.
function setupSessionsIframe(activeSid) {
  const ifr = document.getElementById('sessions-iframe');
  if (!ifr) return;
  const sp = new URLSearchParams({ skill: SKILL_NAME });
  if (activeSid) sp.set('active', activeSid);
  ifr.src = `/sessions?${sp.toString()}`;
}

function handleSessionsMessage(e) {
  if (e.data?.type !== 'linggen-skill-event') return;
  const sid = e.data.payload?.sessionId;
  if (!sid) return;
  if (e.data.event === 'session_select' || e.data.event === 'session_create') {
    // Memory's dashboard greeting runs per session; switch by URL so
    // chat-bridge re-mounts on the new session.
    const url = new URL(window.location.href);
    url.searchParams.set('session', sid);
    if (url.toString() !== window.location.href) {
      window.location.href = url.toString();
    }
  }
}

document.addEventListener('DOMContentLoaded', async () => {
  if (!modelId) {
    try {
      const defaultModel = await fetchDefaultModel();
      modelId = localStorage.getItem('memory:model') || defaultModel || '';
    } catch { /* ignore */ }
  }
  const existingSession = params.get('session') || '';
  setupSessionsIframe(existingSession || null);
  window.addEventListener('message', handleSessionsMessage);
  await mountAndStart(existingSession || null);
});

async function mountAndStart(sessionId) {
  const chatPanel = document.getElementById('chat-panel');
  const mountOpts = {
    skillName: SKILL_NAME,
    agentId: 'ling-mem',
    modelId,
    title: 'Memory',
    onSessionCreated: (sid) => {
      const url = new URL(window.location);
      url.searchParams.set('session', sid);
      history.replaceState(null, '', url);
    },
    onStreamEnd: handleLegacyPageBlock,
    onContentBlock: handleContentBlock,
  };
  if (sessionId) mountOpts.sessionId = sessionId;
  chat = await LinggenUI.mount(chatPanel, mountOpts);

  // Widget buttons send plain text — the agent reads the message and
  // decides what to do based on SKILL.md. JS knows nothing about ranges,
  // scans, or extraction.
  window._chatSend = (text) => { if (chat) chat.send(text); };
  setupActionBar();

  if (sessionId && await tryRestoreCached(sessionId)) {
    // Resumed session with a cached page — don't re-boot the agent.
    // The cached page comes back via tryRestoreCached; tier counts
    // refresh below so the user sees current numbers even on resume.
    refreshTierCounts().catch(() => {});
    return;
  }

  // JS-driven first paint. No LLM round-trip. The agent's boot prompt
  // runs in the background and never touches top_bar / body — JS owns
  // both until the user clicks an action.
  await paintDashboard();

  // WebRTC data-channel setup runs AFTER the iframe's `load` event;
  // sending the boot prompt before the channel is up gets silently
  // dropped. 1.5s matches the working flow used historically.
  setTimeout(() => {
    if (chat) chat.sendHidden(BOOT_PROMPT);
  }, 1500);
}

// ── On-open dashboard ──
//
// Reads three count endpoints + .dream-state.json in parallel, then
// paints a deterministic dashboard. The greeting line + primary CTA
// are rule-picked from state — no LLM required for the first render.
// Tier counts go to top_bar so the agent's later action-result
// PageUpdates (body-only) don't blow them away.

async function paintDashboard() {
  let coreC, semC, epC, dream;
  try {
    [coreC, semC, epC, dream] = await Promise.all([
      fetchMemoryCount({ tier: 'core' }),
      fetchMemoryCount({ tier: 'semantic' }),
      fetchMemoryCount({ episodic: true }),
      readJsonFile(`${homeDir()}/.linggen/memory/.dream-state.json`),
    ]);
  } catch (e) {
    console.warn('[memory] paintDashboard fetch failed', e);
    coreC = { count: 0 };
    semC = { count: 0 };
    epC = { count: 0 };
    dream = null;
  }
  const scanHdr = await readJsonlHeader(`${homeDir()}/.linggen/memory/.scan-output.jsonl`).catch(() => null);
  const summary = { coreC, semC, epC, dream, scanHdr };
  const greeting = pickGreeting(summary);

  applyPageUpdate({
    top_bar: buildTierCards(summary),
    body: [greeting],
    footer: buildFooter(summary),
  });
  cacheCurrentPage();
}

async function refreshTierCounts() {
  const [coreC, semC, epC] = await Promise.all([
    fetchMemoryCount({ tier: 'core' }),
    fetchMemoryCount({ tier: 'semantic' }),
    fetchMemoryCount({ episodic: true }),
  ]);
  // Top-bar only — body stays as whatever the agent (or cache) put there.
  applyPageUpdate({
    top_bar: buildTierCards({ coreC, semC, epC }),
  });
}

function homeDir() {
  // The skill's cwd is `~/.linggen` per SKILL.md, and the /api/bash
  // proxy expands `~` to $HOME. Just hard-code the literal here so
  // command strings stay readable.
  return '~';
}

function buildTierCards({ coreC, semC, epC }) {
  return [
    cardWidget('CORE', coreC, null),
    cardWidget('SEMANTIC', semC, null),
    cardWidget('EPISODIC', epC, epC?.count > 50 ? 'amber' : null),
  ];
}

function cardWidget(label, c, alertColor) {
  const value = (c && typeof c.count === 'number') ? c.count : '—';
  const sub = c?.latest_created_at ? `latest ${ageOf(c.latest_created_at)}` : '';
  return { data: { label, value, sub, color: alertColor } };
}

function buildFooter({ dream, scanHdr }) {
  const parts = [];
  if (scanHdr?.finished_at) {
    parts.push(`scan ${ageOf(scanHdr.finished_at)} · ${scanHdr.sessions_scanned ?? 0} sessions`);
  } else {
    parts.push('scan: never');
  }
  if (dream?.last_run_at) {
    parts.push(`hippocampus ${ageOf(dream.last_run_at)}`);
  } else {
    parts.push('hippocampus: never');
  }
  return { text: parts.join('   ·   ') };
}

function pickGreeting({ coreC, semC, epC, dream, scanHdr }) {
  const totalRows = (coreC?.count || 0) + (semC?.count || 0) + (epC?.count || 0);
  const lastScan = scanHdr?.finished_at;
  const lastHippo = dream?.last_run_at;
  const epCount = epC?.count || 0;
  // "Pending" = the scan happened AFTER the most recent hippocampus
  // (or hippocampus has never run). We only know the scanned-session
  // count; the LLM-judged candidate count isn't materialized anywhere
  // until dream runs. Show sessions-to-review honestly.
  const scanIsNewerThanDream = scanHdr && (!lastHippo
    || new Date(scanHdr.finished_at) > new Date(lastHippo));
  const sessionsToReview = scanIsNewerThanDream
    ? (scanHdr.sessions_scanned || 0)
    : 0;

  let title, primary;
  if (totalRows === 0 && !lastScan) {
    title = "Welcome — your memory's empty. Run Scan to read recent sessions.";
    primary = { label: 'Scan today', icon: '🔍', message: 'Scan today', kind: 'primary' };
  } else if (sessionsToReview > 0) {
    title = `${sessionsToReview} session${sessionsToReview === 1 ? '' : 's'} scanned since last hippocampus. Run hippocampus to judge & consolidate.`;
    primary = { label: 'Run hippocampus', icon: '🧠', message: '/shared-memory dream', kind: 'primary' };
  } else if (lastScan && daysSince(lastScan) >= 1) {
    title = `Last scan ${ageOf(lastScan)}. Pull in newer sessions?`;
    primary = { label: 'Scan today', icon: '🔍', message: 'Scan today', kind: 'primary' };
  } else if (epCount > 50) {
    title = `Staging is filling up — ${epCount} episodic rows. Time for a hippocampus pass.`;
    primary = { label: 'Run hippocampus', icon: '🧠', message: '/shared-memory dream', kind: 'primary' };
  } else {
    title = `Memory's up to date — ${totalRows} rows across all tiers.`;
    primary = { label: 'Browse all ↗', href: 'http://127.0.0.1:9888', kind: 'primary' };
  }

  return {
    type: 'greeting',
    icon: '🧠',
    title,
    stats: 'Scan extracts denoised transcripts (script, free). Hippocampus judges them → memory.',
    actions: [
      primary,
      { label: 'Scan today', icon: '🔍', message: 'Scan today' },
      { label: 'Scan week', message: 'Scan this week' },
      { label: 'Hippocampus', icon: '🧠', message: '/shared-memory dream' },
      { label: 'Browse ↗', href: 'http://127.0.0.1:9888' },
    ],
  };
}

function ageOf(iso) {
  if (!iso) return '';
  const ms = Date.now() - new Date(iso).getTime();
  if (!isFinite(ms) || ms < 0) return '';
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d}d ago`;
  return `${Math.floor(d / 30)}mo ago`;
}

function daysSince(iso) {
  if (!iso) return Infinity;
  return (Date.now() - new Date(iso).getTime()) / 86400000;
}

// ── Top action bar wiring ──
//
// The buttons in memory.html's header send plain chat messages — the
// agent parses them per BOOT_PROMPT and runs the corresponding action.
// Period is on a sibling <select>. After scan / hippocampus, the agent
// emits a PageUpdate with the run report; tier-counts in top_bar
// refresh automatically because the daemon's count endpoint runs after
// every PageUpdate (see handleContentBlock).

function setupActionBar() {
  const scanBtn = document.getElementById('scan-btn');
  const dreamBtn = document.getElementById('dream-btn');
  const periodSel = document.getElementById('scan-period');
  if (!scanBtn || !dreamBtn) return;

  scanBtn.addEventListener('click', () => {
    const p = periodSel?.value || 'today';
    const label = p === 'today' ? 'today' : (p === '7d' ? 'this week' : 'this month');
    window._chatSend(`Scan ${label}`);
  });

  dreamBtn.addEventListener('click', () => {
    window._chatSend('/shared-memory dream');
  });
}

// ── PageUpdate ingestion ──

function handleContentBlock(payload) {
  if (payload?.tool !== 'PageUpdate') return;
  try {
    const args = typeof payload.args === 'string' ? JSON.parse(payload.args) : payload.args;
    if (!args || typeof args !== 'object') return;
    const partial = {};
    if (args.top_bar !== undefined) partial.top_bar = args.top_bar;
    if (args.body !== undefined) partial.body = args.body;
    if (args.footer !== undefined) partial.footer = args.footer;
    if (Object.keys(partial).length === 0) return;
    applyPageUpdate(partial);
    cacheCurrentPage();
    // After any agent-emitted PageUpdate the row totals may have moved
    // (hippocampus writes + promotes + evicts). Re-fetch the counts so
    // the top_bar reflects post-action state. Best-effort; failures stay
    // silent.
    refreshTierCounts().catch(() => {});
  } catch (e) {
    console.warn('[memory] failed to parse PageUpdate args', e);
  }
}

// Fallback: some models emit a <!--page ... --> or ```page block in text
// instead of calling the tool. Parse and apply silently — no nag.
function handleLegacyPageBlock(text) {
  const page = parsePageBlock(text);
  if (!page) return;
  applyPageUpdate(page);
  cacheCurrentPage();
  refreshTierCounts().catch(() => {});
}

// ── Cache ──
//
// Per-session page state lives on disk at
// `~/.linggen/skills/shared-memory/data/<sid>/page.json`. Mirrors
// Pulse's pattern so a session opened from a different browser /
// remote-access client sees the same cached widgets. Switched from
// browser localStorage in Phase 6 of the rebuild.

function currentSessionId() {
  return new URLSearchParams(window.location.search).get('session') || '';
}

function pageCachePath(sid) {
  return `~/.linggen/skills/shared-memory/data/${sid}/page.json`;
}

// Fire-and-forget — callers don't await. Failures swallowed so a
// slow disk write never blocks a render.
function cacheCurrentPage() {
  const sid = currentSessionId();
  if (!sid) return;
  writeJsonFile(pageCachePath(sid), getCurrentPage()).catch(() => {});
}

// Async — callers must `await tryRestoreCached(sid)`. Returns true if
// a non-empty cached page was restored, false otherwise (so the
// caller knows whether to paint the on-open dashboard from scratch).
async function tryRestoreCached(sessionId) {
  if (!sessionId) return false;
  const page = await readJsonFile(pageCachePath(sessionId)).catch(() => null);
  if (!page) return false;
  if (!(page.top_bar?.length || page.body?.length)) return false;
  restorePage(page);
  return true;
}
