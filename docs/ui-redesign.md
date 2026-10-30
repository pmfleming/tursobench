# Workbench UI redesign

The supplied HTML is **visual direction**, not an implementation of execution,
transaction, binding or editing semantics. The native Rust parser, typed values,
worker ownership, export formats and staged-commit checks remain authoritative.
Turso 0.8.3 is still the only engine; native FTS remains enabled.

## Where to find existing capabilities

| Surface | Capabilities |
| --- | --- |
| Connection bar | Open existing files, explicit exclusive-use acknowledgement, read-only default/write opt-in, acknowledged close, theme and F1 help |
| Transaction bar | Editor transaction state, BEGIN / COMMIT / ROLLBACK; discard **delivery**, pending close and unknown-outcome warnings |
| Schema rail | Searchable tables/views/indexes/triggers, refresh, DDL/copy, columns/defaults/keys/indexes/FKs, explicit row counts |
| SQL editor workspace | Highlighting, prefix completion, single-statement and selection/cursor execution, exact typed positional/named JSON bindings, named NULL templates, query plan and bytecode |
| Data / results workspace | Retained engine rows, page/sort/typed equality filter, PK/index/FK hints, drag/context-menu column widths, zebra rows, NULL/empty text distinction, selected cells, draft overlays |
| Inspector | Exact stored types/values, structured cell editing, cell/row revert, deletion, key-omitting clone, bound FK navigation/back, record detail, conservative JSON formatting; dock or pop out |
| Staged edits | Blank inserts with DEFAULT distinct from NULL, preview of real generated SQL and bindings, individual-operation removal, explicit atomic commit and local discard. Review remains reachable from the footer in every workspace |
| Export and copy retained results | JSONL, JSON, annotated CSV/TSV, SQL INSERTs with explicit target; new-file-only publication, scope labels and bounded clipboard copy; never exports draft overlays or reruns queries |
| History / Saved rails | Scoped timed history, failure/slow flags, slow filtering/sorting, named searchable bookmarks with rename/delete; recalls preserve EXPLAIN and clear bindings |
| Checks rail | Read-only database/journal/page/free-page/integrity/FK diagnostics, on the worker |
| Settings rail | Theme, busy timeout for the next open, query retention, slow threshold, NULL display, font source |
| Outcomes footer | Last execution outcomes, including replaced requests; full status on hover |

SQL editor and Data are **presentation modes over one editor and one retained
result**, not independent query tabs or transactions. The disconnected **Open database**
mode is separate: offline SQL editing/bookmarking and inspection/copy/export of
retained results remain available, with their source labelled. Opening another
file clears the previous retained results/metadata.

Switching modes does not execute SQL or discard drafts. The result header identifies its real source.
Execution/completion shortcuts operate only while the SQL editor is visible;
result-copy shortcuts do not secretly copy a hidden result from Diagram mode.

## Diagram

Choose **Diagram**, then click a table for Structure or double-click to browse.
Drag the background to pan, scroll to zoom, drag headers to rearrange, and use
**Find**, **Fit all**, **100%**, or **All columns**. At overview scales, key labels
collapse; zoom in or use Structure for exact definitions. Fit follows viewport
resizing until you manually pan/zoom/arrange. Refresh/all-columns reflows the layout;
manual node positions are not persisted.

- Discovery comes from Turso's catalog and native metadata, not regexes or guesses
  from column names. Includes disconnected/keyless tables and engine-managed Turso
  tables, excluding reserved `sqlite_*` catalog/statistics objects.
- PK positions, unique-index membership and FK membership identify key columns.
  Structure lists native index metadata and composite FK column mappings; schema-object
  DDL supplies predicates/expressions not returned by native metadata.
  Arrows run from referencing columns to their declared targets. Self-links and
  reverse-direction links use outside/facing ports.
- Composite, implicit-parent-PK, cyclic, quoted and Unicode names are supported.
  Missing/ambiguous target metadata stays explicitly unresolved, never fabricated.
- Uses a separate ticketed response on the existing browse worker. It does not
  overwrite query rows, selected table identity or staged drafts, and performs no
  table scans, row counts or writes. Uncommitted editor DDL is not shown.
- Discovery is bounded to **512 tables / 8 MiB owned metadata**, including container
  capacities but excluding allocator overhead, temporary discovery/inspection data
  and engine allocations. Per-table inspection limits still apply. A limit is
  prominently labelled **LIMITED — not the entire schema**; per-table failures
  remain visible as error nodes. Large graphs still require zoom/search.
- This is a cached metadata view, not a live schema monitor. Subsequent executions
  conservatively mark it potentially stale. Refresh explicitly after schema changes.
  Closing waits for active metadata work too; no interruption promise is added.

Reserved `__turso_internal_` tables are inspectable but cannot receive generated
edits. Finish a manual editor transaction before staging changes; generated commits
continue to use a dedicated connection and never join the editor transaction.

## Laptop and widescreen behavior

The startup viewport is 1280×720. Toolbars wrap rather than clipping ROLLBACK or
other actions. Below 1500 logical points, a docked inspector replaces the sidebar;
**Panels** or a rail button restores the sidebar, and **Inspector** restores the
dock. Below 1100 points the inspector becomes a resizable bottom pane. Widescreens
can keep both panes open. The editor, grid, inspector, insert and commit-review
contents have independent bounded/scrollable regions.

Dark and light themes use restrained teal accents, monospace data and distinct
selection/staged/deleted states. Fontconfig system sans/mono fonts are discovered,
loaded and validated off-thread when available (16 MiB/file maximum). Built-in
fonts remain the fallback. No network font requests or new bundled font assets.

## Validation and boundaries

- Workspace tests exercise native diagram metadata and read-only file preservation,
  composite/implicit/self/cyclic/missing/quoted/Unicode relationships, committed
  schema isolation, the 512-table limit and engine-managed edit protection.
- Egui interaction tests check diagram/result/draft ownership, selection and guarded
  navigation, empty-draft identity reset, offline result/SQL access, and both themes
  at 960×640, 1280×720, 1366×768, 1920×1080 and 3440×1440, plus a 1440×960 input surface at 1.5× UI scale. All three transaction
  controls are checked for viewport containment.
- Native desktop smokes now render table data and the diagram before exercising
  typed SQL, read-only write rejection and acknowledged close.

This is not a port of every prototype feature: independent workspace buffers/tabs,
a command palette, slot-aware graphical parameter forms and persisted diagram
layouts are not claimed. Existing binding JSON remains fully supported. The diagram
adds no schema-mutation actions; manual writable SQL still supports DDL. No graphical
schema designer, profiling, graphical EXPLAIN or schema diff has been added.
Remaining licensing, platform and production fault-injection gates are unchanged;
see [release status](release.md).
