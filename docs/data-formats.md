# Application storage and exports

## Preferences

`$XDG_CONFIG_HOME/tursobench/preferences.db`, falling back to
`$HOME/.config/tursobench/preferences.db`. This is an application-owned **local
Turso database**, separate from every database opened in the workbench. Existing
`preferences.json` files are neither read nor imported; there is no backward
compatibility or automatic format migration. The new store starts with defaults.

The `app_preferences` table contains one row (`id = 1`), with `format_version`, a
JSON-encoded `payload` snapshot and `previous` snapshot. Both database schema and
snapshot format are version 1; the database has application ID `0x54554250`.
Saving replaces the current snapshot and retains its predecessor in the same
transaction. JSON is an internal serialization inside Turso, not a preferences
file. Snapshots are limited to 2 MiB. Back up the closed store with any required
WAL state; `previous` is a recovery snapshot, not a separate database backup.

A dedicated storage worker owns the Turso connection. Startup waits for the initial
settings; later writes are asynchronous and coalesced. Normal shutdown drains the
latest pending snapshot and releases the writer. Forced termination can still lose
uncommitted/pending preferences. Lock wait is 250 ms and the cooperative statement
deadline is 5 s. Multiple concurrent application writers are not supported.
Unreadable, unrelated or unsupported-version stores are preserved and disable
saving. Invalid preference fields disable saving with a visible error rather than
replacing saved queries with defaults. External snapshot changes and failed or
uncertain saves also block further writes until restart/inspection. On Unix, the
application preferences directory is private (0700) and newly created databases
are 0600; WAL/sidecars stay inside the private directory.

Read/write opt-in is not persisted. Admission is checked using live locks rather
than a saved exclusive-use acknowledgement. History
and saved queries are capped at 100 entries and 512 KiB each, with a 64 KiB SQL
maximum. SQL literals and database paths are persisted and may be sensitive.
Parameter values, query results, editor buffers, staged edits and transactions
are not persisted. Settings, discovery state, inspector visibility, workspace and
diagram layouts share the same snapshot. Loading saved SQL clears its bindings.

The `discovery` preference stores the startup toggle, added search folders,
removed default/recent-parent roots (up to 128 paths / 512 KiB),
exclusions, recent successful opens (absolute path and last-open time), and cached
candidates (absolute path, size and optional modification time). Limits are 32
search folders / 32 KiB, 64 exclusions / 32 KiB, 50 recent files / 64 KiB and 200
cached candidates / 256 KiB, with 4096 bytes per path. Paths are normalized
lexically, without filesystem access; non-UTF-8 paths are not persisted. Startup
discovery is enabled by default. Cached headers are rechecked;
modification times and directory timestamps are never proof of validity. Use
**Forget database** in a row's context menu to remove an individual saved path; a future scan can
rediscover it. Excluded folders disappear from the chooser and cached candidates.
Successful opens are recorded only after the engine reports Opened, never on
selection or failed open. See [database discovery](database-discovery.md).

## Workspace layouts

Layouts save on managed close in workspace_layouts, bounded to 20 absolute database
paths and 128 KiB of descriptor payload. They contain reopenable view kinds/object
names, split ratios, selected tabs and focused/expanded identities. Invalid IDs,
oversized trees, unsupported view kinds and nonfinite geometry are rejected.
Results, pinned records, SQL drafts and bindings are not stored. Restoring table
views performs bounded reads; restoring SQL creates an empty editor without execution.

## Typed JSONL v1

Export streams the **already retained engine result**, not local staged overlays,
to a new `.jsonl` file
on a background thread, without re-running SQL (and thus without accidentally
repeating writes). A bounded copy of displayed rows is made for the job. It does
not stream an unlimited table from the database; it exports the retained window.

First line is a header with `format: "tursobench-typed-jsonl"`, `version: 1`, engine,
column names/declared types, expected row count and an explicit scope:

- `complete-query-result`: all rows from a completed, unlimited delivery;
- `table-window`: only the currently retained bounded browse window, not the table;
- `table-page`: legacy scope in older exports;
- `related-row-lookup`: a bounded related-row lookup (up to 100), not the table;
- `pinned-record`: one independently retained record snapshot;
- `single-retained-row`: one retained row copied to the clipboard;
- `limited-query-result`: a budget-limited retained prefix;
- `failed-query-partial-result` / `discarded-query-partial-result`: not complete.

Each following row has `record: "row"` and a `values` array:

```json
{"record":"row","values":[{"type":"null"},{"type":"integer","value":"9223372036854775807"},{"type":"real","value":"-0"},{"type":"text","value":""},{"type":"blob","hex":"00ff"}]}
```

Integer and real values are strings to avoid JSON consumer precision loss.
Integers are signed decimal i64. Reals use Rust's round-trip f64 decimal formatting,
including `inf`, `-inf`, `NaN` if returned by the engine. NaN payload bits are not
preserved. Text is JSON escaped Unicode; blobs are lowercase hex. NULL and empty
text/blob are distinct. Custom-type metadata is not available in this preview.

The last line is `{"record":"end","rows":N}`. Missing/mismatched end records mean
an incomplete/corrupt export. A same-directory private temporary file is flushed
and synced, then published atomically without overwriting an existing destination.
A failure never relabels partial data as complete or overwrites a user database.
The export file format differs deliberately from the editor's tagged input JSON.

## Other retained-result formats

All disk formats use the same atomic, no-overwrite publication and explicit scope.
The file extension must match the selected format. Clipboard result/row exports
are limited to 2 MiB; exceeding the limit leaves the clipboard unchanged.

