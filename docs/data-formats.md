# Application storage and exports

## Preferences

`$XDG_CONFIG_HOME/tursobench/preferences.json`, falling back to
`$HOME/.config/tursobench/preferences.json`. Never stored in the inspected database.
Read/write opt-in and exclusive-use acknowledgement are deliberately not persisted.
Busy timeout defaults to 250 ms, configurable 0–5000 ms after reopen. Query row
retention is configurable 1–10,000; fixed byte/cell budgets still apply.

History and bookmarks are capped at 100 entries and 512 KiB each (including names
and scope paths), with a 64 KiB per-statement maximum. History records the last
outcome/time per distinct SQL + database path; bookmarks have editable names.
Absolute database path spellings **are persisted** for scoping (relative input is
resolved against the working directory); aliases can have separate histories. Legacy statement lists migrate to visibly unscoped entries. SQL literals
and paths may be sensitive. Parameter values, query results, editor buffers and
drafts are not persisted. Loading history/bookmarks clears bindings. Clear history
or delete bookmarks from the sidebar. System/Light/Dark appearance, NULL marker and slow-query
threshold are also stored. Preferences use a private temporary file, flush/sync,
then atomic replacement in the application directory. Saves run on a background
worker with coalescing. This is best-effort convenience storage, not durable draft
recovery; abrupt process shutdown can lose the most recent preference change.
Preferences carry format version 1 and have a 2 MiB input/output limit. Valid
fields/array entries survive malformed peers; recovery preserves the original
and saves a separate copy. Each valid save retains a previous-file backup.
Unreadable or newer-format startup data disables automatic persistence. Runtime
corruption/newer data also refuses replacement, preserving the previous backup.
These failures produce a visible warning/error and safe defaults where needed.

The `discovery` preference stores the startup toggle, added search folders,
exclusions, recent successful opens (absolute path and last-open time), and cached
candidates (absolute path, size and optional modification time). Limits are 32
search folders / 32 KiB, 64 exclusions / 32 KiB, 50 recent files / 64 KiB and 200
cached candidates / 256 KiB, with 4096 bytes per path. Paths are normalized
lexically, without filesystem access; non-UTF-8 paths are not persisted. Older
preferences default to startup discovery enabled. Cached headers are rechecked;
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
map in older preferences. Grid column presentation and inspected/filter values
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
