# Typed staged editing

Only controlled browse results for non-engine-managed base tables with an engine-reported
primary key and no hidden/generated columns are editable. Reserved `__turso_internal_`
storage is inspectable but excluded from generated edits. Views, keyless tables,
arbitrary SQL results and partial/discarded/error results cannot be edited.
Original row keys must be non-NULL. Custom types and WITHOUT ROWID are disabled.

Drafts are local owned typed data. Rendering, clicking and focus changes issue no
writes. The inspector stages explicit typed values. The UI requires editor autocommit
before staging, preventing drafts from blocking the transaction controls needed to
finish a pre-existing manual transaction. Insert JSON has one optional
value per column: JSON `null` means omit/use default; `{"type":"Null"}` means
SQL NULL. Text remains text; blobs remain byte arrays. Non-finite reals and cells
over 64 KiB are rejected. At most 1,000 operations and 8 MiB per batch.

Preview shows the exact quoted SQL and separate typed bindings. Explicit COMMIT
is the only staged-write action. Result-changing navigation is locked until commit/discard.
Presentation-mode changes and independent read-only diagram metadata do not replace
row/draft identity and remain available.
The listed operation order is preserved; the app does not reorder dependencies,
disable constraints or solve key swaps. Repeated edits of one cell merge; deletion
supersedes row updates. Editing a deleted row again replaces its deletion.

Commit opens a dedicated fresh connection, enables/verifies foreign keys, begins
IMMEDIATE, rereads schema and rejects changed metadata. UPDATE/DELETE predicates
include all original values, their storage classes, and binary comparisons (not
column case-insensitive collations). Changed keys use ORIGINAL values in the
predicate. Exactly one affected row is required for every operation, including
inserts. A conflict or constraint failure rolls back the entire transaction.
This detects differing values, not ABA changes that return to identical values.
Triggers run normally; their side effects follow engine semantics.

Only confirmed commit clears drafts and refreshes. Rejected batches retain drafts.
Commit failure or rollback failure reports UNKNOWN, retains drafts and disables
further generated commits until close/reopen and manual inspection. Never retry
an uncertain outcome automatically. Closing a writable database requires explicit
confirmation that local drafts will be discarded and editor transactions rolled
back. The UI does not claim a rollback undoes already committed work.

Tests cover composite/changed keys, NULL/blob/empty text/defaults, constraints,
foreign-key enforcement independent of editor settings, conflicting writes from
the same application's editor, schema changes, busy editor transactions and
mid-batch atomic rollback. Headless egui pointer interaction proves cell selection
and staging do not write; failed commit retains drafts, successful commit clears.
Cross-process editing is unsupported, not validated by these tests.
