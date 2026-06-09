// Memory · Browse — step 4.
//
// Adds the query parser (filter chips vs. semantic search text), the
// sort control, and filter-aware reloading. The list endpoint runs when
// the free-text portion is empty; otherwise we hit search.

const HEALTH_POLL_MS = 15_000;
const LIST_LIMIT = 50;
const SEARCH_LIMIT = 50;

// ── Vocabulary ────────────────────────────────────────────────────────────

const FACT_TYPES = new Set([
  'fact', 'preference', 'decision', 'tried', 'fixed', 'learned', 'built',
]);
const ORIGINS = new Set(['user', 'agent', 'derived']);
const OUTCOMES = new Set(['positive', 'negative', 'neutral']);

// ── Tiny API helper ───────────────────────────────────────────────────────

class ApiError extends Error {
  constructor(message, code, status) {
    super(message);
    this.code = code;
    this.status = status;
  }
}

async function api(path, body) {
  const res = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {}),
    cache: 'no-store',
  });
  let envelope = null;
  try { envelope = await res.json(); } catch { /* non-JSON body */ }
  if (!res.ok || !envelope?.ok) {
    const msg = envelope?.error ?? `HTTP ${res.status}`;
    const code = envelope?.code ?? 'NETWORK';
    throw new ApiError(msg, code, res.status);
  }
  return envelope.data;
}

// ── Health pill ───────────────────────────────────────────────────────────

async function pollHealth() {
  const el = document.getElementById('health');
  const started = performance.now();
  try {
    const res = await fetch('/api/health', { cache: 'no-store' });
    const elapsed = performance.now() - started;
    if (!res.ok) throw new Error(`status ${res.status}`);
    const body = await res.json();
    if (!body.ok) throw new Error(body.error || 'unhealthy');
    const slow = elapsed > 1000;
    el.className = slow ? 'health slow' : 'health ok';
    el.textContent = slow ? '⏻ slow' : '⏻ healthy';
  } catch {
    el.className = 'health offline';
    el.textContent = '⏻ offline';
  }
}

// ── Query parser ──────────────────────────────────────────────────────────
//
// Strict grammar — unknown fields or malformed values fall through to
// `text`, so the user sees exactly what was consumed (as chips) vs. left
// for semantic search (remaining in the input).

function parseQuery(raw) {
  const text = [];
  const picked = {
    contexts: [],
    type: null,
    from: null,
    outcome: null,
    since: null,
    until: null,
  };
  for (const token of raw.split(/\s+/).filter(Boolean)) {
    const match = token.match(/^\/([a-z]+):(.+)$/i);
    if (!match) { text.push(token); continue; }
    if (!applyFilterToken(match[1].toLowerCase(), match[2], picked)) {
      text.push(token);
    }
  }
  return { text: text.join(' '), picked };
}

function applyFilterToken(field, value, picked) {
  switch (field) {
    case 'context':
      picked.contexts.push(value);
      return true;
    case 'type':
      if (!FACT_TYPES.has(value)) return false;
      picked.type = value;
      return true;
    case 'from':
      if (!ORIGINS.has(value)) return false;
      picked.from = value;
      return true;
    case 'outcome':
      if (!OUTCOMES.has(value)) return false;
      picked.outcome = value;
      return true;
    case 'since':
    case 'until': {
      const iso = normalizeDate(value);
      if (!iso) return false;
      picked[field] = iso;
      return true;
    }
    default:
      return false;
  }
}

function normalizeDate(s) {
  // Accept bare yyyy-mm-dd by promoting to start-of-day UTC, otherwise
  // delegate to the built-in Date parser.
  if (/^\d{4}-\d{2}-\d{2}$/.test(s)) return `${s}T00:00:00Z`;
  const t = Date.parse(s);
  if (Number.isNaN(t)) return null;
  return new Date(t).toISOString();
}

// ── Filter + query state ──────────────────────────────────────────────────
//
// Single source of truth. Chips, input content, and sort select all
// re-render from this after every mutation.

const state = {
  filters: {
    contexts: [],
    type: null,
    from: null,
    outcome: null,
    since: null,
    until: null,
    source_session: null,
  },
  // Storage view — mutually exclusive across the three places a row can
  // live. Translates to `tier` / `episodic` on the wire (see filterPayload
  // + isEpisodicMode below). Default `all` matches the historical no-tier
  // semantic-table dump.
  //   'all'       → semantic table, no tier filter
  //   'core'      → semantic table, tier=core
  //   'semantic' → semantic table, tier=semantic
  //   'episodic'  → episodic table (tier irrelevant — rows don't carry one)
  view: 'all',
  text: '',
  sort: 'newest',

  loaded: [],
  offset: 0,
  hasMore: false,
  loading: false,

  selectedId: null,
  // `draft` is the working copy shown in the detail pane.
  //   {kind: 'edit', id, original: Fact, edited: Fact}
  //   {kind: 'new', edited: Fact}
  //   null = nothing selected
  draft: null,

  // Multi-select for bulk actions. Independent of `selectedId`, which is
  // the single "currently-viewed-in-detail" row.
  selected: new Set(),
};

function blankFact() {
  return {
    content: '',
    contexts: [],
    tags: [],
    type: 'fact',
    from: 'user',
    outcome: null,
    cwd: null,
    occurred_at: null,
  };
}

function cloneDraft(fact) {
  return {
    content: fact.content ?? '',
    contexts: Array.isArray(fact.contexts) ? [...fact.contexts] : [],
    tags: Array.isArray(fact.tags) ? [...fact.tags] : [],
    type: fact.type ?? 'fact',
    tier: fact.tier ?? 'semantic',
    from: fact.from ?? 'derived',
    outcome: fact.outcome ?? null,
    cwd: fact.cwd ?? null,
    occurred_at: fact.occurred_at ?? null,
  };
}

function hasAnyFilter() {
  const f = state.filters;
  return (
    f.contexts.length > 0 ||
    f.type !== null ||
    f.from !== null ||
    f.outcome !== null ||
    f.since !== null ||
    f.until !== null ||
    f.source_session !== null
  );
}

function mergeFilters(picked) {
  const f = state.filters;
  for (const c of picked.contexts) {
    if (!f.contexts.includes(c)) f.contexts.push(c);
  }
  if (picked.type !== null)    f.type = picked.type;
  if (picked.from !== null)    f.from = picked.from;
  if (picked.outcome !== null) f.outcome = picked.outcome;
  if (picked.since !== null)   f.since = picked.since;
  if (picked.until !== null)   f.until = picked.until;
}

function clearFilters() {
  state.filters = {
    contexts: [],
    type: null,
    from: null,
    outcome: null,
    since: null,
    until: null,
    source_session: null,
  };
}

function removeFilter(field, value) {
  if (field === 'contexts') {
    state.filters.contexts = state.filters.contexts.filter(c => c !== value);
  } else {
    state.filters[field] = null;
  }
}

// ── Query submit ──────────────────────────────────────────────────────────

function onQuerySubmit() {
  const input = document.getElementById('query');
  const { text, picked } = parseQuery(input.value);
  mergeFilters(picked);
  state.text = text;
  input.value = text;
  renderFiltersBar();
  reload();
}

// ── Data loading ──────────────────────────────────────────────────────────

function filterPayload() {
  const f = state.filters;
  const body = {};
  if (f.contexts.length > 0) body.contexts = f.contexts;
  if (f.type)    body.type = f.type;
  if (f.from)    body.from = f.from;
  if (f.outcome) body.outcome = f.outcome;
  if (f.since)   body.since = f.since;
  if (f.until)   body.until = f.until;
  if (f.source_session) body.source_session = f.source_session;
  // Storage view: 'episodic' is a top-level flag on the request (switches
  // table); 'core' / 'semantic' add a `tier` filter inside the semantic
  // table. 'all' leaves both off.
  if (state.view === 'episodic')  body.episodic = true;
  if (state.view === 'core')      body.tier = 'core';
  if (state.view === 'semantic') body.tier = 'semantic';
  return body;
}

function setView(name) {
  if (state.view === name) return;
  state.view = name;
  renderViewBar();
  reload();
}

