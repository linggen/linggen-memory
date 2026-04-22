---
type: spec
audience: implementation — web UI layout, interactions, daemon integration
---

# linggen-memory — web UI spec (v0.1)

The **Data Browser** is the human-facing surface of `ling-mem`. It is served
by the same daemon as the REST API: one `ling-mem serve` process, one origin
(`http://127.0.0.1:<port>`), static assets embedded in the binary.

> **Spec reconciliation complete.** `product-spec.md` and `tech-spec.md`
> previously read "no HTTP daemon in this binary / web UI lives in the
> skill wrapper." That was the pre-Phase-3 plan. Phase 3 landed `src/http/`
> and `src/daemon/`, and this document's build commits landed the UI under
> `static/` served by `src/http/ui.rs`. The sibling specs are now in sync.

## Scope

In: browse, filter, semantic search, add, edit, single-delete, bulk-forget,
bulk-delete-by-selection.

Out: analytics, timeline views, import/export UI (NDJSON round-trip stays CLI-only
for v0.1), multi-user switching, authentication, any write path that is not one
of the seven `Memory.*` endpoints.

## Delivery

- Static files live at `linggen-memory/static/` (flat: `index.html`, `app.js`,
  `styles.css`). No build step, no npm.
- Embedded into the binary via `rust-embed` at compile time.
- Served by axum under `GET /` (index) and `GET /assets/*` (fall-through).
  The `/api/*` routes already exist; the static router is additive.
- Single page. No client-side router — all state is in-memory and URL-less.
  A future revision may add `?filter=…` deep-links; not v0.1.

## Layout

