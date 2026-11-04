# Backend service contract

`core` owns values, identities and events; `db` owns Turso 0.8.3 and its matching
parser. A session uses a dedicated named worker and current-thread Tokio runtime.
The GUI holds no engine handles. Persistent editor and browse connections are
serialized; generated edits and imports use fresh configured connections. Browse
reads see committed state, independently of the editor's manual transaction.

## File access and lifecycle

Open accepts existing, nonempty regular UTF-8 paths. Canonicalization and engine
calls run on the worker. Read-only is the default and engine-enforced. Both modes
require exclusive use; the acknowledgement does not lock out unrelated writers.
Trusted paths must not be swapped concurrently. Writable opening first performs
read-only schema preflight and explicitly discloses possible DELETE-to-WAL journal
migration. Read-only execution rejects ATTACH and VACUUM, including INTO.

FTS Cargo support and `experimental_index_method(true)` apply to every managed
open; other experimental flags remain off. Native FTS has dedicated coverage;
this is not qualification of every possible experimental index method.
Foreign keys are enabled and verified when connections are configured. Editor SQL
can change its own connection-local PRAGMAs; generated write lanes reconfigure
foreign keys independently.

Commands/events carry database/session/request/generation identities. Pending
request IDs must be unique; callers normally increment from 1. Open/close and
committed invalidations use request zero. Results reject mismatching tickets.
The scheduler accepts 32 requests and at most 16 MiB aggregate pending payload.
Submission never blocks the GUI. Obsolete queued reads can be skipped; editor
ordering is preserved and pending catalog refreshes coalesce. The event queue has
8 slots with backpressure. Discard/disconnect release row-delivery pressure.

Close rejects queued/new work, interrupts an active stoppable read, waits for
remaining work, explicitly rolls back, and drops connections, database and deferred
backing before acknowledging Closed. Cleanup failure is reported. Dropping a
Session is best effort; worker disconnection is UNKNOWN, not an acknowledgement.
Native window close waits for cleanup; it does not silently dismiss a failure.
Changing access requires acknowledged close and reopen.

## Execution and outcomes

Single executions use matching-parser validation; malformed suffixes/multiple
statements fail before execution. Cursor/selection execution uses token boundaries
that preserve comments, strings and trigger/CASE bodies, allowing a valid statement
past an unrelated syntax error. Run, Ctrl+Enter and EXPLAIN share that selection.
EXPLAIN accepts SELECT/INSERT/UPDATE/DELETE and never executes the original SQL
as a fallback. Typed bindings accept positional/named native JSON and tagged
scalars; names include their prefix and missing/extra/mixed bindings are rejected.
Deferred display values are not valid bindings.

Scripts prevalidate at most 100 statements / 1 MiB, provide per-step results and
stop on runtime failure. Sequential mode reports prior committed effects. Atomic
mode requires editor autocommit and rejects embedded transaction controls; commit
or rollback uncertainty remains UNKNOWN. Named parameters are scoped per step;
nonempty positional script bindings are rejected. Cached script results share
8 MiB; selecting one may use another bounded 8 MiB display copy.

SELECT/EXPLAIN delivery stops at limits; discard/close can interrupt the active
read through the pinned wrapper patch. Writes/RETURNING finish when output is
limited or discarded. Every configured connection has a cooperative engine
statement deadline (default 30 s, zero disables, maximum 1 h). Busy timeout defaults
to 250 ms, range 0–5000 ms, with no application retries. Settings apply on reopen.
Neither interruption nor timeout proves rollback. Completions report delivery
(Complete/Limited/Discarded), execution error and observed transaction state
separately. Already decoded buffered rows are flushed on a later execution error.
The UI keeps 100 outcomes, including replaced operations. Unknown application-owned
transaction outcomes block further mutations until close/reopen and inspection.
Elapsed time includes execution, delivery and backpressure, excluding queue wait.

Committed writes emit `Invalidated { schema, source_request }`. Manual writes wait
until commit; full rollback clears pending invalidation. Other retained tiles show
stale rows and require refresh before paging/staging. Schema changes refresh the
catalog/diagram and invalidate inspection metadata. Drafts remain owned by their
original view; arbitrary editor SQL is never rerun automatically.

## Budgets and large values

Query defaults are 1,000 rows / 8 MiB / 64 KiB per cell, with 10,000-row maximum,
512 columns and 1 MiB column metadata. Chunks flush at 64 rows or 64 KiB; one row
can exceed the chunk target. Budgets estimate owned data, not allocator overhead.
Engine allocations and one incoming row are unbounded; there is no hostile-file,
hard memory, hard deadline or hard shutdown guarantee.

