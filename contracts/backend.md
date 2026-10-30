# Backend service contract

`core` owns values, identities and events; `db` owns Turso and the matching parser.
`Session::open` starts one named worker. Canonicalization, schema validation and
all engine calls happen there. Existing nonempty regular UTF-8 paths only.
The preflight check is not protection against a malicious concurrent path swap.
Use trusted paths not being modified by another program.

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

Closing waits for the active operation, rejects queued work, explicitly rolls
back transactions, and reports cleanup errors. The UI must consume events until
Closed; dropping Session is best-effort cleanup, not a rollback acknowledgement.
No timeout or discard is proof of rollback. Busy timeout is 250 ms, without retry.
Single-statement validation uses Turso 0.8.3's parser, including trigger bodies;
multiple statements and malformed suffixes are rejected before execution.

Table browsing uses a separate connection and controlled generated SELECTs: 100
rows/page, typed equality filters (`IS`, including NULL), quoted identifiers,
column sorting followed by primary-key tie breakers. Tables without a primary
key sort by all columns; identical duplicate rows are indistinguishable and not
editable. Paging currently uses bounded OFFSET (maximum 1,000,000), not keyset;
it may be slower at deep offsets. Concurrent local editor commits can shift page
boundaries: no snapshot consistency is promised. Every page is labelled as a
page, never a complete table. Views are read-only. Hidden/generated columns are
rejected rather than guessed. Schema discovery is itself budgeted and may be limited.