Master-detail. Fixed header; resizable divider between list and detail
(persisted in `localStorage`).

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ Memory · Browse                showing <n> of ≥<n>  ·  ⏻ <daemon status>   │  ← header
├─────────────────────────────────────────────────────────────────────────────┤
│ [🔍 search or /filter:value ……]                [+ Add]   [⋯ Bulk ▾]         │  ← query bar
│ ─ active filters ──────────────────────────────── sort: newest ▾ ────────── │
│  type=preference ✕   context=code/linggen ✕   clear all                     │
├────────────────────────────────────┬────────────────────────────────────────┤
│ LIST                               │ DETAIL                                 │
│  (row cards, checkbox + summary)   │  (editable field panel)                │
│                                    │                                        │
│ [ Load 50 more ]                   │ [ Save ] [ Cancel ]    [ Delete ]      │
└────────────────────────────────────┴────────────────────────────────────────┘
```

## Components

### Header

- **Title** — `Memory · Browse`.
- **Count** — `showing <loaded> of ≥<loaded>` when loaded < limit; `showing <loaded> of <loaded+>` when another page may exist. (No total endpoint.)
- **Daemon status** — polls `GET /api/health` every 15s; shows green `⏻ healthy`, amber `⏻ slow` (>1s), red `⏻ offline` with a retry button.

### Query bar

One textbox. Parse rules (pure function `parseQuery(str) → {filters, text}`):

| Token pattern            | Action                                                |
|:-------------------------|:------------------------------------------------------|
| `/type:<value>`          | Set `filters.type = value` (overwrites existing).     |
| `/context:<value>`       | Append `value` to `filters.contexts`.                 |
| `/from:<value>`          | Set `filters.from`.                                   |
| `/outcome:<value>`       | Set `filters.outcome`.                                |
| `/since:<ISO or yyyy-mm-dd>` | Set `filters.since`.                              |
| `/until:<same>`          | Set `filters.until`.                                  |
| anything else            | Appended to `text`.                                   |

Tokens apply **on Enter**. Matching tokens are consumed and rendered as chips;
remaining text stays in the box (if non-empty) and drives semantic search.
Chips also render above the list with `×` to remove.

**Mode switch is implicit:** `text` non-empty → `search` endpoint; `text`
empty → `list` endpoint. The mode is never a visible toggle.

**Sort select** — `newest | oldest`. Only consumed by `list`; when search is
active the select is disabled (results are score-ranked).

### List

One card per row (see [Row card](#row-card)). Virtualized is out of scope for
v0.1 — `limit` caps the DOM size.

- **Single-click** a row → selects it (detail pane loads).
- **Checkbox** in the row corner → adds to multi-select. A checkbox count
  appears in the Bulk menu: `Delete selected (3)`.
- **Keyboard** — see [Keyboard](#keyboard).
- **Load 50 more** button at the bottom. Fires the same request with
  `offset += 50` (see [Backend delta](#backend-delta)); results append.
  Hides when the last response returned fewer than `limit` rows.

#### Row card

```
┌──────────────────────────────────────────────┐
│ ☐  <type>    <from>   <outcome?>  · <age>    │
│    <content, 2-line clamp>                   │
│    <context chips>   <tag chips, first 3>    │
└──────────────────────────────────────────────┘
```

- Left border tint by `type` (reuse dashboard tokens: `--added`, `--updated`,
  `--replaced`; four new tokens for the remaining four types — see
  [Design tokens](#design-tokens)).
- `<outcome?>` renders only for `tried` / `fixed` types: ✓ positive,
  ✗ negative, — neutral.
- `<age>` relative: `Nm` (minutes), `Nh`, `Nd`, `Nw`, `Nmo`. Hover tooltip
  shows absolute `created_at`.
- Selected row: lighter background + left accent bar.

### Detail pane

Populated when a row is selected. All fields editable except `id`,
`created_at`, `source_session`. Empty state (no selection):

> Select a fact on the left to see details, or click **+ Add** to create one.

Field layout (top to bottom):

| Field       | Control                                               | Notes |
|:------------|:------------------------------------------------------|:------|
| `content`   | `<textarea>` auto-grow                                | required |
| `contexts`  | chip editor (type to add, `×` to remove, `Enter`/`,` commits) | |
| `tags`      | chip editor                                           | prefix convention visible as placeholder text |
| `type`      | `<select>` over 7 `FactType` values                   | |
| `from`      | `<select>` over 3 `Origin` values                     | |
| `outcome`   | `<select>` — `—, positive, negative, neutral`         | `—` sends `clear_outcome: true` |
| `cwd`       | `<input>` with a × to clear                           | clearing sends `clear_cwd: true` |
| `occurred_at` | text input (ISO 8601 or `YYYY-MM-DD`) + clear; **add-only** for v0.1 | see note below |
| `created_at`  | read-only, absolute UTC                             | |
| `source_session` | read-only, truncated with copy button            | |
| `id`        | read-only, truncated with copy button                 | |

**`occurred_at` is add-only.** The server's `UpdateRequest` does not
expose `occurred_at` or `clear_occurred_at`, so the edit form renders it
as a read-only timestamp (mirroring `created_at`). New facts can set it
via `add`. Full edit symmetry would require a ~10-line backend patch to
add `occurred_at: Option<DateTime<Utc>>` and `clear_occurred_at: bool` to
`UpdateRequest` and `FactPatch`; deferred until a user actually needs it.

**Chose text input, not `datetime-local`.** Datetime-local surfaces values
in the browser's local timezone and round-trips are error-prone against a
UTC store. A plain text input accepting ISO 8601 or bare `YYYY-MM-DD` is
clearer and matches the wire format exactly.

**Dirty tracking.** Detail state has `original` + `edited`. `Save` enabled
only when they differ. Navigating to another row with unsaved changes opens
a confirm: *Discard changes? [Discard] [Keep editing]*.

**Save.** `POST /api/memory/update` with the minimal diff (only changed
fields present); response replaces `original` and the row in the list.

**Delete.** Confirm dialog quoting the first 80 chars of `content`.
`POST /api/memory/delete`. On success, removes from list, clears detail.

### Add flow

**No modal.** `+ Add` loads a blank draft into the detail pane with
`content` focused. Save triggers `POST /api/memory/add`; the new row
prepends to the list and becomes selected. Cancel discards the draft.

Rationale: modals break keyboard flow and duplicate the detail-pane field
layout. One source of truth.

### Bulk menu

Dropdown from the `⋯ Bulk ▾` button:

| Item                              | Enabled when              | Action                                      |
|:----------------------------------|:--------------------------|:--------------------------------------------|
| `Delete selected (N)`             | N ≥ 1                     | Confirm, then N× `POST /api/memory/delete`. |
| `Forget by current filter…`       | ≥1 chip present           | Confirm, then `POST /api/memory/forget`.    |
| `Clear selection`                 | N ≥ 1                     | Clears checkboxes.                          |

**Forget confirm** shows the filter and the result of a pre-flight
`list` with the same filter: *"This will delete 14 facts matching
`type=preference, context=code/linggen`. This cannot be undone. [Forget] [Cancel]"*.
No partial undo.

**Delete selected** fans out client-side; on any failure, report which
rows did not delete and leave them in the list. Do not retry automatically.

## States

| State           | Trigger                        | Rendering                                                 |
|:----------------|:-------------------------------|:----------------------------------------------------------|
| loading         | request in flight, list empty  | 5 skeleton cards                                          |
| empty (no data) | response 0 rows, no filters    | illustration-less message: "Your memory is empty. Run a curation from the dashboard, or click **+ Add**." |
| empty (filtered)| response 0 rows, chips present | "No facts match. [Clear filters]"                         |
| daemon-down     | `/api/health` fails ×2         | full-width red banner at top with retry; list greyed out  |
| request error   | non-`/api/health` 4xx/5xx      | toast (top-right, 5s, dismissible) + inline red in detail for save failures |

Toast content: `error` string from the envelope + `code` in monospace badge.

## Keyboard

| Key         | Action                                                    |
|:------------|:----------------------------------------------------------|
| `/`         | Focus the query box.                                      |
| `Esc`       | Blur query box → clear selection → exit editing (in order; one level per press). |
| `j` / `↓`   | Select next row in list.                                  |
| `k` / `↑`   | Select previous row.                                      |
| `g g`       | Jump to first row.                                        |
| `G`         | Jump to last loaded row.                                  |
| `x`         | Toggle checkbox on selected row.                          |
| `e` / `Enter` | Focus `content` textarea for edit.                      |
| `⌘⏎` / `Ctrl+Enter` | Save edits (when in detail pane).                 |
| `⌫` / `Del` (in list) | Delete selected row with confirm.               |
| `n`         | Start a new fact (= `+ Add`).                             |
| `?`         | Toggle keyboard help overlay.                             |

Modifiers follow platform conventions — `⌘` on macOS, `Ctrl` elsewhere.
Handlers are suppressed while focus is inside a text input except for
`Esc`, `⌘⏎`, and `⌘S` (which triggers Save).

## Design tokens

Reuse the dashboard palette in `skills/memory/index.html` so the two
surfaces are visually one app. Add four type-tint tokens (currently only
`added` / `updated` / `replaced` exist):

```css
--t-fact:       #7aa2f7;  /* blue   — neutral stable */
--t-preference: #bb9af7;  /* purple — behavioral rule */
--t-decision:   #7dcfff;  /* cyan   — reasoning */
--t-tried:      #e0af68;  /* amber  — attempt */
--t-fixed:      #9ece6a;  /* green  — resolution */
--t-learned:    #73daca;  /* teal   — env/tool */
--t-built:      #f7768e;  /* red    — shipped */
```

Used for the row card's left border and the type chip's background tint.

## Endpoint mapping

| UI action              | Request                                                                 | Response handling |
|:-----------------------|:------------------------------------------------------------------------|:------------------|
| Filter browse          | `POST /api/memory/list {filters, sort, limit, offset}`                  | Replace list if `offset=0`, append otherwise. |
| Semantic search        | `POST /api/memory/search {query, ...filters, limit}`                    | Replace list. Sort select disabled. |
| Open row               | `POST /api/memory/get {id}` (refresh) — optional; list payload is enough on optimistic path | Populate detail. |
| Save edits             | `POST /api/memory/update {id, ...diff, clear_outcome?, clear_cwd?}`     | Update row + detail. |
| Add                    | `POST /api/memory/add {content, ...fields}`                             | Prepend to list; select. |
| Delete                 | `POST /api/memory/delete {id}`                                          | Remove from list; clear detail if matched. |
| Forget by filter       | `POST /api/memory/forget {filters}` (preceded by pre-flight `list`)     | Rerun current list request. |
| Daemon health          | `GET /api/health` every 15s                                             | Update header pill. |

All POSTs use `Content-Type: application/json`. Responses are unwrapped
by a `callApi(path, body)` helper: on `{ok:false}`, throw an `ApiError`
with `{message, code}`; handlers decide toast vs inline.

## Real-time / refresh

No WebSocket, no SSE in v0.1. A manual `↻` button in the header reruns
the current request. The curation dashboard (other surface) adds rows
without notifying this UI; the user hits refresh when they want to see
them. Acceptable because the browser is for *auditing*, not monitoring.

## Backend delta

**One change, one file.**

```rust
// src/http/memory.rs — ListRequest
#[derive(Debug, Deserialize)]
pub struct ListRequest {
    #[serde(flatten)]
    pub filters: FilterDTO,
    #[serde(default)]
    pub sort: SortDTO,
    #[serde(default = "default_list_limit")]
    pub limit: usize,
    #[serde(default)]         // NEW
    pub offset: usize,        // NEW — 0 means "first page"
}
```

Plumbed into `FactsStore::list` by widening the signature:

```rust
pub async fn list(
    &self,
    filters: &Filters,
    order: SortOrder,
    limit: usize,
    offset: usize,          // NEW
) -> Result<Vec<Fact>>
```

v0.1 implementation: fetch `limit + offset`, sort in-process (as today),
return the slice `[offset .. offset + limit]`. LanceDB's `query().limit()`
has no native offset; we pay the over-fetch until Phase-4 scaling work
revisits list pagination. Documented in `tech-spec.md` under storage.

No other API shape changes.

## Static-asset serving

Actual implementation (see `src/http/ui.rs` and `src/http/mod.rs`):

```rust
// src/http/ui.rs
#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/", get(serve_index))
        .route("/assets/{*path}", get(serve_asset))  // axum 0.8 nested-path syntax
}

