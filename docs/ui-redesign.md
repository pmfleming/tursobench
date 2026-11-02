# Map: the schema is home

TursoBench implements **Direction B** from the simplified redesign proposal.
Opening a database loads an automatically arranged schema map. Select a table to
open its **Data / Structure** drawer; **Expand** gives the drawer the workspace,
and **Return to map** restores the map without changing its camera or selection.
The native Rust parser, typed values, worker ownership, export formats and
staged-commit checks remain authoritative. Turso 0.8.3 is the only engine.

## Navigation and surfaces

| Surface | Capabilities |
| --- | --- |
| Header | Database identity, read-only/editing badge, acknowledged close, schema home, command search, SQL toggle and Tools menu |
| Map / List | Relationship-based table placement; Map has pan, zoom, Find, Fit all, Arrange, key/all-column labels and draggable headers. List includes views, system tables, indexes and triggers |
| Data drawer | Retained rows, 100-row pages, sort, typed filter chips, linked foreign keys, reference trail and Back |
| Structure drawer | Native columns/defaults/keys/indexes/FKs, original table DDL and owned index/trigger definitions, including CHECK clauses and FTS methods |
| Row inspector | Exact stored values/types, cell edits and record detail, outgoing references and “Referenced by” lookups; dock or pop out |
| SQL drawer | Highlighting, completion, single-statement/selection execution, typed JSON bindings, query plan/bytecode, History and Saved |
| Tools / command search | History, Saved, database checks, settings, theme, help, export/copy and edit review |
| Global status | Execution/close state, retained-result scope, staged changes with Review, execution outcomes and uncertain-commit warnings |

**Ctrl K** searches loaded tables, views, columns and commands. Up/Down selects;
Enter activates; Escape closes. Column matches open their table and highlight the
column header. **Ctrl J** toggles SQL. **Alt Left** returns along row references.
Escape closes the inspector, restores an expanded drawer, or closes the drawer.
F1 shows keyboard help. Ctrl Shift C copies visible retained data as TSV.

The SQL editor and table browser share **one retained result**, not independent
query tabs. A query is labelled Query result, EXPLAIN result or Diagnostic result;
it is never presented as the selected table's data. Catalog/map refreshes use
separate tickets and do not replace rows or drafts. Offline SQL editing and
inspection/copy/export of retained results remain available with source labels.
Opening another database clears old results and metadata.

Table/FK navigation cannot discard staged edits. Failed submissions leave the
previous selection intact; stale row events cannot replace a newer result.
Back restores the page/filter/sort or related-row lookup. Related lookups retain
at most 100 rows, without claiming a complete result or automatic counts.

## Schema map and relationships

- Discovery uses native catalog/index/FK metadata, never column-name guesses.
  Parent hubs begin deterministic breadth-first layers within connected groups;
  disconnected groups are packed separately. Cycles and self-links are supported.
- Manual positions, zoom, pan, selected node and column detail are saved per file
  when closing. Reopening starts at the map with the drawer closed. Refresh keeps
  manual positions for surviving objects; Arrange explicitly resets placement.
- Discovery is bounded to **512 tables / 8 MiB owned metadata**. Limits and
  per-table failures remain visible. List has its own bounded catalog and includes
  views and tables beyond the graph cutoff; if that catalog is limited, use SQL to
  inspect additional objects. Views have no invented dependency edges.
- Arrows run from referencing columns to declared targets. Composite, implicit
  parent-PK, cyclic, quoted and Unicode names are supported. Missing or ambiguous
  metadata remains unresolved. “UQ” indicates membership, not individual-column
  uniqueness in a composite/partial index.
- FK cells link only when all key parts are non-NULL and resolved. Multiple FKs on
  a cell are selectable in its context menu. Right-click also exposes inspection
  and editing of FK values.
- Record detail lists incoming relationships from loaded metadata, with an explicit
  notice when cached/incomplete. Clicking performs a bound worker lookup that
  revalidates the FK definition and applies the parent's affinity and collation.
  No related-row counts are fetched automatically. Unresolved and NULL keys cannot
  be followed. Structure displays original CHECK/FTS/trigger DDL without guessing
  parsed badges from text.
- Metadata is a committed-schema snapshot, separate from uncommitted editor DDL.
  SQL execution and generated commits mark the graph potentially stale. Refresh
  explicitly after schema changes. Metadata work has no interruption promise.

## Filters, editing and access

Filter chips combine with AND, with an explicit value type: text, integer, real,
NULL or blob hex. Operators are equality/inequality, less/greater, contains,
is null and is not null. Equality uses IS / IS NOT, including NULL. Contains
escapes `%`, `_` and backslash for literal substring matching; native LIKE case
behavior applies. Up to 32 predicates / 1 MiB of bound values are accepted.
Identifiers are quoted and values are bound. Filters and row values are not saved
in preferences. Row counts remain explicit full scans; paging remains OFFSET,
without snapshot or bounded-execution guarantees.

Opening still requires the existing exclusive-use acknowledgement for **both**
access modes, and the SQLite classification override when applicable. Read-only
is enforced by the engine. The access badge offers a confirmed clean close/reopen;
it waits for Closed before starting the new session, discards local drafts and
rolls back open editor transactions. Cleanup failure or an uncertain commit blocks
automatic reopening. Consent is not remembered across database selections.

Grid edits remain staged, with typed editing, clone/insert, DEFAULT distinct from
NULL, revert, generated SQL/bindings preview and explicit atomic commit. Views,
keyless and engine-managed tables cannot receive generated edits. Editor SQL can
autocommit writes; it does not use the grid's staging mechanism.

Transaction controls appear for writable sessions, while SQL is open, or whenever
an editor transaction is active/unknown—including read-only transactions. Active
transaction state survives hiding SQL. Execution/close/unknown-outcome status and
staged-edit Review remain globally available. Discard suppresses delivery only;
it does not interrupt execution or roll back writes.

## Validation and limits

Workspace tests cover read-only byte preservation, native metadata, bound filters,
reverse FK comparison rules, stale metadata rejection, result/draft ownership,
rapid selection, real composite-FK clicks, Back, palette keyboard navigation,
read-only transactions and clean access switching. Layout tests cover disconnected
512-node graphs, relationships/cycles and both themes at 960×640, 1280×720,
1366×768, 1920×1080 and 3440×1440, plus 1440×960 at 1.5× scale. Drawer headers are
checked in both Data/Structure and expanded/collapsed states.

Desktop smokes exercise map-first opening, table drawers, expand/return, typed SQL,
read-only rejection and acknowledged close against disposable fixtures.
No guessed relationships, schema designer, query tabs, background counts, live
multiprocess opening, or graphical CHECK-expression parser is added. Platform and
production fault-injection gates remain as documented in [release status](release.md).
