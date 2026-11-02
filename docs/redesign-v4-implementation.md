# v4 implementation preparation

Prepared against `e4ef7a9`, the revision reviewed by
[redesign-v4-controls.html](redesign-v4-controls.html). This is an implementation
plan with the completed stages recorded below; the application implements the v4 controls and grid changes, with automatic counts deferred at the execution gate. The HTML supplies the intended
placement and interactions. Existing backend and editing contracts supply the
execution, ownership and safety rules.

## Working decisions

Use the document's recommended activity rail, scroll paging and default docked
inspector. Treat the following as implementation defaults, subject to product
feedback:

- A 44-point rail owns Tables, Map, SQL, History, Saved, Checks, Settings and Help.
  SQL is an independent toggle; Map and Settings select main-area routes;
  History, Saved and Checks select sidebar content. Tables restores the tree
  without rerunning a query. Map keeps the tree available.
- Inspector defaults to an overlay below 1440 logical points, a 320-point dock
  from 1440, and a 360-point dock from 1920. At 2400, add the related-record split.
  Remember the inspector visibility choice per database. Breakpoints use egui
  logical points, so high-DPI rendering does not misclassify a physical monitor.
- Scroll fetches retain 100-row engine pages with a bounded rolling window.
  Scrolling backwards can reload evicted pages. The readout reports the retained
  absolute range; it cannot always start at 1 after eviction.
- Initially show unknown tree counts as `—` and count on demand. The document's
  recommended automatic counts require a separate execution-budget milestone:
  the current worker cannot time out a full scan. Do not call a display retention
  limit or a UI timer an execution budget.
- Keep Ctrl E for Export, Ctrl J for SQL, F1 for Help and Ctrl K for the palette.
  Ctrl F focuses the token filter. F5 acts on the focused scope. Arrow keys and
  Space act on the grid only when text editing or a popup does not own input.
  Enter follows an unambiguous FK; otherwise it starts an eligible cell edit.
  Double-click/context menu remains the explicit edit path for FK cells.

## Source-to-code map

Paths below are relative to `crates/tursobench-ui/src/` unless prefixed otherwise.

| Work | Current implementation | Required change |
| --- | --- | --- |
| Shell and surfaces | `app/shell.rs`: `ToolPanel`, `Nav`, `UiState`, `Layout`, `shell` | Add rail and Settings route; compose sidebar, inspector and main splits without overlapping panels |
| Sidebar | `app/sidebar.rs`: `navigation_sidebar`, `sidebar_object`, `sidebar_table`, `tool_window` | Replace footer/windows with sidebar views; one Find field; full-width tree rows and disclosure targets |
| Tool bodies | `app/usability.rs`, `app/schema.rs`: history, saved, settings, diagnostics | Reuse existing behavior in the new surfaces, including search, bookmark management and parameter clearing |
| Table bar/status | `app/shell.rs`: `workspace_header`, `page_controls`, `status_bar` | Fixed labelled segments, token field, range/count link, scoped refresh, inspector toggle, rare-action menu; one completion summary |
| Grid presentation | `app/grid.rs`, `app/tools.rs` | Autofit/manual widths, column presentation order/visibility, selection, facets, inline insert/edit |
| Filters | `app/filters.rs`, `app/results.rs`: `CellKind` | Token parser and editable typed chips; exact-value context filtering |
| Inspector/Structure | `app/inspector.rs`, `app/schema.rs`: `structure_inspector` | Compact fields, expandable long values, relation counts, responsive Structure columns |
| Routing and events | `app.rs`: `submit`, `handle_event`, `browse_table`, `reload`; `app/schema.rs`: navigation restoration | Separate page append and auxiliary reads from replacing the primary result |
| Commands and focus | `app/commands.rs`, `app/editor.rs`: `shortcuts` | Keep palette actions reachable; scope shortcuts, popup Escape and grid focus |
| Persistence | `storage.rs`: `Preferences` | Add backward-compatible, bounded per-database inspector preferences; never persist inspected/filter values |
| Engine support | `crates/tursobench-core/src/lib.rs`; `crates/tursobench-db/src/{lib,worker,browse,inspection}.rs` | Add inclusive comparisons and auxiliary lookup/count operations as needed; preserve typed bindings and ticket isolation |

## Dependencies to settle in code

### Result ownership and paging

`App::submit` currently starts a fresh `ResultSet`, clears selection/scroll and
changes the route on every submission. `ResultSet::start` discards previous rows.
Calling this path from the scroll callback would erase the grid at every page.

Introduce a browse-window state containing query identity, absolute first-row
offset, contiguous page range, pending page ticket, end-of-data state and retained
byte/row totals. The query identity includes session, table, typed predicates and
sort. Changing identity invalidates pending pages and resets the window.

Use a separate temporary result for the pending page. Validate ticket, identity
and column metadata before appending/prepending a completed page. Deduplicate
requests triggered by repeated frames; apply backpressure to the worker's
two-command queue. A short **complete** page ends the sequence; Limited, Failed
or Discarded delivery must not imply end-of-table. An exact multiple of 100 may
need one final empty fetch. Preserve the existing maximum OFFSET of 1,000,000.

