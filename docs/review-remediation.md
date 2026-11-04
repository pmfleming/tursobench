# Review remediation

Implementation follows the eight stages agreed on 2026-11-04. Each stage is
committed independently after its targeted checks. This document records actual
delivery and remaining work; it is not a production-readiness claim.

## Stage 1 — correctness and recovery

- Replace the unsupported FK PRAGMA with parent-affinity/collation anti-joins,
  reporting violation counts and checked-table coverage. Metadata failures are
  errors, not clean results. Diagnostic scope is the committed browse connection.
- Give catalog discovery a separate 10,000-object / 8 MiB budget and 1 MiB cell
  budget, prioritize tables/views, and display incomplete delivery. Lazy DDL
  transport is part of the large-value work in stage 5.
- Use rendered row stride, including spacing, for eviction and keyboard scrolling.
- Parse table storage options and reject generated edits without proven rowid
  support, including WITHOUT ROWID, at the shared backend boundary.
- Recover valid preference fields and collection entries, preserve the original
  when recovering, retain a previous-file backup on save, and disable automatic
  persistence for unreadable or newer-version files.
- Flush successfully decoded partial rows when a later engine error occurs.

## Remaining stages

2. File access safety and result freshness.
3. Execution lifecycle, scheduling and verified cancellation/deadlines.
4. Indexed browsing, navigation, typed/grouped filters and reference paging.
5. Deferred large cells, bounded value viewing and lazy DDL.
6. SQL selection/scripts/parameters and typed/bulk editing.
7. Streaming import/export and explicit maintenance.
8. Profiling, fault testing and release qualification.

Stage 1 verification: 147 workspace tests pass, including real orphan and
1,001-index/76 KiB-DDL regressions, recovery and row-stride cases. Workspace
all-target Clippy passes with warnings denied; formatting and diff checks pass.

## Stage 2 — access and freshness

Writable opening now performs read-only schema preflight. The access panel
explicitly discloses that writable opening can migrate journal mode to WAL;
Turso 0.8.3 does not expose a qualified preserve-journal opening mode. This is
an acknowledged writable-open effect, not a no-migration guarantee. Both modes
state that exclusive use is required and read-only access cannot exclude an
unrelated writer. Read-only execution rejects ATTACH and VACUUM (including INTO).

Committed SQL emits a session invalidation event; manual-transaction changes
wait until commit, while full rollback clears pending invalidation. Other tiles
keep their retained rows with a visible stale marker; paging and new staging
require refresh. Schema mutations refresh the catalog and invalidate inspection
metadata. Staged commits invalidate other tiles without discarding their drafts.

Stage 2 verification: all 149 workspace tests, formatting, and all-target Clippy
with warnings denied pass. New regressions cover no-file-output read-only SQL,
manual rollback, committed cross-tile invalidation and automatic DDL discovery.

## Stage 3 — execution lifecycle

A pinned local Rust-wrapper patch exposes existing SDK interruption and statement
wall-clock deadlines. Default deadline is 30 s (0 disables, maximum 1 hour), applied
on reopen to all managed connections. Discard/close interrupt SELECT/EXPLAIN reads;
budget-limited reads stop stepping. Writes/RETURNING still finish when delivery is
suppressed. Interrupt/deadline errors preserve observed transaction state.

The FIFO scheduler accepts 32 requests with a separate 16 MiB aggregate payload
budget. Superseded reads skip execution, catalog refreshes coalesce while pending,
and the UI distinguishes queued/running work. This preserves editor ordering and
provides room for normal multi-tile metadata without an unbounded queue.

Real-engine regressions cover CPU aggregation before the first row, manual
transaction rollback after timeout, atomic interrupted bulk insertion, connection
reuse and 15 queued reads after cancellation. All 151 workspace tests pass.
Formatting and workspace all-target Clippy with warnings denied also pass. The
engine patch provenance, source archive SHA-256 and license are in vendor/turso.

## Stage 4 — browsing and navigation

Browse ordering uses selected sort columns plus proven primary-key/rowid ties,
without the all-column sorter for ordinary tables. Inline INTEGER PRIMARY KEY
rowid aliases are recognized structurally (DESC is excluded). Non-null PKs use
forward/backward keyset paging; nullable keys and views retain an OFFSET fallback.
The silent million-row clamp is removed. Shift-click adds secondary sort columns.

The browser adds row jumps, explicit filter-aware counts and bounded grouped
AND/OR expressions with typed IN lists. Bare numeric literals remain numeric on
untyped/view columns; quote values to compare as text. Incoming-reference full
views now use normal paging and counts; bounded previews probe one extra row and
report Limited. Scalar references use an index-friendly IN query only when parent
and child affinity match and parent collation is BINARY; other cases retain the
parent-governed comparison semantics.

Stage 4 verification: all 154 workspace tests, formatting and all-target Clippy
pass. New cases cover forward/backward seek, NULL/collated multi-column ordering,
251-row reference traversal, truncation status, grouped filters and matching
counts. Real EXPLAIN plans confirm indexed first-page/sorted browsing needs no
temporary B-tree. Concurrent changes invalidate cursors rather than promising a
snapshot across page requests.