function renderViewBar() {
  const bar = document.getElementById('view-tabs');
  if (!bar) return;
  for (const btn of bar.querySelectorAll('button[data-view]')) {
    const active = btn.dataset.view === state.view;
    btn.classList.toggle('active', active);
    btn.setAttribute('aria-selected', active ? 'true' : 'false');
  }
}

function isSearchMode() {
  return state.text.trim().length > 0;
}

// Fetch rows for the current view. For semantic-only views (`core`,
// `semantic`) and episodic-only, this is one round-trip. For `all`
// the daemon's list/search endpoint is single-table by design (the
// `episodic: true` flag flips which table is queried), so we fan out
// two parallel requests and merge — concat + sort by created_at desc,
// then trim to `limit`. Loses clean pagination beyond the first page
// in `all` mode, which is fine for the typical store size; loadMore()
// is suppressed there.
// Tag rows with their storage origin (`_storage`: 'episodic' | 'semantic')
// so the detail pane can show the right tier even after the All-view
// merge loses the per-fetch context. The episodic table doesn't carry
// a tier column, so we synthesize one from the table identity.
function tagStorage(rows, episodic) {
  const tag = episodic ? 'episodic' : 'semantic';
  for (const r of (rows || [])) r._storage = tag;
  return rows || [];
}

async function fetchRowsForView(endpoint, basePayload) {
  const isSearch = endpoint.endsWith('/search');

  // Search endpoint. A tab view ('semantic' / 'episodic') pins to one table
  // via the `table` param so its rows don't double-fetch. The 'all' view
  // makes ONE cross-table (`table=both`, the default) call: the daemon fuses
  // dense + lexical (BM25) rankings via RRF and dedups across both tables,
  // returning rows in global hybrid-relevance order. We must NOT re-sort
  // client-side — sorting by the cosine `score` would undo the RRF order
  // (a keyword hit with low cosine would sink) and the server already
  // collapsed cross-table dups. Storage label comes from each row's tier.
  if (isSearch) {
    if (state.view !== 'all') {
      const isEp = state.view === 'episodic';
      const rows = await api(endpoint, {
        ...basePayload,
        table: isEp ? 'episodic' : 'semantic',
      });
      return tagStorage(rows, isEp);
    }
    const rows = await api(endpoint, basePayload);
    return tagStorageFromTier(rows);
  }

  // List endpoint contract changed in `39e100b` — the daemon now spans
  // both tables when `episodic` is unset. So 'all' view = one call, no
  // client-side merge. Tab views ('core' / 'semantic' / 'episodic')
  // still pin to a single table via the existing `episodic: bool` flag
  // (set in `filterPayload`).
  const rows = await api(endpoint, basePayload);
  return tagStorageFromTier(rows);
}

// Tier-derived storage label. Invariant: rows in the episodic table
// carry `tier === 'episodic'`; semantic-table rows carry `core` or
// `semantic`. Both daemon insert paths enforce this on write.
function tagStorageFromTier(rows) {
  for (const r of (rows || [])) {
    r._storage = r.tier === 'episodic' ? 'episodic' : 'semantic';
  }
  return rows || [];
}

// Storage label: "core" / "semantic" / "episodic". Derived from
// `_storage` (which table the row came from) + `tier` (semantic-table
// rows only). Episodic rows always read "episodic" regardless of tier.
function storageLabel(row) {
  if (!row) return '—';
  if (row._storage === 'episodic') return 'episodic';
  return row.tier === 'core' ? 'core' : 'semantic';
}

// Host label: prefer the row's explicit `host` column (set on write by
// `detect_host()` / engine dispatch), fall back to cwd inference only
// for legacy rows that pre-date the column.
const HOST_LABELS = {
  'claude-code': 'Claude Code',
  'codex':       'Codex',
  'openclaw':    'OpenClaw',
  'linggen':     'Linggen',
};
function hostLabel(row) {
  if (!row) return '—';
  if (row.host) return HOST_LABELS[row.host] ?? row.host;
  const cwd = row.cwd || '';
  if (cwd.includes('/.claude/'))   return 'Claude Code';
  if (cwd.includes('/.codex/'))    return 'Codex';
  if (cwd.includes('/.openclaw/')) return 'OpenClaw';
  if (cwd.includes('/.linggen/'))  return 'Linggen';
  return '—';
}

async function reload() {
  state.loaded = [];
  state.offset = 0;
  state.hasMore = false;
  state.selectedId = null;
  state.draft = null;
  state.selected.clear();
  renderBulkSummary();
  closeBulkMenu();
  renderDetail();
  showLoading();
  try {
    const rows = isSearchMode()
      ? await fetchRowsForView('/api/memory/search', {
          query: state.text,
          ...filterPayload(),
          limit: SEARCH_LIMIT,
          // Console search is for human inspection: show every match ranked
          // by relevance, never an empty list. An explicit min_score of 0
          // overrides the daemon's recall floor (which terse queries like
          // "dog" fall under — every row scores below it and you get
          // nothing). The recall hook / engine auto-recall still use the
          // floor; only the console opts out.
          min_score: 0,
        })
      : await fetchRowsForView('/api/memory/list', {
          ...filterPayload(),
          sort: state.sort,
          limit: LIST_LIMIT,
          offset: 0,
        });
    state.loaded = rows;
    state.offset = rows.length;
    // Pagination via offset only works when the request hits a single
    // table. Search is capped at SEARCH_LIMIT (no offset); `all` mode
    // fans out two requests we can't offset together. Both → no more.
    state.hasMore = !isSearchMode()
                 && state.view !== 'all'
                 && rows.length === LIST_LIMIT;
    renderList();
    updateCount();
  } catch (err) {
    showError(err);
  }
  renderSortControl();
}

async function loadMore() {
  if (state.loading || !state.hasMore || isSearchMode()) return;
  state.loading = true;
  renderFooter();
  try {
    const rows = await api('/api/memory/list', {
      ...filterPayload(),
      sort: state.sort,
      limit: LIST_LIMIT,
      offset: state.offset,
    });
    state.loaded.push(...rows);
    state.offset += rows.length;
    state.hasMore = rows.length === LIST_LIMIT;
    renderList();
    updateCount();
  } catch (err) {
    showError(err);
  } finally {
    state.loading = false;
    renderFooter();
  }
}

// ── Filter-bar rendering ──────────────────────────────────────────────────

function renderFiltersBar() {
  const bar = document.getElementById('filters-bar');
  const chipsEl = document.getElementById('filter-chips');
  const visible = hasAnyFilter() || isSearchMode();
  bar.hidden = !visible;
  chipsEl.replaceChildren();
  for (const chip of activeFilterChips()) chipsEl.appendChild(chip);
  renderSortControl();
}

function activeFilterChips() {
  const f = state.filters;
  const chips = [];
  for (const c of f.contexts) chips.push(filterChip('context', c));
  if (f.type)    chips.push(filterChip('type', f.type));
  if (f.from)    chips.push(filterChip('from', f.from));
  if (f.outcome) chips.push(filterChip('outcome', f.outcome));
  if (f.since)   chips.push(filterChip('since', f.since));
  if (f.until)   chips.push(filterChip('until', f.until));
  if (f.source_session) chips.push(filterChip('source_session', f.source_session));
  return chips;
}

function filterChip(field, value) {
  const el = document.createElement('span');
  el.className = 'filter-chip';
  const key = document.createElement('span');
  key.className = 'key';
  key.textContent = `${field}:`;
  const val = document.createElement('span');
  val.className = 'value';
  val.textContent = value;
  const close = document.createElement('button');
  close.type = 'button';
  close.setAttribute('aria-label', `remove ${field} filter`);
  close.textContent = '×';
  close.addEventListener('click', () => {
    removeFilter(field === 'context' ? 'contexts' : field, value);
    renderFiltersBar();
    reload();
  });
  el.append(key, val, close);
  return el;
}

function renderSortControl() {
  const sort = document.getElementById('sort');
  sort.value = state.sort;
  sort.disabled = isSearchMode();
  sort.title = isSearchMode()
    ? 'Sort is disabled during semantic search — results are ranked by relevance'
    : '';
}

// ── List rendering ────────────────────────────────────────────────────────

