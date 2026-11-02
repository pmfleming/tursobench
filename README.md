# TursoBench

A local-first **Turso-only** desktop database workbench, written in Rust with
eframe/egui and pinned to **Turso 0.8.3**. Linux preview; not a multi-engine client.

## Run

```sh
cargo run --locked -p tursobench-ui -- /path/to/existing.db
# Pinned Nix development environment (no global Cargo installation required):
nix develop -c cargo run --locked -p tursobench-ui
```

### Nix environment

`flake.nix` / `flake.lock` pin Nixpkgs and the Rust overlay. The shell reads
`rust-toolchain.toml` for Rust 1.95.0 and includes Cargo, rustfmt, Clippy,
rust-analyzer, Rust sources, a C compiler, Python, Xvfb and desktop libraries.

Enter the environment with `nix develop`, then run Cargo normally. The legacy
`nix-shell` entry point uses the same pinned inputs and environment.

For automatic activation when entering this folder, enable direnv's hook in your
shell and approve the project's `.envrc` once:

```sh
direnv allow
cargo run --locked -p tursobench-ui
```

For Bash without an existing hook, run `eval "$(direnv hook bash)"` in your terminal
(or add it to your Bash configuration). Otherwise use `nix develop`; no direnv hook
is required. `.direnv/` is local cache data and is ignored by Git. Nix requires
`nix-command` and `flakes` enabled; the first activation may download dependencies.

Rust 1.95.0 is the tested toolchain. Linux needs OpenGL, X11 or Wayland and
libxkbcommon; the native file dialog requires the D-Bus runtime library
(`libdbus-1.so.3`) and an XDG portal with a file chooser backend. The Nix shell
provides the library; the desktop session provides the portal. Typing a path
works without a portal. No database is created by Open. For a disposable sample:

```sh
python3 tests/fixtures/generate.py /tmp/tursobench-sample.db
```

**Use trusted files not open in another application.** Back up closed files with
all required WAL state before trying writes. Do not copy only the main file of a
live database. Live multiprocess use, cloud sync, encryption and other engines
are unsupported. The exclusive-use checkbox is an acknowledgement, not an OS lock.

## Features

- Startup database discovery across application data/state and development folders, with recent files, Turso/SQLite/unknown type icons, a per-file SQLite override, and search-folder controls. See [discovery behavior and limits](docs/database-discovery.md).
- Responsive dark/light workspaces, activity rail and docked/pop-out inspector for laptops and widescreens.
- Engine-enforced read-only opening by default; read/write is explicit opt-in.
- Searchable schema/DDL, native index/FK details, related-row navigation and read-only diagnostics.
- Zoomable schema diagram: tables, keys, composite/self/cyclic foreign keys, pan, arrange, inspect and browse.
- 100-row table pages, sorting, typed equality filters and index-awareness hints.
- Virtualized results, resizable columns, typed cell/record inspection and clipboard copy.
- Highlighted SQL, selection/current-statement execution, prefix completion and EXPLAIN.
- Single-statement SQL including CTEs, PRAGMAs and RETURNING; typed positional/named bindings.
- Native Turso FTS and experimental index methods enabled: open existing FTS
  databases, create `USING fts(...)` indexes and query with `fts_match(...)`.
  See [FTS support and validation](docs/fts.md).
- Persistent SQL-editor transactions, explicit commit/rollback, managed close.
- Local staged inserts/updates/deletes, SQL/binding preview and atomic commits
  with optimistic conflicts. Only suitable non-engine-managed primary-key base tables are editable.
- Typed cell editing, row cloning, draft overlays and per-cell/row/operation undo.
- Timed searchable history, named database-scoped bookmarks, themes and keyboard help.
- Retained-result exports: typed JSONL/JSON, annotated CSV/TSV and SQL INSERTs.

See the [UI guide and capability map](docs/ui-redesign.md) for workspace navigation,
responsive behavior, diagram limits and validation. The SQL/Data modes share one
editor and one retained result; they are not independent transaction tabs.

See the [tursotui functionality review](docs/tursotui-review.md) for changes,
shortcuts, evidence and the remaining gaps—not a claim of complete parity.

Parameters use tagged JSON (cell editing also has a typed form), for example:

```json
[{"type":"Integer","value":42},{"type":"Null"},{"type":"Blob","value":[0,255]}]
```

For `:id`, use an object: `{":id":{"type":"Integer","value":42}}`.
Named keys retain their `:`, `$` or `@` prefix; do not mix named and positional placeholders.

Insert drafts use one entry per column: JSON `null` omits that column (DEFAULT);
`{"type":"Null"}` explicitly inserts SQL NULL. Drafts never write until COMMIT.
Query parameters are not persisted. History **does** store SQL literals: clear
history if they contain sensitive information.

## Important limits

“Discard results” does **not** interrupt execution or roll back writes. Queries
are drained to completion even after the display budget is reached. Closing can
therefore wait indefinitely for a non-yielding query. Application retention is
bounded, but engine allocations are not: a huge cell/query can exhaust memory.
Default query retention: 1,000 rows, 8 MiB, 64 KiB/cell. Large-result completion is
labelled Limited, not silently called complete. Table paging uses OFFSET, is not
a snapshot, and may be slow at deep offsets. Multi-statement editor buffers can be
executed one selection/statement at a time, but never as scripts. No arbitrary-result
editing, keyless editing, or automatic migration/checkpoint/repair.

This is a working preview, **not a production safety certification**. See
[compatibility evidence and unpassed gates](contracts/engine-compatibility.md),
[backend contract](contracts/backend.md), [editing contract](contracts/editing.md),
[exports/storage](docs/data-formats.md) and [release status](docs/release.md).

## Validate and package

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked -p engine-spike
cargo build --locked -p tursobench-ui
bash scripts/desktop-smoke.sh  # requires Python 3, xvfb-run, timeout and Mesa
bash scripts/discovery-smoke.sh  # startup scan with an isolated home and disposable files
bash scripts/local-project-smoke.sh  # exact local-project schemas; synthetic data only
bash scripts/package-linux.sh
```

The smoke test uses 100,000 disposable rows, an isolated preferences directory,
and a real desktop renderer under Xvfb. See the [manual smoke checklist](docs/desktop-smoke-test.md).
Packages contain a binary, desktop launcher, documentation, lockfile and dependency
license inventory/texts. Build on the intended Linux distribution; a Nix-linked
binary is not a portable Linux tarball. macOS/Windows packages are not promised.

MIT licensed. No tursotui code or tests have been copied. Dependencies retain their
own licenses; consult generated package notices before redistribution.
