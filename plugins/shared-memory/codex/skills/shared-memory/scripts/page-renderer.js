// Page renderer — takes a page JSON and renders the left panel.
// The model emits ```page blocks; this module draws them.

import { renderTopBarWidget, renderBodyWidget } from './widget-renderers.js';

/** Current page state — persists across partial updates. */
let currentPage = { top_bar: [], body: [], footer: null };

/**
 * Apply a (possibly partial) page update and re-render.
 * If the update only has `body`, top_bar and footer persist.
 */
export function applyPageUpdate(partial) {
  if (partial.top_bar !== undefined) currentPage.top_bar = partial.top_bar;
  if (partial.body !== undefined) currentPage.body = partial.body;
  if (partial.footer !== undefined) currentPage.footer = partial.footer;
  renderPage(currentPage);
}

/** Get current page state (for caching). */
export function getCurrentPage() {
  return currentPage;
}

/** Restore from cached page (on session resume). */
export function restorePage(page) {
  currentPage = page;
  renderPage(currentPage);
}

/** Parse page blocks from model text. Supports both <!--page ... --> and ```page ... ```. */
export function parsePageBlock(text) {
  // Try HTML comment format first (hidden from chat rendering)
  // Allow optional newline after <!--page and before -->
  const commentMatches = [...text.matchAll(/<!--page\s*([\s\S]*?)\s*-->/g)];
  // Fallback: fenced code block format
  const fencedMatches = [...text.matchAll(/```page\s*\n([\s\S]*?)\n```/g)];
  const matches = commentMatches.length > 0 ? commentMatches : fencedMatches;
  if (!matches.length) return null;
  for (let i = matches.length - 1; i >= 0; i--) {
    try {
      // Strip trailing commas before ] or } — models often produce these
      const cleaned = matches[i][1].replace(/,\s*([\]}])/g, '$1');
      return JSON.parse(cleaned);
    } catch (e) {
      console.warn(`[memory] Page block ${i + 1}/${matches.length} invalid:`, e.message);
    }
  }
  return null;
}

// ── Render ──

function renderPage(page) {
  renderTopBar(page.top_bar);
  renderBody(page.body);
  renderFooter(page.footer);
}

function renderTopBar(widgets) {
  const el = document.getElementById('top-bar-area');
  if (!el) return;
  el.innerHTML = '';
  if (!widgets || widgets.length === 0) return;
  const row = document.createElement('div');
  row.className = 'top-bar-row';
  for (const w of widgets) {
    const node = renderTopBarWidget(w);
    if (node) row.appendChild(node);
  }
  el.appendChild(row);
}

function renderBody(widgets) {
  const el = document.getElementById('body-area');
  if (!el) return;
  el.innerHTML = '';
  if (!widgets || widgets.length === 0) return;
  for (const w of widgets) {
    const node = renderBodyWidget(w);
    if (node) {
      node.classList.add('widget-enter');
      el.appendChild(node);
    }
  }
}

function renderFooter(footer) {
  const el = document.getElementById('footer-area');
  if (!el) return;
  el.innerHTML = '';
  if (!footer) return;
  if (footer.text) {
    const span = document.createElement('span');
    span.className = 'footer-text';
    span.textContent = footer.text;
    el.appendChild(span);
  }
}