function showLoading() {
  const list = document.getElementById('list');
  list.className = 'loading';
  list.replaceChildren(...[0, 1, 2].map(() => {
    const d = document.createElement('div');
    d.className = 'skel';
    return d;
  }));
  document.getElementById('list-footer').textContent = '';
}

function showError(err) {
  const list = document.getElementById('list');
  list.className = '';
  const banner = document.createElement('div');
  banner.className = 'error-banner';
  banner.textContent = `Couldn’t load facts: ${err.message} [${err.code ?? '—'}]`;
  list.replaceChildren(banner);
  document.getElementById('list-footer').textContent = '';
}

function renderList() {
  const list = document.getElementById('list');
  list.className = '';
  if (state.loaded.length === 0) {
    list.replaceChildren(emptyListNode());
    renderFooter();
    return;
  }
  list.replaceChildren(...state.loaded.map(renderRow));
  renderFooter();
}

function emptyListNode() {
  const empty = document.createElement('div');
  empty.className = 'empty';
  if (isSearchMode()) {
    empty.textContent = `No facts match "${state.text}".`;
  } else if (hasAnyFilter()) {
    empty.append('No facts match the current filters. ');
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.textContent = 'Clear filters';
    btn.addEventListener('click', () => {
      clearFilters();
      renderFiltersBar();
      reload();
    });
    empty.appendChild(btn);
  } else {
    empty.textContent =
      'Your memory is empty. Run a curation from the dashboard, or wait for step 6 to add one here.';
  }
  return empty;
}

function renderRow(fact) {
  const row = document.createElement('div');
  row.className = `row t-${fact.type ?? 'fact'}`;
  if (fact.id === state.selectedId) row.classList.add('selected');
  row.dataset.id = fact.id;
  row.setAttribute('role', 'option');
  row.setAttribute('aria-selected', fact.id === state.selectedId ? 'true' : 'false');
  row.addEventListener('click', (e) => {
    // Checkbox will own its own click handler later; skip selection when
    // the user is clicking directly on it.
    if (e.target.matches('input[type="checkbox"]')) return;
    selectRow(fact.id);
  });

  const box = document.createElement('input');
  box.type = 'checkbox';
  box.checked = state.selected.has(fact.id);
  box.setAttribute('aria-label', 'select fact');
  box.addEventListener('click', (e) => {
    e.stopPropagation();
    toggleSelect(fact.id);
  });

  const body = document.createElement('div');
  body.className = 'row-body';
  body.appendChild(renderRowHead(fact));
  body.appendChild(renderRowContent(fact.content));
  const meta = renderRowMeta(fact);
  if (meta) body.appendChild(meta);

  row.appendChild(box);
  row.appendChild(body);
  return row;
}

function selectRow(id) {
  if (state.selectedId === id && state.draft?.kind === 'edit') return;
  if (!confirmDiscardDraftIfDirty()) return;
  const fact = state.loaded.find(f => f.id === id);
  if (!fact) return;
  state.selectedId = id;
  state.draft = { kind: 'edit', id, original: cloneDraft(fact), edited: cloneDraft(fact) };
  syncSelectionDom(id);
  renderDetail();
}

function syncSelectionDom(id) {
  for (const el of document.querySelectorAll('.row')) {
    const match = el.dataset.id === id;
    el.classList.toggle('selected', match);
    el.setAttribute('aria-selected', match ? 'true' : 'false');
  }
}

function confirmDiscardDraftIfDirty() {
  if (!isDraftDirty()) return true;
  return window.confirm('Discard unsaved changes?');
}

function selectedFact() {
  if (!state.selectedId) return null;
  return state.loaded.find(f => f.id === state.selectedId) ?? null;
}

function renderRowHead(fact) {
  const head = document.createElement('div');
  head.className = 'row-head';

  const type = document.createElement('span');
  type.className = `row-type t-${fact.type ?? 'fact'}`;
  type.textContent = fact.type ?? 'fact';
  head.appendChild(type);

  if (fact.from) {
    const from = document.createElement('span');
    from.textContent = fact.from;
    head.appendChild(from);
  }

  if (fact.outcome) {
    const oc = document.createElement('span');
    oc.className = `row-outcome ${fact.outcome}`;
    oc.textContent = outcomeGlyph(fact.outcome);
    oc.title = fact.outcome;
    head.appendChild(oc);
  }

  if (typeof fact.score === 'number') {
    const score = document.createElement('span');
    score.className = 'row-score';
    score.textContent = fact.score.toFixed(3);
    score.title = `Relevance score (cosine similarity, range [-1, 1]; higher = closer match)`;
    head.appendChild(score);
  }

  // Storage + host badges — small dim chips next to the type/from
  // markers so a glance distinguishes core / semantic / episodic
  // rows, and which host wrote the row when cwd lets us tell.
  const storage = storageLabel(fact);
  const stor = document.createElement('span');
  stor.className = `row-storage s-${storage}`;
  stor.textContent = storage;
  stor.title = `storage tier (${storage})`;
  head.appendChild(stor);

  const host = hostLabel(fact);
  if (host !== '—') {
    const h = document.createElement('span');
    h.className = 'row-host';
    h.textContent = host;
    h.title = `inferred host (from cwd: ${fact.cwd || ''})`;
    head.appendChild(h);
  }

  const age = document.createElement('span');
  age.className = 'row-age';
  // Activity time (updated_at ?? created_at) — same key the server sorts
  // by, so the badge age and the row's position always agree.
  const activityTs = fact.updated_at ?? fact.created_at;
  age.textContent = relAge(activityTs);
  age.title = activityTs ?? '';
  head.appendChild(age);

  return head;
}

function renderRowContent(content) {
  const el = document.createElement('div');
  el.className = 'row-content';
  el.textContent = content ?? '';
  return el;
}

function renderRowMeta(fact) {
  const contexts = Array.isArray(fact.contexts) ? fact.contexts : [];
  const tags = Array.isArray(fact.tags) ? fact.tags.slice(0, 3) : [];
  if (contexts.length === 0 && tags.length === 0) return null;
  const meta = document.createElement('div');
  meta.className = 'row-meta';
  for (const c of contexts) meta.appendChild(chip(c, 'chip'));
  for (const t of tags) meta.appendChild(chip(t, 'chip tag'));
  return meta;
}

function chip(text, className) {
  const el = document.createElement('span');
  el.className = className;
  el.textContent = text;
  return el;
}

function renderFooter() {
  const el = document.getElementById('list-footer');
  el.replaceChildren();
  if (state.loading) {
    el.textContent = 'Loading…';
    return;
  }
  if (isSearchMode()) {
    el.textContent = state.loaded.length > 0 ? 'End of ranked results' : '';
    return;
  }
  if (!state.hasMore) {
    el.textContent = state.loaded.length > 0 ? 'End of list' : '';
    return;
  }
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.textContent = `Load ${LIST_LIMIT} more`;
  btn.addEventListener('click', loadMore);
  el.appendChild(btn);
}

function updateCount() {
  const el = document.getElementById('count');
  const n = state.loaded.length;
  if (n === 0) { el.textContent = '—'; return; }
  if (isSearchMode()) { el.textContent = `${n} match${n === 1 ? '' : 'es'}`; return; }
  const suffix = state.hasMore ? `≥${n}` : `${n}`;
  el.textContent = `showing ${n} of ${suffix}`;
}

// ── Detail pane (editable) ────────────────────────────────────────────────

function renderDetail() {
  const pane = document.getElementById('detail-pane');
  if (!state.draft) {
    const empty = document.createElement('div');
    empty.className = 'detail-empty';
    empty.textContent = 'Select a fact on the left, or click + Add to create one.';
    pane.replaceChildren(empty);
    return;
  }
  const root = document.createElement('div');
  root.className = 'detail';
  root.append(
    detailHead(state.draft.edited),
    detailContentEditor(),
    detailGrid(),
    validationNote(),
    detailActions(),
  );
  pane.replaceChildren(root);
}