Bound the combined page window, including pending data, by row and byte limits
(initial target: the configured retention row limit, at most 10,000 rows and
8 MiB; 64 KiB per cell). Evict whole pages away from the viewport, adjust scroll
position, and never evict draft-owned rows. Pause page replacement/fetching while
drafts or a staged commit exist. Preserve or clear selections by retained row
identity, never by a reused local row index. Back restores the query/window and
matching selection after reload. Exports describe the retained window only.

### Auxiliary reads and counts

FK peeks, related counts and the wide related grid must have their own tickets
and bounded retained data. Existing `Lookup`/`References` use the primary browse
submission path; invoking them through `submit` from a hover would navigate away
and replace the parent grid. Reuse their validated query generation behind an
auxiliary result owner instead. Background completions must still report genuine
editor transaction state without replacing primary result status.

Debounce peeks, allow one pending auxiliary request at a time, bound caches by
entries and bytes, and key entries by session, metadata revision, relationship
and typed key tuple. Ignore stale selections; invalidate on writes, schema
refresh and reopen. Revalidate declared relationships, including composite keys,
implicit parent PKs, parent affinity/collation and self-links. NULL or unresolved
keys do not trigger reads. Use `—` for an unknown count, never a fabricated zero.

The current `Operation::Count(String)` counts the whole table, not the active
filter. A filtered range must use an explicitly filtered count or label a cached
total as a table total. Related counts also need a dedicated count operation;
the length of a capped 100-row lookup is not an exact count.

Automatic counts are a gated follow-up: establish a cancellable, isolated
read-only execution mechanism first, test that an expensive count cannot occupy
the interactive worker indefinitely, then add idle scheduling and timeout states.
Until then, keep counts explicit. The current busy timeout only limits lock waits;
discard suppresses delivery and does not stop execution.

### Typed values, columns and edits

- Parse `column op value` locally into existing `Filter` values; quote identifiers
  and bind values through `browse_sql`. Support quoted identifiers, whitespace,
  operator precedence (`>=` before `>`), `!=`/`≠`, NULL predicates, quoted text,
  empty text, Unicode, signed numbers and explicit blob hex. Errors retain input
  and leave the current query unchanged. Declared affinity supplies a default;
  the chip editor retains an explicit storage-type override for mixed data.
- Add `LessOrEqual`/`GreaterOrEqual` to `FilterOp`, SQL generation and backend
  tests: the prototype offers `<=`/`>=`, but the application currently does not.
  Preserve existing `IS`/`IS NOT` equality and literal-substring escaping.
- Cell filters use the stored `Value`, never its truncated preview or formatted
  string. Filter changes respect draft guards. Facets initially summarize loaded
  retained values and label that sample; they do not imply whole-table counts.
- Store widths, order and hidden state separately from canonical schema column
  indices. Reordering/hiding must not change edit keys, filter/sort targets,
  composite-FK tuples, export order contracts or column-jump behavior. Reconcile
  presentation state after schema changes and provide Show hidden columns.
- Autofit measures header adornments and up to 200 retained values, with initial
  widths clamped to 56–300 points. A non-manual long text column flexes into
  remaining space with a 160-point minimum. Keep manual widths during fetches;
  double-click refits. Measure bounded previews and cache measurements.
- Inline editing uses existing `CellKind` parsing and `stage_cell`; Enter stages,
  Escape cancels, and neither writes SQL. Retain a full typed editor for blobs
  and long values. The `+ new row` line opens the existing typed insert flow so
  DEFAULT remains distinct from NULL. Existing eligibility/undo/review/commit
  rules continue to apply.
- Selection stores anchor/cursor using stable row and canonical column identity,
  while visual ranges follow visible column order. Copy uses exact typed values.
  Numeric aggregates use numeric cells only, expose the numeric count, and handle
  integer overflow/nonfinite values explicitly rather than silently losing
  integer precision through `f64` conversion.

## Implementation sequence and acceptance

1. **Rail and routing.** Add rail, sidebar tool views, database-name menu and
   Settings page; remove tool windows/footer and redundant Map/List controls.
   Tool/view changes preserve primary result, drafts, SQL text and map camera.
   Keep catalog completeness/error information available in the tree.
2. **Header and responsive panels.** Stable Data/Structure segments, one Find
   field, scoped refresh, rare-action menu, compact status, logical breakpoints,
   overlay/docked inspector and per-file visibility. Render SQL under the main
   grid, excluding the inspector. Structure uses two columns from 1920 points.
   Fit and Arrange remain accessible map commands with one zoom readout.
3. **Column presentation.** Autofit/refit, manual width tracking, drag reorder,
   hide/restore and accessible menu alternatives. Sort cycles ascending,
   descending, off. Hidden/reordered columns still target correct stored values.
