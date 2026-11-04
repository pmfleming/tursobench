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

## Stage 5 — large values

Oversized text/blob cells now carry a typed deferred handle; they cannot become SQL
bindings, staged originals or silently exported placeholder text. The worker backs
values from the original execution in private temporary files: at most 64 MiB per
value, 256 MiB / 1,024 files per session with FIFO expiration. Values outside these
limits remain visibly deferred/unavailable. Close drops the backing before its
acknowledgement. This bounds application backing, not the engine's incoming row.

The selected-cell viewer reads bounded 16 KiB text/hex chunks, searches the current
chunk, and atomically saves exact original bytes to a new file. It never reruns SQL.
Large DDL is excluded from catalog payloads and can be loaded separately into a
result/large-value viewer. A browse page has an explicit continuation flag when its
byte budget ends before 100 rows; forward/reverse keyed paging uses actual page
length. Deferred rows require complete originals before staging edits.

Stage 5 verification: all 157 workspace tests, formatting and all-target Clippy
pass. New tests preserve a 100 KB cell at row 50, read/save its original bytes,
refuse overwrite, continue 50 KB-wide rows, and reverse a short byte-limited page.

## Stage 6 — SQL and editing

Run, selection and EXPLAIN share cursor/selection semantics. Lexer boundaries
preserve comments, strings and trigger/CASE bodies while allowing a valid statement
past an unrelated malformed one. Run-all prevalidates up to 100 statements/1 MiB,
then executes sequentially with stop-on-error or in an explicit application-owned
atomic transaction. Atomic mode rejects embedded transaction controls and an active
editor transaction. Named script parameters are scoped per statement. Per-step
results/outcomes are retained within an aggregate 8 MiB budget; the selected result
can use another bounded 8 MiB display copy. Commit/rollback uncertainty is reported
as UNKNOWN, never successful rollback.

Bindings accept ordinary positional/named JSON alongside tagged values, rejecting
out-of-range integers and ambiguous nested arrays. Inserts have a typed form with
separate Default/Omit, NULL and Value choices plus optional tagged JSON. A grid
selection can stage unique-row deletions through the existing atomic review/commit
path. Original positive/negative infinity values can be matched for updates and
deletes; newly supplied non-finite values remain rejected.

Stage 6 verification: all 162 workspace tests, formatting and all-target Clippy
pass, including sequential partial success versus atomic rollback, trigger/script
boundaries, convenient bindings, staged bulk deletion and infinity row matching.

## Stage 7 — transfer and maintenance

Complete-table and execute-once query JSONL exports stream original engine values
without display retention limits. Version 2 headers permit an unknown initial row
count; exact end records and atomic no-overwrite publication remain mandatory.
Write execution finishes even after an output failure, and its actual state is
reported separately from publication. No retained result silently reruns SQL.

Typed JSON/JSONL and CSV/TSV imports provide bounded previews, explicit column/type
mapping, validation, atomic rollback or opt-in confirmed batch commits. CSV quoting,
multiline fields, literal versus unquoted NULL markers and blobs are covered.
Standard CSV/TSV has no metadata preamble and preserves text; annotated and
spreadsheet-protected variants are separate explicit choices. REAL display keeps
1.0 distinct, SQL export checks exact engine numeric conversion, and table headers
recover declared type parameters from parsed DDL.

Explicit checkpoint validates native status. Backup checkpoints and closes the
source, rejects remaining WAL data, verifies a private closed-file copy and
publishes a new destination. The app exposes transfer/maintenance controls from
Diagnostics and retained export, preserving the existing retained-only path.

Stage 7 verification: 166 workspace tests, formatting and all-target Clippy pass.
Integration tests export/import 1,205 rows including a 100 KB value, detect missing
end records, distinguish atomic rollback from 1,000 confirmed batch commits, map
quoted multiline CSV/NULL/blob values, execute INSERT RETURNING only once, and
restore recent WAL data from a verified backup. A 512-value floating-point probe
checks accepted SQL spellings against actual engine result bits. Unknown script,
import and generated-commit outcomes block further mutations until reopen.