function detailHead(edited) {
  const head = document.createElement('div');
  head.className = 'detail-head';
  const type = document.createElement('span');
  type.className = `row-type t-${edited.type ?? 'fact'}`;
  type.textContent = edited.type ?? 'fact';
  head.appendChild(type);
  const from = document.createElement('span');
  from.textContent = edited.from ?? 'derived';
  head.appendChild(from);
  if (edited.outcome) {
    const oc = document.createElement('span');
    oc.className = `row-outcome ${edited.outcome}`;
    oc.textContent = `${outcomeGlyph(edited.outcome)} ${edited.outcome}`;
    head.appendChild(oc);
  }
  if (state.draft.kind === 'new') {
    const tag = document.createElement('span');
    tag.textContent = '· new';
    tag.style.color = 'var(--accent)';
    head.appendChild(tag);
  }
  return head;
}

function detailContentEditor() {
  const ta = document.createElement('textarea');
  ta.className = 'detail-content-edit';
  ta.value = state.draft.edited.content;
  ta.rows = 3;
  ta.placeholder = 'One or two sentences — retrievable by meaning, not keyword.';
  ta.addEventListener('input', () => {
    state.draft.edited.content = ta.value;
    autoGrow(ta);
    renderActionsOnly();
  });
  // ⌘⏎ / Ctrl+Enter save is handled by the document-level keyboard
  // dispatcher — see `isSaveChord()`.
  requestAnimationFrame(() => autoGrow(ta));
  return ta;
}

function autoGrow(ta) {
  ta.style.height = 'auto';
  ta.style.height = `${Math.min(ta.scrollHeight + 2, 400)}px`;
}

function detailGrid() {
  const grid = document.createElement('dl');
  grid.className = 'detail-grid';
  const edited = state.draft.edited;
  const rows = [
    ['Contexts',  chipEditor('contexts', 'add context…')],
    ['Tags',      chipEditor('tags', 'topic:ui, intent:learn, …')],
    ['Type',      enumSelect('type', FACT_TYPE_LIST)],
    ['From',      enumSelect('from', ORIGIN_LIST)],
    ['Outcome',   nullableSelect('outcome', OUTCOME_LIST)],
    ['cwd',       clearableInput('cwd', '/path/to/workdir')],
  ];

  if (state.draft.kind === 'new') {
    rows.push(['Occurred', clearableInput('occurred_at', 'YYYY-MM-DDTHH:MM:SSZ')]);
  } else {
    rows.push(['Occurred', textOrDim(formatTimestamp(edited.occurred_at))]);
    rows.push(['divider']);
    const original = factForSelected();
    rows.push(['Tier', enumSelect('tier', TIER_LIST)]);
    rows.push(['Host', textOrDim(hostLabel(original))]);
    rows.push(['Created', textOrDim(formatTimestamp(original?.created_at))]);
    rows.push(['Session', idCell(original?.source_session)]);
    rows.push(['id',      idCell(original?.id)]);
  }

  for (const row of rows) {
    if (row[0] === 'divider') {
      const hr = document.createElement('hr');
      hr.className = 'detail-divider';
      hr.style.gridColumn = '1 / -1';
      grid.appendChild(hr);
      continue;
    }
    const [label, node] = row;
    const dt = document.createElement('dt');
    dt.textContent = label;
    const dd = document.createElement('dd');
    dd.appendChild(node);
    grid.append(dt, dd);
  }
  return grid;
}

function factForSelected() {
  if (!state.selectedId) return null;
  return state.loaded.find(f => f.id === state.selectedId) ?? null;
}

// ── Field controls ────────────────────────────────────────────────────────

function chipEditor(field, placeholder) {
  const wrap = document.createElement('div');
  wrap.className = 'chip-editor';
  const values = state.draft.edited[field];

  for (const [i, v] of values.entries()) {
    wrap.appendChild(chipEditorItem(field, v, i));
  }

  const input = document.createElement('input');
  input.type = 'text';
  input.placeholder = placeholder;
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      commitChip(field, input.value);
      input.value = '';
    } else if (e.key === 'Backspace' && input.value === '' && values.length > 0) {
      removeChip(field, values.length - 1);
    }
  });
  input.addEventListener('blur', () => {
    if (input.value.trim() !== '') {
      commitChip(field, input.value);
      input.value = '';
    }
  });
  wrap.appendChild(input);
  return wrap;
}

function chipEditorItem(field, value, idx) {
  const chipEl = document.createElement('span');
  chipEl.className = field === 'tags' ? 'editable-chip tag' : 'editable-chip';
  chipEl.appendChild(document.createTextNode(value));
  const close = document.createElement('button');
  close.type = 'button';
  close.textContent = '×';
  close.setAttribute('aria-label', `remove ${field.slice(0, -1)}`);
  close.addEventListener('click', () => removeChip(field, idx));
  chipEl.appendChild(close);
  return chipEl;
}

function commitChip(field, raw) {
  const v = raw.trim();
  if (!v) return;
  const values = state.draft.edited[field];
  if (values.includes(v)) return;
  values.push(v);
  renderDetail();
  focusChipInput(field);
}

function removeChip(field, idx) {
  state.draft.edited[field].splice(idx, 1);
  renderDetail();
  focusChipInput(field);
}

function focusChipInput(field) {
  requestAnimationFrame(() => {
    const label = field === 'contexts' ? 'Contexts' : 'Tags';
    const dts = document.querySelectorAll('.detail-grid dt');
    for (const dt of dts) {
      if (dt.textContent === label) {
        dt.nextElementSibling?.querySelector('input')?.focus();
        return;
      }
    }
  });
}

const FACT_TYPE_LIST = ['fact', 'preference', 'decision', 'tried', 'fixed', 'learned', 'built'];
const ORIGIN_LIST    = ['user', 'agent', 'derived'];
const OUTCOME_LIST   = ['positive', 'negative', 'neutral'];
const TIER_LIST      = ['core', 'semantic', 'episodic'];

function enumSelect(field, values) {
  const sel = document.createElement('select');
  for (const v of values) {
    const opt = document.createElement('option');
    opt.value = v;
    opt.textContent = v;
    sel.appendChild(opt);
  }
  sel.value = state.draft.edited[field];
  sel.addEventListener('change', () => {
    state.draft.edited[field] = sel.value;
    if (field === 'type') renderDetail();
    else renderActionsOnly();
  });
  return sel;
}

function nullableSelect(field, values) {
  const sel = document.createElement('select');
  const none = document.createElement('option');
  none.value = '';
  none.textContent = '— (none)';
  sel.appendChild(none);
  for (const v of values) {
    const opt = document.createElement('option');
    opt.value = v;
    opt.textContent = v;
    sel.appendChild(opt);
  }
  sel.value = state.draft.edited[field] ?? '';
  sel.addEventListener('change', () => {
    state.draft.edited[field] = sel.value === '' ? null : sel.value;
    if (field === 'outcome') renderDetail();
    else renderActionsOnly();
  });
  return sel;
}

function clearableInput(field, placeholder) {
  const wrap = document.createElement('div');
  wrap.className = 'input-with-clear';
  const input = document.createElement('input');
  input.type = 'text';
  input.placeholder = placeholder;
  input.value = state.draft.edited[field] ?? '';
  input.addEventListener('input', () => {
    state.draft.edited[field] = input.value === '' ? null : input.value;
    renderActionsOnly();
  });
  wrap.appendChild(input);
  const clear = document.createElement('button');
  clear.type = 'button';
  clear.textContent = '×';
  clear.title = 'Clear';
  clear.addEventListener('click', () => {
    state.draft.edited[field] = null;
    input.value = '';
    renderActionsOnly();
  });
  wrap.appendChild(clear);
  return wrap;
}

// ── Action buttons + validation ───────────────────────────────────────────

function validationNote() {
  const el = document.createElement('div');
  el.className = 'validation-note';
  el.id = 'validation-note';
  const msg = validateDraft();
  if (msg) el.textContent = msg;
  return el;
}

function validateDraft() {
  if (!state.draft.edited.content || state.draft.edited.content.trim() === '') {
    return 'Content is required.';
  }
  const occ = state.draft.edited.occurred_at;
  if (occ && normalizeDate(occ) === null) {
    return 'Occurred must be ISO 8601 (e.g. 2026-04-01T12:00:00Z) or YYYY-MM-DD.';
  }
  return null;
}

