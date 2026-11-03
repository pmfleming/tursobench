<p align="center">
  <img src="packaging/tursobench.png" alt="TursoBench app icon" width="88">
</p>

<h1 align="center">TursoBench</h1>

<p align="center">
  <em>“These heavy sands are language tide and wind have silted here”</em><br>
  <sub>— James Joyce, <em>Ulysses</em> (“Proteus”)</sub>
</p>

<p align="center">
  <strong>A native workbench for local Turso databases.</strong><br>
  Rust 1.95 · egui 0.37 · Turso 0.8.3 · Linux preview
</p>

<p align="center">
  <a href="#workspace">Workspace</a> ·
  <a href="#session-database">Session database</a> ·
  <a href="#sql-and-results">SQL</a> ·
  <a href="#run">Run</a> ·
  <a href="#current-limits">Limits</a> ·
  <a href="docs/guide.md">User guide</a>
</p>

TursoBench opens existing local files. Turso is the only execution engine.
Connections default to engine-enforced read-only access.
Writes require explicit read/write access.

Independent project. Not affiliated with the Turso team.

## Workspace

Five fixed tiles share one database session.
Relationships sits above Table data.
Record, Structure and SQL occupy the right column.
Each tile has direct action buttons.
Ribbons scroll horizontally. Window controls stay fixed.

![Current native workspace. Real document_history rows from the local Scratchpad session database.](docs/assets/ui/sessions/workspace-dark.png)

Minimize a tile to its header. Expand a tile across the workspace.
Filters, selections, SQL drafts and staged edits retain their owners.
SQL editors retain separate result buffers.
Editor and browse connections are separate. Browsing sees committed state.

| Surface | Implemented behavior |
| :--- | :--- |
| **Navigator** | Schema search. Recent-file switcher. History. Saved queries. Checks. Settings. |
| **Table data** | Virtualized rows. 100-row fetches. Bound filters. Column sorting and resizing. Range selection. |
| **Record** | Typed values. Field selector. Row-position slider. Parent and incoming references. |
| **Structure** | Original DDL. Declared types. Primary keys. Indexes. Foreign-key actions. |
| **Relationships** | Declared FK edges. Key labels. Pan and zoom. Saved node positions. |
| **SQL** | Syntax highlighting. Completion. Typed bindings. Statement selection. EXPLAIN. Linked results. |

## Session database

These images show the current application on this NixOS laptop.
The examples use a snapshot of Scratchpad's real `session.db`.
The snapshot contains **10 tables**, **6 explicit indexes**, **130 buffers**,
**130 snapshots**, **198 history entries** and **139 blob rows**.
The database itself is not included.

| Tables | Stored state |
| :--- | :--- |
| `buffer_ids` | Stable buffer identifiers. |
| `document_history`, `history` | Current/tip pointers. Edit sequences. Parent links. History payloads. |
| `snapshots`, `parts`, `blobs` | Snapshot sizes. Ordered parts. Content digests. Blob payloads. |
| `session_state`, `tabs`, `closed_tabs` | Session payload. Tab positions. Closed-tab payloads. |
| `quarantine` | Rejected payloads and recorded reasons. |

### Browse and follow references

`document_history.temp_id` references `buffer_ids.id`.
The grid exposes that relationship directly.
Typed predicates apply to the table query. They are not client-side row searches.
Counts are explicit operations.

![Expanded native table view. Actual document_history rows from the session snapshot.](docs/assets/ui/sessions/table-data.png)

### Inspect the schema

The diagram uses declared relationships.
The session schema links buffer identifiers, history, snapshots, parts and blobs.
Unresolved targets stay unresolved.

