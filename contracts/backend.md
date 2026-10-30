# Backend service contract

`core` owns values, identities and events; `db` owns Turso and the matching parser.
`Session::open` starts one named worker. Canonicalization, schema validation and
all engine calls happen there. Existing nonempty regular UTF-8 paths only.
The preflight check is not protection against a malicious concurrent path swap.
Use trusted paths not being modified by another program.

Turso's `fts` Cargo feature and experimental index-method builder flag are enabled.
The same database capability configuration applies to read-only/read-write opens
and all managed connection lanes. Native `USING fts(...)` indexes and `fts_match`
queries are supported; other experimental flags remain disabled. Index creation
and writes still require engine-enforced read/write access. The browser's typed
column filter remains equality-based; use the SQL editor for full-text queries.

A session has persistent serialized editor and browse connections. All commands
carry database/session/request/generation identities. Request numbers must be
monotonically increasing from 1. ResultSet rejects events with a different ticket.
Open/close events use request 0. GUI must not forget active transactions when
replacing query results. Browse/schema cannot see editor uncommitted changes.

Command queue: 2. Event queue: 8. Submit never blocks GUI. Worker blocks under
backpressure; discard and disconnect release row-delivery pressure. Chunks flush
at 64 rows or 64 KiB (one row may exceed chunk target). Retention is capped at
10,000 rows, 8 MiB, 64 KiB/cell, 512 columns, 1 MiB metadata; defaults 1,000 rows.
Limits include owned value size but not all allocator overhead. Engine internal
allocations and one incoming row are not bounded. No untrusted-file guarantee.

Once the budget is reached, remaining rows are drained without retention. This
may take arbitrarily long, but preserves statement completion semantics for
writes with RETURNING. Limits apply to delivery, not execution. No SQL rewriting
for arbitrary queries. Completion reports Complete/Limited/Discarded independently
of error and observed autocommit. An error is never displayed as full success.
The UI keeps the last 100 execution outcomes, including replaced/discarded queries,
so a late write/error is not silently hidden when the current result changes.

Closing waits for the active operation, rejects queued work, explicitly rolls
back transactions, and reports cleanup errors. The UI must consume events until
Closed; dropping Session is best-effort cleanup, not a rollback acknowledgement.
No timeout or discard is proof of rollback. Busy timeout defaults to 250 ms,
configurable from 0–5000 ms at open, without retry. Submissions after close begins
are rejected, and queued commands are checked again before execution.

Read/write mode is opt-in before open. Changing mode requires close (with explicit
rollback confirmation for writable sessions), waiting for Closed, then reopening.
Closed is emitted after connection/database handles are dropped. Foreign keys are
enabled and verified on connection setup. Editor SQL deliberately remains powerful;
users can change connection-local PRAGMAs themselves. Generated edit connections
will always re-enable/verify foreign keys independently. Editor transaction state
is reported even on browse completions; browse state never overwrites it. Unexpected
worker disconnection is an unknown outcome, not a successful close. Native window
close waits for acknowledged cleanup and stays open on cleanup failure.
Single-statement validation uses Turso 0.8.3's parser, including trigger bodies;
multiple statements and malformed suffixes are rejected before execution. The UI
can extract one selection/current statement with the same parser, but never submits
a script. Named JSON objects require exact prefixed keys; positional arrays must
match the required slot count. Missing/extra and mixed-style bindings are rejected
before preparing the statement. Driver values remain typed, never interpolated.
Completion elapsed milliseconds cover worker execution and delivery/backpressure,
not queue wait or an isolated engine benchmark.

Table browsing uses a separate connection and controlled generated SELECTs: 100
rows/page, typed equality filters (`IS`, including NULL), quoted identifiers,
column sorting followed by primary-key and full-row tie breakers. Full-row
suffixes handle nullable primary keys; binary text collation and storage-class
tie breakers distinguish case and numeric storage types. Tables without a primary
key sort by all columns; identical duplicate rows are indistinguishable and not
editable. Paging currently uses bounded OFFSET (maximum 1,000,000), not keyset;
it may be slower at deep offsets. Concurrent local editor commits can shift page
boundaries: no snapshot consistency is promised. Every page is labelled as a
page, never a complete table. Views are read-only. Hidden/generated columns are
rejected rather than guessed. Schema discovery is itself budgeted and may be limited.

Metadata inspection returns a separate ticketed `Inspection`, never replacing the
current row result. Native `index_list`/`index_info`/`foreign_key_list` are used;
individual metadata queries are capped at 512 rows/1 MiB, index lists at 128 and
combined index details at 1 MiB. Table descriptors and resolved FK details are each
capped at 1 MiB; table names at 64 KiB. Implicit FK columns resolve to the parent's ordered
PK; ambiguous/unresolved metadata is not guessed. Related-row SELECTs validate and
quote target columns, bind all key parts, and retain at most 100 rows. Their scope
is a lookup, not a complete table; normal staged-edit identity checks still apply.

`Operation::Diagram` returns a separate ticketed `SchemaDiagram` on the browse lane;
it never replaces retained rows or draft identity. Catalog discovery excludes reserved
`sqlite_*` objects but includes Turso engine-managed tables. At most 512 tables and
8 MiB owned metadata (container/string capacities included, allocator overhead and
temporary/engine allocations excluded) are retained. Limits are explicit, and failed
per-table inspections remain error nodes rather than disappearing. Native inspection
limits and FK resolution rules still apply. The operation performs no table scans or
writes. Tests cover committed schema isolation during uncommitted editor DDL. This is
an explicit cached metadata read, not a schema-change monitor or new connection lane.

Count and diagnostic commands use the browse lane, not the editor transaction.
Diagnostics are a fixed read-only list (database list, journal mode, page count,
freelist count, integrity check, FK check), not arbitrary writable PRAGMA text.
The EXPLAIN helper accepts parsed SELECT/INSERT/UPDATE/DELETE only; DDL, PRAGMAs,
transaction commands and scripts are rejected. It prepares only the prefixed
EXPLAIN statement and never executes the source statement as a fallback. These
operations still obey drain/discard/close semantics; expensive counts/checks are
explicit user actions, not automatically scheduled scans.
