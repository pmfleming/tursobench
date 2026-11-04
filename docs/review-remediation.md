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
