# Application storage and exports

## Preferences

`$XDG_CONFIG_HOME/tursobench/preferences.json`, falling back to
`$HOME/.config/tursobench/preferences.json`. Never stored in the inspected database.
Read/write opt-in and exclusive-use acknowledgement are deliberately not persisted.
Busy timeout defaults to 250 ms, configurable 0–5000 ms after reopen. Query row
retention is configurable 1–10,000; fixed byte/cell budgets still apply.

History and bookmarks are capped at 100 statements and 512 KiB each, with a
64 KiB per-statement maximum. SQL literals may be sensitive. Parameter values,
query results, drafts and database paths are not persisted. Clear history and
bookmarks from the sidebar. Preferences use a private temporary file, flush/sync,
then atomic replacement in the application directory. Saves run on a background
worker with coalescing. This is best-effort convenience storage, not durable draft
recovery; abrupt process shutdown can lose the most recent preference change.
Invalid/oversized preferences produce a visible error and safe defaults.

## Typed JSONL v1

Export streams the **already retained displayed result** to a new `.jsonl` file
on a background thread, without re-running SQL (and thus without accidentally
repeating writes). A bounded copy of displayed rows is made for the job. It does
not stream an unlimited table from the database; for now export pages separately.

First line is a header with `format: "tursobench-typed-jsonl"`, `version: 1`, engine,
column names/declared types, expected row count and an explicit scope:

- `complete-query-result`: all rows from a completed, unlimited delivery;
- `table-page`: only the displayed 100-row-or-smaller page, not the table;
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