function detailActions() {
  const bar = document.createElement('div');
  bar.className = 'detail-actions';
  bar.id = 'detail-actions';

  const save = document.createElement('button');
  save.type = 'button';
  save.className = 'btn-primary';
  save.textContent = state.draft.kind === 'new' ? 'Add fact' : 'Save';
  save.disabled = !canSave();
  save.addEventListener('click', saveDraft);

  const cancel = document.createElement('button');
  cancel.type = 'button';
  cancel.textContent = 'Cancel';
  cancel.addEventListener('click', cancelDraft);

  bar.append(save, cancel);

  if (state.draft.kind === 'edit') {
    const spacer = document.createElement('span');
    spacer.className = 'spacer';
    const del = document.createElement('button');
    del.type = 'button';
    del.className = 'btn-danger';
    del.textContent = 'Delete';
    del.addEventListener('click', () => deleteFact(state.draft.id));
    bar.append(spacer, del);
  }

  return bar;
}

function renderActionsOnly() {
  const old = document.getElementById('detail-actions');
  if (!old) return;
  old.replaceWith(detailActions());
  const note = document.getElementById('validation-note');
  if (note) note.textContent = validateDraft() ?? '';
}

function canSave() {
  if (validateDraft()) return false;
  if (state.draft.kind === 'new') return true;
  return isDraftDirty();
}

function isDraftDirty() {
  if (!state.draft) return false;
  if (state.draft.kind === 'new') {
    return state.draft.edited.content.trim() !== '';
  }
  return diffForUpdate(state.draft.original, state.draft.edited) !== null;
}

// ── Save / cancel / delete / add ──────────────────────────────────────────

async function saveDraft() {
  if (!state.draft) return;
  if (validateDraft()) return;
  if (state.draft.kind === 'new') {
    await saveNewFact();
  } else {
    await saveEditedFact();
  }
}

// True when the currently-shown table is the staging `episodic` store.
// Add / update / delete / get all need `episodic: true` on the wire to
// target the right table, otherwise they 404 (semantic-default lookup
// can't see episodic rows).
function isEpisodicMode() {
  return state.view === 'episodic';
}

function buildAddPayload(e) {
  const body = { content: e.content };
  if (e.contexts.length > 0) body.contexts = e.contexts;
  if (e.tags.length > 0)     body.tags = e.tags;
  body.type = e.type;
  body.from = e.from;
  if (e.outcome) body.outcome = e.outcome;
  if (e.cwd)     body.cwd = e.cwd;
  if (e.occurred_at) body.occurred_at = normalizeDate(e.occurred_at);
  // Core writes carry `tier: 'core'`; everything else stays at the
  // semantic-table default. Episodic writes ride the top-level flag.
  if (state.view === 'core')      body.tier = 'core';
  if (state.view === 'semantic') body.tier = 'semantic';
  if (isEpisodicMode())           body.episodic = true;
  return body;
}

async function saveNewFact() {
  const body = buildAddPayload(state.draft.edited);
  try {
    // `/api/memory/add` returns `{action, fact, [similarity], [previous_id]}`
    // — the wrapper carries dedup metadata that other endpoints don't have.
    // Unwrap to the fact itself for list / draft state.
    const result = await api('/api/memory/add', body);
    const fact = result.fact ?? result;
    state.loaded.unshift(fact);
    state.offset += 1;
    state.selectedId = fact.id;
    state.draft = {
      kind: 'edit',
      id: fact.id,
      original: cloneDraft(fact),
      edited: cloneDraft(fact),
    };
    renderList();
    updateCount();
    renderDetail();
  } catch (err) {
    showError(err);
  }
}

function diffForUpdate(original, edited) {
  const patch = { id: state.draft.id };
  let dirty = false;

  if (edited.content !== original.content) {
    patch.content = edited.content;
    dirty = true;
  }
  if (!arraysEqual(edited.contexts, original.contexts)) {
    patch.contexts = edited.contexts;
    dirty = true;
  }
  if (!arraysEqual(edited.tags, original.tags)) {
    patch.tags = edited.tags;
    dirty = true;
  }
  if (edited.type !== original.type) {
    patch.type = edited.type;
    dirty = true;
  }
  if (edited.tier !== original.tier) {
    patch.tier = edited.tier;
    dirty = true;
  }
  if (edited.from !== original.from) {
    patch.from = edited.from;
    dirty = true;
  }
  const oc = nullablePatch(original.outcome, edited.outcome, 'outcome', 'clear_outcome');
  if (oc) { Object.assign(patch, oc); dirty = true; }
  const cw = nullablePatch(original.cwd, edited.cwd, 'cwd', 'clear_cwd');
  if (cw) { Object.assign(patch, cw); dirty = true; }
  return dirty ? patch : null;
}

function nullablePatch(originalVal, editedVal, setKey, clearKey) {
  if (originalVal === editedVal) return null;
  if (editedVal === null || editedVal === '') return { [clearKey]: true };
  return { [setKey]: editedVal };
}

function arraysEqual(a, b) {
  if (a.length !== b.length) return false;
  return a.every((v, i) => v === b[i]);
}

async function saveEditedFact() {
  const patch = diffForUpdate(state.draft.original, state.draft.edited);
  if (!patch) return;
  if (isEpisodicMode()) patch.episodic = true;
  try {
    const updated = await api('/api/memory/update', patch);
    // Re-derive `_storage` from the (possibly changed) tier so storageLabel()
    // reads the correct table identity after a cross-table tier move.
    tagStorageFromTier([updated]);
    const idx = state.loaded.findIndex(f => f.id === updated.id);
    if (idx >= 0) state.loaded[idx] = updated;
    state.draft = {
      kind: 'edit',
      id: updated.id,
      original: cloneDraft(updated),
      edited: cloneDraft(updated),
    };
    renderList();
    updateCount();
    renderDetail();
    syncSelectionDom(updated.id);
  } catch (err) {
    showError(err);
  }
}

function cancelDraft() {
  if (!state.draft) return;
  if (state.draft.kind === 'new') {
    if (isDraftDirty() && !window.confirm('Discard new fact?')) return;
    state.draft = null;
    state.selectedId = null;
    syncSelectionDom(null);
    renderDetail();
    return;
  }
  if (isDraftDirty() && !window.confirm('Discard unsaved changes?')) return;
  state.draft.edited = cloneDraft(state.draft.original);
  renderDetail();
}

async function deleteFact(id) {
  const fact = state.loaded.find(f => f.id === id);
  if (!fact) return;
  const preview = (fact.content ?? '').slice(0, 80);
  if (!window.confirm(`Delete this fact?\n\n"${preview}${preview.length === 80 ? '…' : ''}"\n\nThis cannot be undone.`)) return;
  const req = { id };
  if (isEpisodicMode()) req.episodic = true;
  try {
    await api('/api/memory/delete', req);
    state.loaded = state.loaded.filter(f => f.id !== id);
    state.offset = Math.max(0, state.offset - 1);
    state.selectedId = null;
    state.draft = null;
    renderList();
    updateCount();
    renderDetail();
  } catch (err) {
    showError(err);
  }
}

function startNewFact() {
  if (!confirmDiscardDraftIfDirty()) return;
  state.selectedId = null;
  state.draft = { kind: 'new', edited: blankFact() };
  syncSelectionDom(null);
  renderDetail();
  requestAnimationFrame(() => {
    document.querySelector('.detail-content-edit')?.focus();
  });
}

// ── Detail helpers (unchanged cells) ──────────────────────────────────────

function chipsList(values) {
  const arr = Array.isArray(values) ? values : [];
  if (arr.length === 0) return dimText('—');
  const wrap = document.createElement('div');
  wrap.className = 'detail-chips';
  for (const v of arr) wrap.appendChild(chip(v, 'chip'));
  return wrap;
}

function textOrDim(value) {
  if (value === null || value === undefined || value === '') return dimText('—');
  const span = document.createElement('span');
  span.textContent = value;
  return span;
}

function dimText(text) {
  const span = document.createElement('span');
  span.className = 'dim';
  span.textContent = text;
  return span;
}

function idCell(value) {
  if (!value) return dimText('—');
  const wrap = document.createElement('span');
  wrap.className = 'detail-id';
  const short = document.createElement('span');
  short.className = 'truncated';
  short.textContent = value;
  short.title = value;
  wrap.append(short, copyButton(value));
  return wrap;
}

