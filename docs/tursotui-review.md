# Functionality review against tursotui

Reference: [`8a4504f75b12ab9102c715b3bf4c73e1cd26b23c`](https://github.com/mikeleppane/tursotui/tree/8a4504f75b12ab9102c715b3bf4c73e1cd26b23c).
Reviewed its README feature descriptions and schema, EXPLAIN and export
implementations. This is a feature comparison, not an endorsement of
its older engine-compatibility statements. No reference code/tests were copied.
**Turso 0.8.3 remains the only engine.** Other engines are not future work.

> This is the earlier feature-comparison snapshot. The subsequent
> [eight-stage remediation](review-remediation.md) supersedes its script, insert,
> reference paging, transfer and cancellation limitations; current behavior is in
> [the guide](guide.md) and [backend contract](../contracts/backend.md).

## Current baseline and gap closure (2026-11-05)

The eight-stage implementation below supersedes the historical matrix that follows.
Current implemented capabilities include sequential/atomic scripts, structured inserts,
reference paging, complete transfers/imports, checkpoint, verified backup, cooperative
read cancellation, column search, and persistent diagram layouts. See the guide for
limits; none of these are outstanding parity work.

Remaining work and acceptance gates:

1. **Baseline**: reconcile this record and PLAN.md. Complete in this stage.
2. **Recovery**: asynchronous, bounded, versioned SQL-document autosave, restore and
   clear/disable controls; restore no bindings, results, mutations or writable session.
   Acknowledged saves survive restart; failed saves retain the last good record.
3. **Completion**: cached schema-aware aliases, qualified columns, JOIN and CTE
   contexts with incomplete-SQL fallback, identifier quoting and scope isolation.
4. **EXPLAIN**: navigable plan, raw/bytecode fallback, scan/index/sort diagnostics and
   conservative index drafts; analysis never executes source mutations.
5. **Profiling**: exact/sampled column statistics and frequencies, explicit sampling
   scope, worker cancellation/deadlines and stale indicators; no unsolicited scans.
6. **Administration**: file/WAL information and curated validated PRAGMA changes,
   with explicit connection scope, writable gating and effective-value readback.
7. **Workspaces**: independent database sessions/documents/transactions/drafts,
   cross-database object search, duplicate-file detection, aggregate resource limits,
   inactive-session routing and coordinated close/quit.
8. **Schema comparison**: snapshot diff for tables, columns, constraints, indexes,
   views and triggers; supported migration drafts and explicit manual guidance for
   unsupported/destructive changes; comparison never writes either database.

Each stage receives focused regression coverage, required repository checks, updated
user documentation and its own commit. Native smoke coverage exercises the new UI
workflows. The existing engine, editing, cancellation and ownership contracts remain
requirements. Production qualification is tracked separately in release.md.

## Historical gap assessment and changes

| Area | TursoBench after this review | Remaining difference / deliberate boundary |
| --- | --- | --- |
| Opening / session isolation | Existing read-only default, explicit writable opt-in, worker-owned editor/browse connections and acknowledged cleanup retained | One local database at a time; tabs/windows remain deferred by PLAN.md |
| Schema browser | Search and grouped tables/views/indexes/triggers; DDL viewer/copy; columns/defaults/PKs; native index and FK metadata; explicit asynchronous row count | Column search/fuzzy cross-database “go to object” and automatic counts are not implemented; counts are opt-in to avoid unsolicited scans |
| SQL editor | Highlighting, native egui text undo/redo, selection/current-statement execution, prefix completion for keywords, Turso functions, schema objects and inspected columns | No alias resolution/context-sensitive ranking or editor-buffer recovery/autosave. Highlighting/completion stop at 64 KiB; execution allows up to 1 MiB |
| Parameters | Typed positional arrays and named objects; exact missing/extra binding validation; named NULL-template generator; driver binding, never interpolation | Mixed positional/named placeholders are rejected. Values are deliberately **not** saved in history/preferences |
| Results | Existing browse sorting/paging retained; header width controls, zebra rows, NULL marker setting, index-lead marks, cell/row/result copy and vertical record detail with conservative JSON formatting | Arbitrary query results are not rewritten to add sorting/filtering; widths now support both drag handles and the header context menu |
| Index awareness | Native declared leading-index/PK markers and possible-scan hints on equality filters | These are metadata hints, not proof an index/method/partial predicate will serve a query; use EXPLAIN |
| Editing | Existing previewed, conflict-checked atomic edits retained; structured typed cell form, draft overlays, per-cell/per-row revert, individual-operation removal, row cloning with keys omitted | Insert input remains a typed array. No arbitrary-result/keyless editing or automatic FK-aware reordering; operations execute in preview order |
| Foreign keys | Native metadata; composite and implicit-parent-PK resolution; typed related-row lookup; bounded back-navigation | NULL/unresolved targets are not guessed. Lookups retain up to 100 rows, back history up to 32 destinations; no snapshot guarantee |
| EXPLAIN | Dedicated query-plan/bytecode actions with bindings; scan-entry notes; guarded against executing source DML | Raw engine rows, not a graphical plan tree; no automatic index recommendations or runtime profile |
| History / bookmarks | Named database-scoped bookmarks with search/rename/delete; searchable timed history, failure/slow flags, slow filtering and sorting; legacy migration | Scope uses the opened absolute path spelling (relative input is resolved against the working directory), not an inode identity: aliases can have separate histories. Legacy entries remain visibly unscoped. No stored bindings |
| Export | Typed JSONL and JSON, annotated CSV/TSV, typed SQL INSERTs; bounded clipboard copy; atomic new-file-only output | Retained results only, not unlimited table export. CSV/TSV are convenience/display formats; see data-formats.md. Staged overlays are not exported |
| Administration | Explicit read-only database-list, journal-mode, page/free-page, integrity and FK checks | No writable PRAGMA dashboard, checkpoint/repair/VACUUM shortcut, or WAL/file-size dashboard. Powerful SQL remains an explicit editor action |
| Desktop usability | File picker/path input, mouse and keyboard actions, help, dark/light theme | Native desktop conventions rather than a recreation of terminal/vim navigation; not every shortcut/panel is configurable |
| ER diagram | Added in the subsequent UI redesign: native tables/keys/FKs, composite/self/cyclic links, zoom/pan/arrangement and browse/structure links | Bounded cached metadata with explicit limited/error states; no schema mutation, persisted layout or inferred relationships |
| Profiling / schema diff | Not implemented | Still deferred, not silently claimed as parity |

This closes practical gaps in the single-database workbench. It is **not complete
tursotui parity**; the remaining differences above are explicit follow-up work.

## Using the additions

The later [responsive UI redesign](ui-redesign.md) reorganizes these capabilities
into activity-rail panels, SQL/Data/Diagram modes and a docked inspector. The modes
share one editor and retained result; execution shortcuts apply in SQL mode only.

- **F1** opens help; **Ctrl+O** opens the database chooser; **Ctrl+T** changes theme.
  The chooser lists local discoveries and recent files before offering native file browsing.
- **Ctrl+Enter / F5** runs the whole editor only if it contains one statement.
- **Ctrl+Shift+Enter** runs one selected statement, or the statement at the cursor.
  Statement boundaries come from Turso's parser, including quoted semicolons and
  trigger bodies. Cursor execution requires the buffer to parse; malformed later
  statements reject the action. No script/batch execution is introduced.
- **Ctrl+Space** offers prefix completions. It will not insert into quoted text,
  comments or parameters; it does not infer aliases.
- **Query plan / Bytecode** explain the whole single editor statement. The helper
  accepts SELECT/INSERT/UPDATE/DELETE only, rejecting PRAGMA/DDL/transaction commands
  before preparation. It never falls back to executing the source statement.
- Click a cell for typed inspection/editing; double-click for record detail.
  Right-click a cell to copy or stage deletion; drag a header edge or right-click it for width.
- The cell inspector offers declared foreign-key links. **Alt+Left** returns to
  the previous page/filter or related lookup. Commit/discard drafts first.
- **Ctrl+Shift+C** copies retained rows as annotated TSV. Copying/exporting does not
  rerun SQL, and does not include staged overlays.

Named parameter example:

```sql
SELECT :label AS label, @payload AS payload, $id AS id;
```

```json
{":label":{"type":"Text","value":"001"},"@payload":{"type":"Blob","value":[0,255]},"$id":{"type":"Integer","value":42}}
```

Positional parameters still use arrays. Repeated named placeholders share the
same value; object keys must include their exact `:`, `@` or `$` prefix.

## Evidence and safety

- `crates/tursobench-db/tests/cases/workbench.rs`: native FTS/ordinary index metadata,
  composite implicit FK resolution, bound related lookups and injection rejection,
  named parameters, EXPLAIN non-mutation, and **all diagnostic actions with
  read-only file-byte preservation**. These use fresh synthetic Turso databases.
- `crates/tursobench-db/src/sql.rs`: parser-based selection with Unicode, comments
  and trigger bodies; parameter recognition ignoring literals/comments.
- UI interaction tests: related lookup/back/NULL handling, metadata/DDL/detail/help
  rendering, statement selection, timed scoped history without saved bindings,
  typed clone/revert controls and existing conflict/rollback behavior; Unicode-safe
  highlighting/completion and preservation of raw JSON numeric/string/key tokens.
- Export tests: typed numbers/NULL/blob, scoped formats, quoting/formula mitigation,
  clipboard budget, no publication/overwrite on failure, and SQL INSERT round trips
  through Turso with quoted identifiers, extreme integers and NUL-containing text.

Turso 0.8.3 returns empty target-column strings for implicit FK references. The
backend resolves those against the parent's ordered declared PK, not a regex or
inferred column name. Ambiguous empty identifiers and missing parent metadata stay
unresolved. This intentionally differs from the old reference's FK-DDL workaround.

Execution timings include worker execution and delivery/backpressure, excluding
queue wait. They are useful history indicators, **not isolated engine benchmarks**.
Metadata/diagnostics/counts run on the browse worker lane and do not observe
uncommitted editor changes. Expensive checks still drain to completion; discard
is not interruption. Existing production safety/release gates remain open in
[release.md](release.md).

## Stage 2 delivery

SQL recovery is implemented in a separate bounded atomic snapshot store, with a
worker, exclusive application-instance lock, version checks, debounce, cursor/text
restoration, and clear/disable controls. Parameters, rows and edits are excluded.
Tests cover restart, deletion, corrupt/locked stores, failed-save preservation and
non-executing UI restoration. Formatting, workspace Clippy/tests, engine spike,
workspace build and all three native smoke scripts passed (2026-11-05).

## Stage 3 delivery

Completion now resolves cached schema aliases, qualified columns, JOIN targets and
CTE names/projections with nested lexical scopes and incomplete-SQL fallback.
Unknown complex expressions are left unresolved. Quoted identifiers and Unicode
remain safe. Scope/CTE regression tests, workspace Clippy/tests, engine spike,
workspace build and all three native smoke scripts passed (2026-11-05).

## Stage 4 delivery

EXPLAIN now has collapsible plan diagnostics, raw/bytecode access and conservative
single-table index drafts checked against cached indexes. Analysis uses the existing
bound, non-mutating backend operation. Real-engine tests execute an index draft,
verify its use and suppress repeat advice. Plan-shape tests, workspace Clippy/tests,
engine spike, build and all three native smoke scripts passed (2026-11-05).

## Stage 5 delivery

Profiles now provide exact or explicitly labeled prefix-sampled column statistics,
NULL/completeness/distinctness, numeric/text metrics and top-value frequency bars.
They use the bounded, cancellable browse execution path; changes mark them stale.
Wide tables select a column; unsupported stddev is an explicit diagnostic row.
Real-engine tests cover exact/sample/keyless/empty inputs, read-only preservation and
unsupported aggregates. Workspace checks, engine spike, build and all native smoke
scripts passed; desktop smoke now executes and renders profiling (2026-11-05).

## Stage 6 delivery

The administration dashboard reports editor/browse/database PRAGMAs and file/WAL
sizes, retaining per-setting unsupported errors. Typed bounded timeout/cache changes
require writable autocommit and read back effective settings on the correct lanes.
Tests prove read-only preservation/rejection, scope isolation and transaction/range
rejection. Workspace checks, engine spike, build and all native smoke scripts passed;
native smoke now executes profiling and the administration dashboard (2026-11-05).

## Stage 7 delivery

DatabaseWorkspace now owns session, documents, transaction/edit state, retained
results, layouts, diagnostics and recovery tracking. Four bounded workspaces can be
open; tabs/Ctrl PageUp/PageDown switch ownership without cloning result buffers.
Ctrl P searches cached objects/columns across databases. File identity deduplicates
aliases. Background sessions drain independently; quit settles all live sessions.
Tests cover inactive results, transaction/draft isolation, duplicate aliases, close,
limits and coordinated quit. Native desktop smoke opens/switches/closes two actual
databases. Workspace Clippy/tests, engine spike, build and all smoke scripts passed
(2026-11-05). Per-session limits multiply up to four; engine memory is not bounded.