Oversized text/blob cells are typed Deferred handles, with original kind/length.
Private anonymous files back at most 64 MiB/value, 256 MiB/session and 1,024 files,
with FIFO expiration. Unsupported sizes/storage failures remain explicitly
unavailable. The OS reclaims anonymous backing on process termination on Linux.
Viewer requests read at most 64 KiB (UI 16 KiB), search the current chunk and save
exact bytes with atomic no-overwrite publication. Deferred originals cannot be
staged or exported as fabricated text. Original SQL is never rerun to obtain them.
Catalog discovery has an independent 10,000-object / 8 MiB budget; tables/views
come first, truncation is visible, and DDL over 16 KiB loads separately on demand.

## Browsing, filtering and metadata

Controlled Browse defaults to 100 rows / 4 MiB. The desktop requests at most
100 rows / 512 KiB, reducing row count at low retention settings. Retention has a
200-row minimum and reserves 100 rows in flight; retained plus pending values stay
within 8 MiB. Byte-limited pages carry an explicit continuation flag. Forward and
reverse paging advance by actual delivered row counts; OFFSET reverse paging
retains a bounded suffix to avoid skipping wide rows. Eviction shares the rendered
row stride and preserves the visible start. Drafts pause paging/eviction.

Ordering uses selected columns plus proven PK/rowid ties. Non-null PKs of up to
8 columns support NULL-aware forward/reverse keyset predicates; other cases use
checked i64-range OFFSET without a hidden million-row clamp. Views and ambiguous
identities retain conservative full-row binary/storage-class ordering. Deferred
cursor values fall back to OFFSET. No snapshot is promised across page requests;
committed changes invalidate cursors. Views, keyless/engine-managed tables,
WITHOUT ROWID and hidden/generated columns remain ineligible for generated edits.

Filters bind typed values, quote identifiers and escape substring LIKE literals.
Alongside flat predicates, AND/OR groups and typed IN lists have depth 8, 128-node,
1,000-binding and 1 MiB limits. Shift-header selection adds sort columns. Row jumps
are explicit offsets; total and filtered counts are explicit reads. Filtered counts
belong to a particular view/result identity, never the global unfiltered total.

Inspection uses supported index/FK metadata with bounded delivery: 512 rows /
1 MiB per metadata query, 128 indexes, 1 MiB combined index details, table/FK
metadata each 1 MiB and names at most 64 KiB. Implicit parent keys resolve through
the ordered PK; ambiguity or truncation fails rather than guessing. Incoming
reference predicates use parent affinity/collation. An indexed IN alternative is
used only for matching declared types and BINARY parent collation. Previews retain
100 rows and probe one more to report Limited; full views use normal paging/counts.
Peek retains one row / 512 KiB and validates current relationship metadata.

Diagram reads retain at most 512 tables / 8 MiB owned metadata, excluding
allocator/engine/temporary overhead. Reserved sqlite objects are excluded; native
engine-managed tables remain inspectable. Failed table inspections remain error
nodes. No row scans or writes are needed. Catalog, diagram and auxiliary context
have separate owners and cannot replace primary rows/draft identity.

Fixed read-only diagnostics cover files, journal mode, page/free counts, integrity
and FK checks. FK validation generates metadata-driven parent-governed anti-joins,
reports per-constraint violations and checked coverage, and errors on incomplete
metadata (limits 1,000 tables / 256 FKs / 1 MiB generated SQL). Expensive checks and
counts are explicit actions; supported statement deadlines still apply.

## Transfers and explicit maintenance

Retained export keeps its labelled window/scope. Complete-table and execute-once
query export stream original values to typed JSONL v2, with exact terminal count,
private staging and atomic no-overwrite publication. Output failure does not stop
a write halfway: execution outcome and file publication are reported separately.
Imports provide bounded previews, explicit mapping/types, prepared inserts and
atomic or opt-in 1,000-row / 8 MiB commit batches. Earlier confirmed batches are
reported after later failure. See `docs/data-formats.md` for record/decoded limits.

Writable-only checkpoint requires editor autocommit and validates native completion
status. Backup checkpoints, closes all source handles, refuses remaining WAL data,
verifies integrity on a private closed-file copy and publishes a new destination.
It never copies a live main file. These operations are explicit, never automatic
repair. Transfer state is bound to its original session.


## Application preferences

`preferences::Store` is a separate application-owned Turso store, used only by the
UI's dedicated storage thread. It does not use inspected-file sessions or alter
their consent/lifecycle rules. The config path is `tursobench/preferences.db` under
XDG config/home. A versioned singleton row stores current/previous JSON snapshots
transactionally, at most 2 MiB each. No JSON-file or legacy-format migration exists.
Read-only preflight rejects unrelated/corrupt/unsupported stores before writable
opening. Failed or uncertain saves disable subsequent writes; external snapshot
changes are never overwritten. Normal shutdown drains coalesced saves. Application
preferences are the explicit exception to existing-files-only opening: this
app-owned database is created automatically. See `docs/data-formats.md`.
