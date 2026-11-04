# Native Turso FTS and local-project validation

TursoBench enables `turso = { version = "=0.8.3", default-features = false,
features = ["fts"] }` and `Builder::experimental_index_method(true)`. Both
read-only and read/write opens use this configuration. All managed connections,
including dedicated edit commits, belong to that configured database handle.
Coordinated source sessions also enable experimental multiprocess WAL; other
experimental flags remain disabled. Shared native FTS reads and updates have
separate-process coverage; see [shared access](shared-access.md).

## Usage

In read/write mode, execute these as **separate statements** in the SQL editor:

```sql
CREATE TABLE documents(id INTEGER PRIMARY KEY, body TEXT NOT NULL) STRICT;
CREATE INDEX documents_fts ON documents USING fts(body);
INSERT INTO documents VALUES(1, 'hello Turso') RETURNING id;
SELECT id FROM documents WHERE fts_match(body, ?1);
```

For the final query, parameters are `[{"type":"Text","value":"Turso"}]`.
Existing native FTS databases can be searched in read-only mode; writes and
index creation still fail at the engine boundary. Column filters in the table
browser remain typed equality filters, not an implicit full-text search UI.
This is Turso's native index-method syntax, not a promise of SQLite FTS5/MATCH
compatibility. No additional database engine or fallback has been introduced.

The local clipboard project's n-gram form is also exercised:

```sql
CREATE INDEX search_grams ON search_content USING fts(indexed_content)
WITH (tokenizer='ngram', min_gram=3, max_gram=3);
```

## Local projects tested

`tests/fixtures/local-projects/` captures exact SQL `SCHEMA` constants from:

- scratchpad: session schema v9 (including quarantine, closed tabs and versions);
- bar-daemon: agenda v1 and notification v1 stores;
- clip-daemon: clipboard history v1 with native n-gram FTS.

`provenance.json` records source revisions, per-file hashes, working-tree
modification flags and schema hashes. Upstream MIT notices are retained. Some
source working trees had uncommitted changes; their exact file hashes, not HEAD
alone, identify the captured schema. No application implementation is imported.

Fixtures are created **from scratch through Turso**, with synthetic text, NULLs,
blobs, foreign keys, composite keys, constraints, indexes, 205-row pagination and
1 MiB oversized blobs. They are closed before TursoBench opens them. No live
application files, clipboard contents, user sessions, WAL copies or running
services were touched. Embedded application payloads are intentionally synthetic:
these tests validate database/workbench behavior, not the applications' JSON
payload contracts or safety of editing real application-owned stores.

Validated:

- schema navigation and every table's metadata/browse query;
- 100/100/5-row pages, sorting, typed filters, NULL and blob preservation;
- large-cell Limited delivery and engine read-only rejection;
- byte-for-byte preservation of all fixture files across read-only open/use/close;
- native FTS creation and query plans, parameterized searches and RETURNING;
- FTS index consistency after generated INSERT/UPDATE/DELETE, optimistic conflicts,
  mid-batch foreign-key failure/rollback and explicit editor rollback;
- independent browse/editor transaction visibility and close-time rollback;
- typed table-page and FTS result exports, headless egui rendering;
- native Wayland/Weston desktop open/browse/query/read-only-write rejection/close for all four
  fixtures, including a real FTS query through the clipboard SQL editor.

## Reproduce

```sh
nix develop -c cargo test --workspace --locked
nix develop -c bash scripts/local-project-smoke.sh
```

For manual inspection, choose a **new output directory**:

```sh
nix develop -c cargo run --locked -p tursobench-db --example local_project_fixtures -- /tmp/tursobench-project-fixtures
nix develop -c cargo run --locked -p tursobench-ui -- /tmp/tursobench-project-fixtures/clipboard.db
```

Refresh source snapshots only deliberately, then review the diff:

```sh
nix develop -c python3 scripts/capture-local-project-schemas.py /path/to/Projects
```

## Dependency safety and limits

Turso's FTS dependency graph initially selected vulnerable zstd 0.13.3 and
zstd-safe 7.3.0. `vendor/tantivy-sstable` preserves the published 0.7.0 Rust
sources and changes only its compression dependency range and an unused dev
dependency. The lockfile now uses zstd 0.14.3 and zstd-safe 8.2.0. See
`vendor/README.md` for archive hash, provenance, MIT notice and removal policy.

After the patch, `cargo audit` reports **zero vulnerability findings**, with
one unsuppressed upstream `lru` panic-safety warning (RUSTSEC-2026-0253). This is
not a blanket security certification; all preview/trusted-file/coordinated-access
restrictions in the compatibility contract still apply. Only native FTS index
methods have been exercised, not every possible experimental index implementation.