![The session database's actual ten-table schema and declared foreign-key relationships.](docs/assets/ui/sessions/relationships.png)

Structure preserves the original `CREATE` statement.
Composite keys, constraints and index columns remain visible.

![Native structure inspector. Actual session schema, indexes and foreign keys.](docs/assets/ui/sessions/structure.png)

## SQL and results

The editor executes one parsed statement per request.
CTEs, PRAGMAs and `RETURNING` are supported.
Run the buffer, selected SQL or the statement at the cursor.
Named and positional bindings retain their storage types.
Values are bound. They are never interpolated.

This query joins actual history pointers to snapshot metadata:

```sql
SELECT d.edit_seq, d.now_id, d.tip_id,
       s.bytes, length(s.digest) AS digest_bytes
FROM document_history AS d
JOIN snapshots AS s ON s.id = d.temp_id
ORDER BY s.bytes DESC
LIMIT 12;
```

![Native SQL editor and its twelve real result rows from the session database snapshot.](docs/assets/ui/sessions/sql-results.png)

The largest retained snapshot is **611,668 bytes**.
The snapshot-size sum is **2,307,840 bytes**.
The query reports **32-byte digests**.
These values describe this captured session. They are not benchmarks.

Native Turso full-text indexes are supported.
Use `USING fts(...)` and `fts_match(...)`.
Index creation requires read/write access.
See [FTS syntax and validation](docs/fts.md).

## Editing, transactions and export

Grid inserts, updates and deletes are staged locally.
Only suitable primary-key base tables support generated edits.
Review exposes diffs, SQL and exact typed bindings.
Commit uses a dedicated atomic batch.
Original-value predicates detect conflicts.
Failed commits retain drafts.

Editor SQL autocommits unless a transaction is open.
`BEGIN`, `COMMIT` and `ROLLBACK` use the persistent editor connection.
Closing waits for execution and rollback acknowledgement.

Export uses retained engine values. Staged overlays are excluded.
Formats include typed JSONL/JSON, annotated CSV/TSV and SQL INSERTs.
Export never re-executes the query. Existing files are not overwritten.

## Desktop appearance

The interface follows the laptop's GTK and desktop-portal palette.
This machine supplies blue accents, black surfaces, amber pending state and green relationships.
Light mode adapts those desktop colors.
System fonts and accessible control names are retained.
Font size is adjustable from 10–20 pt.

![The same real session data in the application's explicit light mode.](docs/assets/ui/sessions/workspace-light.png)

[Capture provenance](docs/assets/ui/README.md#current-readme--real-session-database)
· [Workspace behavior](docs/ui-redesign.md)

## Run

Use the repository's Nix development shell:

```sh
nix develop -c cargo run --locked -p tursobench-ui
```

Select an existing, exclusively available database:

```sh
nix develop -c cargo run --locked -p tursobench-ui -- /path/to/session.db
```

With the pinned Rust toolchain and native libraries installed:

```sh
cargo run --locked -p tursobench-ui -- /path/to/session.db
```

Linux requires OpenGL and X11 or Wayland.
The file picker uses the desktop portal.
Paths can also be entered directly.
Open never creates a database.

## Current limits

- **Exclusive local access.** Close other clients. Back up before writes. Live multiprocess use is unsupported.
- **Turso only.** A SQLite header is a compatibility guess. It does not select another engine or convert the file.
- **Bounded retention.** Queries default to 1,000 rows. Delivery budgets include 8 MiB per result and 64 KiB per cell.
- **OFFSET paging.** Table browsing is not snapshot-consistent. Deep offsets may be slow.
- **Delivery suppression.** Discard stops result delivery. SQL execution and writes continue.
- **Explicit outcomes.** An unknown commit or rollback is not reported as success.
- **Limited editing.** Arbitrary query results, keyless tables, `WITHOUT ROWID` tables and generated columns are not editable.
- **Single statements.** Multi-statement buffers require statement selection. Script execution is unsupported.
- **Preview scope.** No cloud sync or encryption support. Engine memory is not bounded by the UI budgets.

[Engine compatibility](contracts/engine-compatibility.md)
· [Editing contract](contracts/editing.md)
· [Release gates](docs/release.md)

## Build and verify

```sh
nix develop
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked -p tursobench-ui
bash scripts/desktop-smoke.sh
bash scripts/discovery-smoke.sh
bash scripts/local-project-smoke.sh
```

Unit and integration tests cover the core, database worker and native UI.
Desktop checks use native egui rendering under Xvfb/Mesa.
They cover opening, browsing, schema loading, typed SQL, read-only rejection and clean close.
Layout checks include both themes, compact viewports and enlarged text.

| Crate | Responsibility |
| :--- | :--- |
| [`tursobench-core`](crates/tursobench-core) | Typed values. Request identities. Result retention. Draft batches. |
| [`tursobench-db`](crates/tursobench-db) | Turso connections. Serialized workers. SQL validation. Metadata. Atomic edits. |
| [`tursobench-ui`](crates/tursobench-ui) | egui workspace. Native controls. Desktop theme. File discovery. Export. |

[Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) · [Code of conduct](CODE_OF_CONDUCT.md)

[MIT license](LICENSE) · [Third-party notices](third-party/README.md)