- **Typed JSON** (`.json`): one object with `header`, `rows` (arrays of the same
  typed cells as JSONL), and `end: {"rows": N}`. Header format is
  `tursobench-typed-json`. Duplicate column aliases remain separate array entries.
- **CSV/TSV** (`.csv` / `.tsv`): standard files start directly with the header
  and do not alter text. Separate spreadsheet-protection variants prefix formula-like
  text, and separate annotated variants retain scope metadata. In annotated mode
  the first line is a `# TursoBench ...`
  scope/row-count/convention notice. **Skip that metadata line** before parsing
  the header and records as CSV/tab-delimited CSV. Quoting doubles embedded quotes
  and preserves delimiters/newlines. NULL is unquoted `\N`; blobs are `X'hex'`.
  Text whose first non-whitespace character is `=`, `+`, `-` or `@` gets an
  apostrophe prefix as spreadsheet-formula mitigation. These are convenience
  formats, **not lossless typed interchange**: consumers can infer unwanted types,
  and NULL-like text can be ambiguous after parsing. Prefer typed JSON/JSONL for
  fidelity; do not treat quoting as a security boundary for untrusted spreadsheets.
- **SQL INSERTs** (`.sql`): a scope comment followed by INSERT statements into an
  explicitly chosen, quoted target table/columns. Integers, finite reals (including
  a real decimal point), escaped text, NULL and hex blobs retain their SQL kinds.
  NUL-containing text uses `CAST(X'utf8hex' AS TEXT)`. Non-finite reals and duplicate
  result column names are rejected before publication. The target schema must
  already exist and its affinities/constraints still apply. No DDL is inferred,
  no transaction is implicitly opened, and the file is never automatically run.

Record detail pretty-prints valid JSON objects/arrays in TEXT using the original
number/string/key tokens, so large numeric literals and duplicate keys are not
rounded or dropped. Invalid or over-budget formatting falls back to original text.
Cell copying preserves original TEXT; NULL is `NULL`, blobs are `X'hex'`.

## v4 presentation preferences and selection copy

Inspector visibility is saved per database in `inspector_visibility`, bounded to
20 absolute UTF-8 paths / 4096 bytes per path. Missing fields default to an empty
map in older preferences.

`panels` stores whether the sidebar and the assistant are shown (default: sidebar
only). `assistant` stores the chosen agent (`provider`: `Claude`, `Copilot`,
`Codex` or `Custom`), a `custom_command` of up to 4096 bytes, a `model` of up to
200 bytes, and `sharing`: `Schema` or `Rows` per absolute database path, bounded
to 50 paths / 4096 bytes per path. A missing database means the user has not
chosen yet. Conversations are never stored. Grid column presentation and inspected/filter values
are not persisted. Table retention uses a 200-row minimum and reserves 100 rows
in flight; SQL's exact row setting is unchanged.

Ctrl C on a grid selection produces plain TSV in visible column order, with
quoted tabs/newlines/quotes and preserved empty fields, capped at 2 MiB. It copies
stored engine values, not draft overlays. Ctrl Shift C uses standard TSV in canonical result column order. Export offers
standard, annotated and protected variants explicitly. Numeric
selection summaries preserve integer sum/min/max exactly and label approximate
averages or mixed-real aggregates.


## Complete source transfers

The transfer/maintenance panel is available from retained export and Diagnostics.
Complete-table JSONL export streams the current filter/sort through a dedicated
read; execute-and-export runs the selected/cursor statement exactly once on the
editor connection. It never reruns a prior retained result implicitly. New-file
publication is atomic. A file error after a write started does not undo that SQL:
the write is drained and its actual completion/transaction state is reported.

Streaming typed JSONL v2 uses the same cell encoding as v1, with
`rows_expected: null` in the header and a required final exact row count. Scope is
`complete-table` or `complete-query-result`. Imports accept v1/v2 JSONL and retained
typed JSON v1, validate end records, and reject unknown/deferred cell kinds.

CSV/TSV import handles quoted delimiters, escaped quotes, multiline fields and
unquoted NULL markers. Mapping/type choices are explicit; CSV defaults to TEXT
and target affinities/constraints still apply. Preview reads at most ten rows /
1 MiB and does not certify the unread remainder. Input rows are capped at 8 MiB,
encoded records at 16 MiB, and whole-object JSON at 16 MiB (use JSONL for larger
files). NaN cannot be imported as a SQL real; infinities are supported by typed
interchange. Atomic import rolls back on any data/footer error; optional batch
mode commits every 1,000 rows or 8 MiB and reports earlier confirmed commits.

## Deferred values and maintenance

Oversized result cells use session-owned anonymous temporary backing, with a chunk viewer
and exact-byte save action. Retained exports/copies reject deferred cells rather
than fabricating full values. Backing is capped at 64 MiB/value, 256 MiB/session
and 1,024 files with FIFO expiration; engine incoming allocations are not bounded.

Checkpoint and backup require write access and editor autocommit. Backup explicitly
checkpoints and closes the source session, refuses a remaining nonempty WAL, copies
the closed main file, verifies the private copy with Turso integrity_check, and
publishes without overwrite. Exclusive use by other applications remains required.

SQL real literals are checked against the pinned engine numeric conversion before
publication. If neither supported decimal spelling reloads the exact f64 bits,
SQL export is rejected; typed JSON/JSONL remains the fidelity-preserving option.