4. **Filters and facets.** Token field, editable/removable chips, empty-field
   Backspace, exact-value Filter/Exclude/NULL actions and sample facets. All
   routes use the same typed filter application path and reset paging safely.
5. **Selection and inline edits.** Cursor, drag/Shift selection, keyboard paths,
   selection copy/aggregates, compact record fields and typed inline staging.
   FK click, selection drag and resize/reorder gestures must not conflict.
6. **Bounded scroll paging.** Implement separate page tickets and rolling window,
   forward/back fetch, retry/end states, range/count readout, scroll preservation
   and draft pause behavior. Replace explicit pager buttons only after this path
   passes ownership, stale-event and retention checks.
7. **Relationship context.** Cached FK peeks, counted Related rows and generic
   wide master/detail split driven by declared metadata. Parent and child results
   remain independent; changing the selected parent invalidates old child reads.
   Keep SQL and related splits usable together through minimum heights/toggles.
8. **Automatic count gate and final integration.** Implement the isolated timeout
   mechanism before enabling background counts. Finish focus/AccessKit paths,
   persistence migration, smoke coverage and documentation. Record automatic
   counts as deferred if the execution gate is not met; do not claim full v4
   completion in that case.

Each increment must leave the app usable. Start with steps 1–2; do not combine
surface relocation with a primary-result ownership rewrite in the first patch.

## Contract and prototype reconciliations

- Hide idle transaction commands, including when SQL is open. An active
  read-only transaction remains real and must offer Commit/Roll back with SQL
  hidden. `Unknown` remains an explicit recovery state, never ordinary success.
  BEGIN remains available through SQL/the palette, not an idle status button.
  Update tests that currently expect all three transaction buttons while active.
- Status keeps staged Review and execution/close/unknown feedback available.
  Remove duplicated timing text, not the outcome log or delivery-limit states.
- The HTML's Map refresh, More-menu count and SQL-toggle entries duplicate other
  visible controls. Follow the written one-home rule: schema refresh in the
  Tables group menu, count in the range readout, SQL toggle in the rail; keyboard
  and palette access remain. Retain capability through scoped menus if narrow
  layout genuinely removes the normal control.
- The HTML is an interaction reference, not executable engine semantics. SQL
  results, clipboard/export, access changes and several actions are mocked;
  relationship data is hardcoded to the sample. Preserve real SQL selection,
  exports, access acknowledgements and metadata-derived relationships.
- Keep original DDL and native metadata, existing managed close/reopen, staged
  writes and draft guards. No synthesized schema definitions or guessed links.

## Verification when implementation starts

Extend existing interaction tests for behavior/ownership rather than duplicating
paint code. Cover route changes with drafts, read-only active transactions,
typed token parsing and inclusive comparisons, canonical column mapping,
selection/focus, exact copy, page eviction/reload, stale/failed page delivery,
auxiliary stale events, composite FK peeks/counts and old preference migration.

Check both themes at 720×480, 960×640, 1280×720, 1366×768, 1440×900,
1600×900, 1920×1080, 2560×1440 and 3440×1440 logical points, plus high DPI.
Exercise SQL shown/hidden, Data/Structure, inspector states, long names and large
schemas. Retain the existing 15-row floor at 1280×720. Target the document's
roughly 40 visible rows and 11 sample columns at 1920×1080 with SQL closed,
using fitted columns and a docked inspector; verify against the fixture rather
than treating this as a guarantee for every schema.

Use the pinned environment (`cargo` is not on this shell's default PATH):

```sh
nix develop -c cargo fmt --all --check
nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
nix develop -c cargo test --workspace --locked
nix develop -c cargo run --locked -p engine-spike
nix develop -c cargo build --workspace --locked
nix develop -c bash scripts/desktop-smoke.sh
nix develop -c bash scripts/discovery-smoke.sh
nix develop -c bash scripts/local-project-smoke.sh
```

Update `docs/ui-redesign.md`, `docs/desktop-smoke-test.md`, relevant backend/data
format contracts and README claims when behavior ships. Preparation itself does
not assert that runtime tests or visual acceptance have passed.

## Implementation record

Stages 1–7 are committed separately: rail/routing, responsive controls, canonical
column presentation, typed filtering/facets, selection/inline staging, bounded
scroll retention, and independently owned relationship context. Stage 8 integrates
focus, real keyboard Enter/clipboard handling, compact defaults, cache invalidation,
access review and updated contracts, with workspace and native smoke validation.

Automatic background counts are **deferred**. Inspection of pinned Turso 0.8.3's
public Connection API found no interruption/progress hook; busy_timeout is only a
lock-wait budget. Adding automatic COUNT to the shared worker would fail the
execution-budget gate. Explicit table and related counts are implemented. This
is not a claim that the automatic-count portion of v4 is complete.

Browse retention uses a 200-row minimum to reserve one 100-row fetch in flight;
the retained window reserves 4 MiB and the pending page another 4 MiB. SQL still
uses the exact configured row limit. The wide related preview shows at most 12
columns and links to the full view. These practical limits are explicit in the UI
guide and backend/data-format contracts.