function copyButton(value) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'copy-btn';
  btn.textContent = 'copy';
  btn.addEventListener('click', async (e) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(value);
      btn.textContent = 'copied';
      btn.classList.add('copied');
      setTimeout(() => {
        btn.textContent = 'copy';
        btn.classList.remove('copied');
      }, 1200);
    } catch {
      btn.textContent = 'err';
    }
  });
  return btn;
}

function formatTimestamp(iso) {
  if (!iso) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const d = new Date(t);
  const pad = (n) => String(n).padStart(2, '0');
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ` +
         `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())} UTC`;
}

// ── Formatting helpers ────────────────────────────────────────────────────

const OUTCOME_GLYPH = { positive: '✓', negative: '✗', neutral: '—' };
function outcomeGlyph(o) { return OUTCOME_GLYPH[o] ?? '—'; }

const AGE_UNITS = [
  { ms: 60_000,        suffix: 'm', divisor: 60_000 },
  { ms: 3_600_000,     suffix: 'h', divisor: 3_600_000 },
  { ms: 86_400_000,    suffix: 'd', divisor: 86_400_000 },
  { ms: 604_800_000,   suffix: 'w', divisor: 604_800_000 },
  { ms: 2_592_000_000, suffix: 'mo', divisor: 2_592_000_000 },
];

function relAge(iso) {
  if (!iso) return '';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '';
  const delta = Math.max(0, Date.now() - t);
  if (delta < AGE_UNITS[0].ms) return 'just now';
  let chosen = AGE_UNITS[0];
  for (const u of AGE_UNITS) {
    if (delta >= u.ms) chosen = u;
  }
  return `${Math.floor(delta / chosen.divisor)}${chosen.suffix} ago`;
}

// ── Bulk selection + menu ─────────────────────────────────────────────────

function toggleSelect(id) {
  if (state.selected.has(id)) state.selected.delete(id);
  else state.selected.add(id);
  renderBulkSummary();
  // If the menu is open, re-render so "Delete selected (N)" updates live.
  if (!document.getElementById('bulk-menu').hidden) renderBulkMenu();
}

function renderBulkSummary() {
  const btn = document.getElementById('bulk');
  const n = state.selected.size;
  btn.textContent = n > 0 ? `⋯ Bulk (${n}) ▾` : '⋯ Bulk ▾';
}

function toggleBulkMenu() {
  const menu = document.getElementById('bulk-menu');
  if (menu.hidden) openBulkMenu();
  else closeBulkMenu();
}

function openBulkMenu() {
  const menu = document.getElementById('bulk-menu');
  renderBulkMenu();
  menu.hidden = false;
  document.getElementById('bulk').setAttribute('aria-expanded', 'true');
}

function closeBulkMenu() {
  const menu = document.getElementById('bulk-menu');
  menu.hidden = true;
  document.getElementById('bulk').setAttribute('aria-expanded', 'false');
}

function renderBulkMenu() {
  const menu = document.getElementById('bulk-menu');
  menu.replaceChildren();
  const n = state.selected.size;
  const loadedN = state.loaded.length;
  const filterActive = hasAnyFilter();
  const allLoadedSelected = loadedN > 0 && n >= loadedN;
  // If everything loaded is already selected AND nothing more to load → toggle becomes deselect.
  const toggleOff = allLoadedSelected && !state.hasMore;
  // Label reflects whether click will add, add-more, or clear.
  let selectAllLabel;
  if (toggleOff) {
    selectAllLabel = `Deselect all (${n})`;
  } else if (state.hasMore) {
    selectAllLabel = `Select all (${loadedN}+ — load remaining)`;
  } else {
    selectAllLabel = `Select all (${loadedN})`;
  }
  menu.append(
    bulkItem(
      selectAllLabel,
      loadedN === 0,
      null,
      bulkSelectAll,
    ),
    bulkItem(`Delete selected (${n})`, n === 0, 'danger', bulkDeleteSelected),
    bulkItem(
      'Forget by current filter…',
      !filterActive,
      'danger',
      bulkForgetByFilter,
      filterActive ? null : 'add a filter first',
    ),
    bulkItem('Clear selection', n === 0, null, () => {
      state.selected.clear();
      renderBulkSummary();
      renderList();
      closeBulkMenu();
    }),
  );
}

// Select every currently-loaded fact. If more pages exist (non-search mode),
// load them first so a single click selects the entire store. Second click
// (when all-loaded-selected and no more to load) deselects everything — the
// menu label flips to "Deselect all" in that state.
async function bulkSelectAll() {
  const allLoadedSelected =
    state.loaded.length > 0 && state.selected.size >= state.loaded.length;
  if (allLoadedSelected && !state.hasMore) {
    state.selected.clear();
    renderBulkSummary();
    renderList();
    return;
  }
  // Load remaining pages before selecting (skip in search mode — pagination disabled there).
  while (state.hasMore && !isSearchMode()) {
    // eslint-disable-next-line no-await-in-loop
    await loadMore();
    // loadMore() may silently no-op if already loading; break to avoid an infinite loop
    // if something unexpected leaves state.loading stuck.
    if (state.loading) break;
  }
  for (const fact of state.loaded) {
    state.selected.add(fact.id);
  }
  renderBulkSummary();
  renderList();
}

function bulkItem(label, disabled, variant, onClick, hint) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'bulk-item' + (variant ? ` ${variant}` : '');
  btn.disabled = disabled;
  btn.setAttribute('role', 'menuitem');
  const txt = document.createElement('span');
  txt.textContent = label;
  btn.appendChild(txt);
  if (hint) {
    const h = document.createElement('span');
    h.className = 'hint';
    h.textContent = hint;
    btn.appendChild(h);
  }
  btn.addEventListener('click', () => {
    if (btn.disabled) return;
    closeBulkMenu();
    onClick();
  });
  return btn;
}

// ── Delete selected ───────────────────────────────────────────────────────

async function bulkDeleteSelected() {
  const ids = Array.from(state.selected);
  if (ids.length === 0) return;
  if (!window.confirm(`Delete ${ids.length} selected fact${ids.length === 1 ? '' : 's'}? This cannot be undone.`)) return;

  const results = await Promise.all(ids.map((id) => deleteOne(id)));
  const failed = results.filter(r => !r.ok);

  const survived = new Set(failed.map(r => r.id));
  state.loaded = state.loaded.filter(f => !state.selected.has(f.id) || survived.has(f.id));
  state.offset = state.loaded.length;
  state.selected = survived;

  // Clear detail if it pointed at a deleted row.
  if (state.selectedId && !state.loaded.some(f => f.id === state.selectedId)) {
    state.selectedId = null;
    state.draft = null;
  }

  renderList();
  updateCount();
  renderDetail();
  renderBulkSummary();

  if (failed.length > 0) {
    showBulkError(
      `Failed to delete ${failed.length} row(s): ` +
      failed.map(r => `${r.id.slice(0, 8)}… — ${r.err}`).join('; '),
    );
  } else {
    clearBulkError();
  }
}

async function deleteOne(id) {
  const req = { id };
  if (isEpisodicMode()) req.episodic = true;
  try {
    await api('/api/memory/delete', req);
    return { ok: true, id };
  } catch (err) {
    return { ok: false, id, err: err.message ?? 'unknown' };
  }
}

function showBulkError(msg) {
  clearBulkError();
  const el = document.createElement('div');
  el.className = 'bulk-error';
  el.id = 'bulk-error';
  el.textContent = msg;
  const pane = document.getElementById('list-pane');
  pane.insertBefore(el, document.getElementById('list'));
}

function clearBulkError() {
  document.getElementById('bulk-error')?.remove();
}

// ── Forget by filter ──────────────────────────────────────────────────────

const FORGET_PREFLIGHT_CAP = 1000;

async function bulkForgetByFilter() {
  if (!hasAnyFilter()) return;
  let preflight;
  try {
    preflight = await api('/api/memory/list', {
      ...filterPayload(),
      sort: 'newest',
      limit: FORGET_PREFLIGHT_CAP,
      offset: 0,
    });
  } catch (err) {
    showBulkError(`Could not preflight: ${err.message}`);
    return;
  }
  const count = preflight.length;
  const countStr = count >= FORGET_PREFLIGHT_CAP ? `≥${FORGET_PREFLIGHT_CAP}` : String(count);
  const summary = describeActiveFilters();

  if (count === 0) {
    window.alert(`No facts match ${summary}. Nothing to forget.`);
    return;
  }
  if (!window.confirm(
    `This will delete ${countStr} fact${count === 1 ? '' : 's'} matching:\n\n${summary}\n\n` +
    `This cannot be undone.`,
  )) return;

  try {
    await api('/api/memory/forget', filterPayload());
    clearBulkError();
    reload();
  } catch (err) {
    showBulkError(`Forget failed: ${err.message}`);
  }
}

function describeActiveFilters() {
  const f = state.filters;
  const parts = [];
  for (const c of f.contexts) parts.push(`context=${c}`);
  if (f.type)    parts.push(`type=${f.type}`);
  if (f.from)    parts.push(`from=${f.from}`);
  if (f.outcome) parts.push(`outcome=${f.outcome}`);
  if (f.since)   parts.push(`since=${f.since}`);
  if (f.until)   parts.push(`until=${f.until}`);
  return parts.join(', ');
}

// ── Keyboard map ──────────────────────────────────────────────────────────
//
// The document-level dispatcher runs on every keydown. When focus is in
// an editable, only the "global overrides" fire; otherwise the full map
// is in scope. `g g` is a chord with a 700ms window.

const PLATFORM_MAC = /Mac|iPhone|iPad/.test(navigator.platform);

const NAV_KEYS = new Map([
  ['/',         focusQuery],
  ['?',         toggleHelp],
  ['j',         () => moveSelection(+1)],
  ['k',         () => moveSelection(-1)],
  ['ArrowDown', () => moveSelection(+1)],
  ['ArrowUp',   () => moveSelection(-1)],
  ['G',         selectLastRow],
  ['x',         toggleCheckboxOnSelected],
  ['e',         focusContentEditor],
  ['Enter',     focusContentEditor],
  ['Backspace', deleteSelectedRow],
  ['Delete',    deleteSelectedRow],
  ['n',         startNewFact],
]);

let pendingG = false;
let pendingGTimer = null;

document.addEventListener('keydown', onKeyDown);

function onKeyDown(e) {
  // Escape is a cascade — works everywhere.
  if (e.key === 'Escape') {
    handleEscape(e);
    return;
  }

  // ⌘⏎ / Ctrl+⏎ and ⌘S / Ctrl+S save from anywhere in the app.
  if (isSaveChord(e)) {
    if (state.draft && canSave()) {
      e.preventDefault();
      saveDraft();
    }
    return;
  }

  // Let the editable owner handle all other keys — don't steal typing.
  if (isEditableFocused()) return;

  // Help overlay absorbs other keys while open (besides Esc already handled).
  if (isHelpOpen()) return;

  if (e.key === 'g') {
    handleG(e);
    return;
  }

  const handler = NAV_KEYS.get(e.key);
  if (!handler) return;
  e.preventDefault();
  handler();
}

function isEditableFocused() {
  const el = document.activeElement;
  if (!el) return false;
  if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') return true;
  return el.isContentEditable === true;
}

function isSaveChord(e) {
  const mod = PLATFORM_MAC ? e.metaKey : e.ctrlKey;
  if (!mod) return false;
  return e.key === 'Enter' || e.key === 's' || e.key === 'S';
}

function handleEscape(e) {
  if (isHelpOpen()) { e.preventDefault(); closeHelp(); return; }
  const menuOpen = !document.getElementById('bulk-menu').hidden;
  if (menuOpen)              { e.preventDefault(); closeBulkMenu(); return; }
  if (isEditableFocused())   { document.activeElement.blur(); return; }
  if (state.selectedId || state.draft) {
    e.preventDefault();
    clearSelection();
  }
}

function clearSelection() {
  if (state.draft && isDraftDirty() && !window.confirm('Discard unsaved changes?')) return;
  state.selectedId = null;
  state.draft = null;
  syncSelectionDom(null);
  renderDetail();
}

function handleG(e) {
  if (pendingG) {
    clearTimeout(pendingGTimer);
    pendingG = false;
    e.preventDefault();
    selectFirstRow();
    return;
  }
  pendingG = true;
  e.preventDefault();
  pendingGTimer = setTimeout(() => { pendingG = false; }, 700);
}

// ── Keyboard actions ──────────────────────────────────────────────────────

function focusQuery() {
  document.getElementById('query').focus();
}

function moveSelection(delta) {
  if (state.loaded.length === 0) return;
  const idx = state.loaded.findIndex(f => f.id === state.selectedId);
  const next = idx < 0
    ? (delta > 0 ? 0 : state.loaded.length - 1)
    : Math.max(0, Math.min(state.loaded.length - 1, idx + delta));
  selectRow(state.loaded[next].id);
  scrollSelectedIntoView();
}

function selectFirstRow() {
  if (state.loaded.length === 0) return;
  selectRow(state.loaded[0].id);
  scrollSelectedIntoView();
}

function selectLastRow() {
  if (state.loaded.length === 0) return;
  selectRow(state.loaded[state.loaded.length - 1].id);
  scrollSelectedIntoView();
}

function scrollSelectedIntoView() {
  if (!state.selectedId) return;
  const row = document.querySelector(`.row[data-id="${state.selectedId}"]`);
  row?.scrollIntoView({ block: 'nearest' });
}

function toggleCheckboxOnSelected() {
  if (!state.selectedId) return;
  toggleSelect(state.selectedId);
  // Also reflect on the checkbox element without a full re-render.
  const box = document.querySelector(`.row[data-id="${state.selectedId}"] input[type="checkbox"]`);
  if (box) box.checked = state.selected.has(state.selectedId);
}

function focusContentEditor() {
  if (!state.draft) {
    // `e` / Enter in the list with nothing drafted → open the selected row
    // for edit by re-selecting it (selectRow initializes the draft).
    if (state.selectedId) selectRow(state.selectedId);
    if (!state.draft) return;
  }
  document.querySelector('.detail-content-edit')?.focus();
}

function deleteSelectedRow() {
  if (!state.selectedId) return;
  deleteFact(state.selectedId);
}

// ── Help overlay ──────────────────────────────────────────────────────────

const HELP_SECTIONS = [
  ['Navigation', [
    ['/',              'Focus search'],
    ['j · ↓',          'Next row'],
    ['k · ↑',          'Previous row'],
    ['g g',            'Jump to first row'],
    ['G',              'Jump to last loaded row'],
    ['Esc',            'Step back: close menu → blur input → clear selection'],
  ]],
  ['Editing', [
    ['e · Enter',      'Edit selected row’s content'],
    ['n',              'New fact'],
    ['x',              'Toggle checkbox on selected row'],
    [PLATFORM_MAC ? '⌘⏎ · ⌘S' : 'Ctrl+⏎ · Ctrl+S', 'Save'],
    ['⌫ · Del',        'Delete selected row (in list)'],
  ]],
  ['Meta', [
    ['?',              'Toggle this help'],
  ]],
];

function toggleHelp() {
  if (isHelpOpen()) closeHelp();
  else openHelp();
}

function isHelpOpen() {
  return !document.getElementById('help-overlay').hidden;
}

function openHelp() {
  const overlay = document.getElementById('help-overlay');
  overlay.replaceChildren(buildHelpCard());
  overlay.hidden = false;
  overlay.addEventListener('click', onHelpBackdropClick);
  overlay.querySelector('.help-close')?.focus();
}

function closeHelp() {
  const overlay = document.getElementById('help-overlay');
  overlay.hidden = true;
  overlay.removeEventListener('click', onHelpBackdropClick);
  overlay.replaceChildren();
}

function onHelpBackdropClick(e) {
  if (e.target === e.currentTarget) closeHelp();
}

// ── Settings modal ─────────────────────────────────────────────────
// Reads + writes /api/config (currently just episodic_ttl_days). Read
// path is also used by the dream pass on every host — keep the schema
// minimal and forward-compatible.

function isSettingsOpen() {
  return !document.getElementById('settings-overlay').hidden;
}

function toggleSettings() {
  if (isSettingsOpen()) closeSettings();
  else openSettings();
}

async function openSettings() {
  const overlay = document.getElementById('settings-overlay');
  overlay.replaceChildren(await buildSettingsCard());
  overlay.hidden = false;
  overlay.addEventListener('click', onSettingsBackdropClick);
  overlay.querySelector('input')?.focus();
}

function closeSettings() {
  const overlay = document.getElementById('settings-overlay');
  overlay.hidden = true;
  overlay.removeEventListener('click', onSettingsBackdropClick);
  overlay.replaceChildren();
}

function onSettingsBackdropClick(e) {
  if (e.target === e.currentTarget) closeSettings();
}

async function buildSettingsCard() {
  const card = document.createElement('div');
  card.className = 'help-card settings-card';
  card.setAttribute('role', 'document');
  // Load current config (best-effort — fall back to defaults).
  let cfg = { episodic_ttl_days: 7, recall_min_score: 0.6 };
  try {
    const resp = await fetch('/api/config');
    const json = await resp.json();
    cfg = { ...cfg, ...(json?.data ?? {}) };
  } catch { /* defaults */ }
  card.innerHTML = `
    <header>
      <h2 id="settings-title">Settings</h2>
      <button type="button" class="help-close" aria-label="Close" title="Close (Esc)">×</button>
    </header>
    <div class="settings-row">
      <label for="settings-ttl">Episodic TTL (days)</label>
      <input id="settings-ttl" type="number" min="1" max="3650" step="1"
             value="${cfg.episodic_ttl_days}" />
      <p class="settings-hint">
        How long an episodic row survives before the next dream pass is
        allowed to promote-or-evict it. Default 7. Increase to let
        episodic memory live longer before consolidation.
      </p>
    </div>
    <div class="settings-row">
      <label for="settings-recall">Recall threshold</label>
      <input id="settings-recall" type="number" min="0" max="1" step="0.05"
             value="${Math.round((cfg.recall_min_score ?? 0.6) * 100) / 100}" />
      <p class="settings-hint">
        Minimum cosine similarity for a memory to be recalled. Applied across
        every host (Claude Code, Codex, Linggen) unless that host overrides it.
        Default 0.6. Higher = stricter (fewer, more relevant); lower = broader.
      </p>
    </div>
    <div class="settings-actions">
      <button type="button" class="settings-save">Save</button>
      <button type="button" class="settings-cancel">Cancel</button>
    </div>
    <p class="settings-status" hidden></p>
  `;
  card.querySelector('.help-close').addEventListener('click', closeSettings);
  card.querySelector('.settings-cancel').addEventListener('click', closeSettings);
  card.querySelector('.settings-save').addEventListener('click', async () => {
    const status = card.querySelector('.settings-status');
    const ttl = parseInt(card.querySelector('#settings-ttl').value, 10);
    if (!Number.isFinite(ttl) || ttl < 1 || ttl > 3650) {
      status.textContent = 'Episodic TTL must be 1–3650 days.';
      status.hidden = false;
      return;
    }
    const recall = parseFloat(card.querySelector('#settings-recall').value);
    if (!Number.isFinite(recall) || recall < 0 || recall > 1) {
      status.textContent = 'Recall threshold must be between 0 and 1.';
      status.hidden = false;
      return;
    }
    try {
      // PUT replaces the whole Config — send every field so omitting one
      // doesn't reset it to the server default.
      await fetch('/api/config', {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ episodic_ttl_days: ttl, recall_min_score: recall }),
      });
      status.textContent = 'Saved.';
      status.hidden = false;
      setTimeout(closeSettings, 700);
    } catch (err) {
      status.textContent = `Save failed: ${err}`;
      status.hidden = false;
    }
  });
  return card;
}

function buildHelpCard() {
  const card = document.createElement('div');
  card.className = 'help-card';

  const h = document.createElement('h2');
  h.id = 'help-title';
  h.textContent = 'Keyboard shortcuts';
  card.appendChild(h);

  const grid = document.createElement('div');
  grid.className = 'help-grid';
  for (const [section, rows] of HELP_SECTIONS) {
    const label = document.createElement('div');
    label.className = 'help-section';
    label.textContent = section;
    grid.appendChild(label);
    for (const [keys, desc] of rows) {
      grid.appendChild(helpKeysCell(keys));
      const d = document.createElement('span');
      d.textContent = desc;
      grid.appendChild(d);
    }
  }
  card.appendChild(grid);

  const footer = document.createElement('div');
  footer.className = 'help-footer';
  const close = document.createElement('button');
  close.type = 'button';
  close.className = 'help-close';
  close.textContent = 'Close (Esc)';
  close.addEventListener('click', closeHelp);
  footer.appendChild(close);
  card.appendChild(footer);

  return card;
}

function helpKeysCell(spec) {
  const cell = document.createElement('span');
  cell.className = 'keys';
  // Keys are joined by ` · ` in the spec; render each chunk as one or more <kbd>.
  const parts = spec.split(' · ');
  parts.forEach((part, i) => {
    if (i > 0) cell.appendChild(document.createTextNode(' · '));
    for (const k of part.split(' ')) {
      const kbd = document.createElement('kbd');
      kbd.textContent = k;
      cell.appendChild(kbd);
    }
  });
  return cell;
}

// ── Bootstrap ─────────────────────────────────────────────────────────────

document.getElementById('refresh').addEventListener('click', reload);

const addBtn = document.getElementById('add');
addBtn.disabled = false;
addBtn.removeAttribute('title');
addBtn.addEventListener('click', startNewFact);

document.getElementById('bulk').addEventListener('click', (e) => {
  e.stopPropagation();
  toggleBulkMenu();
});

document.addEventListener('click', (e) => {
  const wrap = document.getElementById('bulk-wrap');
  if (!wrap.contains(e.target)) closeBulkMenu();
});

document.getElementById('help-btn').addEventListener('click', toggleHelp);
document.getElementById('settings-btn').addEventListener('click', toggleSettings);

document.getElementById('query').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    e.preventDefault();
    onQuerySubmit();
  }
});

// Show the inline × clear button only when the input has content.
{
  const input = document.getElementById('query');
  const clear = document.getElementById('query-clear');
  const syncClear = () => { clear.hidden = input.value.length === 0; };
  input.addEventListener('input', () => {
    syncClear();
    // Emptying the box manually (not via × / Enter) must also exit search
    // mode — otherwise `state.text` stays stale and a later view-tab click
    // re-runs the old search. Only fires on the empty→committed mismatch,
    // so typing a fresh query doesn't reload on every keystroke.
    if (input.value.trim() === '' && state.text !== '') {
      state.text = '';
      reload();
    }
  });
  clear.addEventListener('click', () => {
    input.value = '';
    state.text = '';
    clearFilters();
    renderFiltersBar();
    syncClear();
    input.focus();
    reload();
  });
  syncClear();
}

document.getElementById('sort').addEventListener('change', (e) => {
  state.sort = e.target.value;
  reload();
});

document.getElementById('clear-all').addEventListener('click', () => {
  clearFilters();
  state.text = '';
  document.getElementById('query').value = '';
  renderFiltersBar();
  reload();
});

// Storage-view buttons (Core / Semantic / Episodic / All).
document.getElementById('view-tabs').addEventListener('click', (e) => {
  const btn = e.target.closest('button[data-view]');
  if (!btn) return;
  setView(btn.dataset.view);
});

// Seed filters from URL — drives deep-links from outside the dashboard
// (e.g. the memory skill's "Open in ling-mem console ↗" buttons after
// a hippocampus run). Accepted params:
//   ?since=<ISO ts or YYYY-MM-DD>  → state.filters.since
//   ?session=<source_session id>   → state.filters.source_session
//   ?tier=core|semantic|episodic   → state.view
// Anything else is ignored.
{
  const sp = new URLSearchParams(window.location.search);
  const since = sp.get('since');
  if (since) state.filters.since = since;
  const session = sp.get('session');
  if (session) state.filters.source_session = session;
  const tier = sp.get('tier');
  if (tier && ['core', 'semantic', 'episodic'].includes(tier)) state.view = tier;
}

pollHealth();
setInterval(pollHealth, HEALTH_POLL_MS);

renderFiltersBar();
renderViewBar();
reload();
