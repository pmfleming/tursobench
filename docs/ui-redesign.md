# TursoBench fixed workspace

The v6 native design adapts the [approved HTML mockup](redesign-v6-fixed-workspace.html)
to egui, using native controls, accessible painter icons and the desktop theme.

## Workspace

Five tiles have fixed positions. Relationships sits above Table data on the left;
Record, Structure and SQL share the right column. Each tile combines its context
and icon actions in one compact ribbon. There is no global ribbon, activity rail,
separate open-tile title bar, window dragging or divider resizing.

- Minimize collapses a tile to its header and gives the other tiles in that column
  more room. Click the header to restore it.
- Expand fills the workspace while preserving the database navigator. Restore
  returns to the fixed arrangement and preserves the other minimized tiles.
- Ctrl Alt Enter expands/restores the focused tile. Ctrl Tab cycles the five tiles.
  Escape dismisses an active edit, popup or dialog before restoring expansion.
- Tile ribbons expose actions directly with 32-point icons and 56-point buttons,
  twice their previous size. Narrow ribbons scroll horizontally; actions never
  collapse into a More menu. Column visibility and open SQL editors have dedicated
  selectors. Tile navigation is not repeated in each ribbon. Restore all tiles
  appears only when a tile is minimized or expanded; review appears in Data and
  Record when their rows have staged edits. The labelled Discard results control
  appears only for an active request; it suppresses delivery without stopping SQL
  execution. Fixed tiles have no close buttons. Tooltips, shortcuts and accessible
  names remain available.
- The SQL ribbon includes a Results group for copying, exporting, fitting and
  selecting result columns, and inspecting the selected result record. These
  controls act on that editor's result buffer, not on the table-data tile.
- The navigator footer opens Tables, History, Saved queries, Checks and Settings
  in the left panel. Query cards use icon actions; checks and preferences use
  compact groups. F1 opens Help. The database name opens recent databases and
  managed open/close actions. See the [navigator controls](guide.md#navigator-tools).

Table browsing selects the data shown in Record and Structure. Space opens the
selected record. Record uses the same live rows and staged edits as Table data;
there is no copied, detached table record. Indexes and triggers can independently
select their original DDL in Structure. FK links use declared metadata, including
composite keys and parent collation/affinity; no relationships are inferred.

SQL keeps its editor and linked results inside one tile. Ctrl J focuses the current
editor; Ctrl Enter runs SQL and Ctrl Shift Enter runs the selection/current
statement. More provides parameters, EXPLAIN, completion, new editors and an
Open editors selector. History and bookmarks load into a new editor. SQL drafts
remain session-only; use Saved queries to keep them across restarts.

The cache retains at most 16 document buffers. Background results route to their
exact request owner. Idle hidden table/result buffers can be evicted, while drafts,
editors and pending requests are retained. All panes share one database session
and transaction. Delivery discard does not interrupt SQL or roll back writes.

## Table data and edits

Filters generate bound predicates against the **whole table**, before fetching the
first matching 100-row page. They never filter only the loaded rows. Type
`column op value` and Enter; quoted identifiers/text, exact integers, reals, NULL,
blobs and AND chips retain their existing semantics. Invalid input leaves the
previous result intact. Header funnels offer sampled values; their labelled
sample counts are not whole-table distributions. Exact table and relationship
counts remain explicit full scans because the engine has no execution interrupt.

Columns measure headers and up to 200 retained values, with bounded automatic
widths. Reorder, hide, fit and resize columns without changing their schema index.
Sorting cycles ascending, descending, off. Scroll fetches bounded pages and evicts
older rows at the retention limit. Drafts suspend paging; stale replies cannot
replace a newer result. Back restores the table's filters, sort and selection.

Status/state text columns automatically use pills. Priority numbers from 0–5 and
progress/percent/percentage numbers from 0–100 receive small bars. Unsupported
values stay plain. The header context menu's Cell appearance submenu overrides
detection per column, including a Plain option. These presentation choices live
with the retained table's column layout for the session; stored values, sorting,
filters and exports remain unchanged. Declared FK cells retain their link styling
and bound navigation.

Arrow keys and Shift extend selection; Ctrl C copies stored values in visible
order, preserving empty fields and exact integers. Double-click or Enter starts
eligible inline editing; Escape cancels. Record supports the same typed staging,
plus copy, clone and deletion actions. Blobs and long values have typed detail.
Generated edits remain restricted to eligible primary-key base tables.

The staged-edit dock offers **Commit**, **Diff / SQL**, and **Discard**. Commit is
an explicit database write and does not require opening review first. Review
shows before/after values, generated SQL and exact typed bindings. The dock is available in both Data
and Record, including expanded Record. Session status keeps pending edits and
editor transaction actions reachable from other tiles. A rejected commit retains
its drafts; an unknown outcome retains them and disables retry/discard. Original
values, optimistic conflict checks and atomic batch semantics remain unchanged.

## Appearance and restoration

Follow system is the default. Linux observes the desktop portal and supported GTK
named colors off the UI thread, including live scheme, accent and contrast changes.
Explicit Light and Dark overrides remain available. Native fonts are reused; the
fallback palette adopts the mockup's green accent. Ribbons scroll as the window
narrows while minimize and expand stay fixed; expanding any tile gives its contents the full
workspace. Record and Structure scroll independently.

Managed close saves visibility, focus, expansion and reopenable view identities
for up to 20 databases. Reopening loads table data and metadata without executing
saved SQL. SQL text, parameters, filters, rows and staged edits are not persisted
in layout preferences. Legacy split-tree layouts migrate to the fixed arrangement.

Native captures use synthetic data: [laptop dark](assets/ui/v6/laptop-dark.png),
[laptop light](assets/ui/v6/laptop-light.png), [minimized tiles](assets/ui/v6/minimized-tiles.png),
[expanded relationships](assets/ui/v6/expanded-map.png) and
[widescreen](assets/ui/v6/widescreen.png).

## Validation

The geometry matrix covers every minimize combination at compact, laptop and wide
sizes, checking containment, separation and expansion. Native egui tests exercise
actual minimize/restore clicks, ribbon bounds across themes and scaling, SQL reruns,
live record edits, direct commit without review, full-table filtering beyond retained
rows, and visibility restoration without persisting query data. Existing ownership,
transaction, paging, export and database-switch tests remain in place.

Use the [desktop checklist](desktop-smoke-test.md) for manual verification.