// src/http/mod.rs
pub fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/api/health", get(health::handler))
        .merge(memory::router())
        .merge(ui::router())
        .with_state(state)  // state attached after merge so the UI router's
                            // typed state parameter unifies with the API's
}
```

Two syntax notes that tripped up the draft:

1. **Axum 0.8** spells nested path captures as `/assets/{*path}`, not
   `/assets/*path` (the older style). The `{*}` wrapping plus the `path`
   param name maps to an `axum::extract::Path<String>` in the handler.
2. `with_state` must come **after** `merge(ui::router())`; merging typed
   `Router<SharedState>` into an untyped `Router` before attaching state
   fails type-unification.

New deps: `rust-embed = "8"`, `mime_guess = "2"`. Both pure-Rust, no build
tooling. **Note on the `debug-embed` feature:** `rust-embed`'s default
behaviour already reads from disk in debug builds and embeds in release —
which is exactly what we want. The `debug-embed` feature flag *forces*
embedding even in debug builds, so we leave it **off**. Edits under
`static/` are picked up on the next request without recompile.

## File layout after this change

```
linggen-memory/
├── src/http/...                  # unchanged + router extension
├── static/                       # NEW
│   ├── index.html
│   ├── app.js                    # vanilla ES modules, no bundler
│   └── styles.css
└── doc/ui-spec.md                # this file
```

No npm, no Vite, no TypeScript. Single-crate discipline extends to the
frontend: one folder, three files, edited by hand.

## Accessibility

- Query box, buttons, and chip `×` are keyboard-focusable. Chips render
  as `<button>` not `<div>`.
- Row cards have `role="option"` inside an `aria-activedescendant` list.
- Detail inputs labelled with `<label for>`.
- Focus outlines preserved — no `outline: none`.
- Minimum contrast: text ≥ 4.5:1 on its background (tokens already meet).

## Spec updates

**Shipped alongside the UI commits** — `product-spec.md` and `tech-spec.md`
have been amended to match reality:

- `product-spec.md` — "Browse (human)" mode now describes the Data Browser
  served by the daemon; the skill-wrapper language has been removed; the
  HTTP-daemon-is-out-of-scope bullet is gone.
- `tech-spec.md` — repo tree includes `static/`, `http/`, `daemon/`,
  `sessions/`; `serve`/`start`/`stop`/`restart`/`status` are documented as
  in-scope daemon lifecycle commands; "Skill integration" rewritten to
  reflect that the binary ships the UI and the skill wrapper is a thin
  launcher pointing at the daemon port.

## Out of scope for v0.1 (flagged for future)

- Deep-link URLs (`?type=preference&context=code/linggen`).
- Virtualized list for > ~1000 loaded rows.
- Row-hover quick-actions (star, pin) — schema doesn't support them yet.
- Multi-select across pages (selection resets on "Load more").
- Markdown rendering in `content` — render as plain text for v0.1 (backticks
  show literally; predictable).
- Light theme — dark only, matching the dashboard.
- Import / export UI — use the CLI (`ling-mem list > facts.ndjson` /
  `ling-mem add --stdin < facts.ndjson`).

## Build order

1. Backend delta: `offset` on `ListRequest` + `FactsStore::list` signature
   + two-line slice in `list()`. One commit. Covers unit test for pagination.
2. Static asset plumbing: `rust-embed` + router routes + a placeholder
   `static/index.html` returning `"Memory"`. One commit.
3. Frontend skeleton: layout + daemon-health pill + empty `list` wiring. One commit.
4. Query bar parser + filter chips + sort + infinite-scroll list. One commit.
5. Detail pane (read-only). One commit.
6. Edit + save + add + delete. One commit.
7. Bulk actions + forget confirm. One commit.
8. Keyboard map + help overlay. One commit.
9. Spec-file cleanup per [Spec updates](#spec-updates). One commit.

Each step compiles and runs; a human can open `http://127.0.0.1:<port>`
at any point and see a working (if partial) browser.
