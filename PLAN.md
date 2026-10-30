# TursoBench implementation plan

## 1. Direction and scope

TursoBench is a local-first desktop database workbench built with Rust and
`eframe`/`egui`, targeting the **Turso Rust engine crate 0.8.3**.

**Turso is the only supported database engine.** Support for other database
engines is out of scope for the project, not a deferred feature. All database
access uses Turso; file-format compatibility does not imply support for another
engine.

This supersedes the earlier proposal to add a GUI to the existing tursotui
backend. **We will rewrite the application database layer.** We are not rewriting
the Turso storage engine itself, and this is not a dependency-version bump or a
Ratatui-to-egui widget translation.

Project rationale supplied by the owner:

- The original tursotui project is no longer maintained.
- TursoBench needs a maintained database layer designed for Turso 0.8.3.

Treat tursotui as a feature reference and a possible source of individually
reviewed algorithms/tests, not a runtime dependency or a source of authoritative
engine compatibility rules. TursoBench owns its backend API and release cadence.

### Initial product

- Open existing local databases, initially in engine-enforced read-only mode.
- Browse schema and table data, filter/sort/page results, inspect individual cells.
- Run parameterized SQL without blocking the GUI.
- Opt into read/write access; stage, preview, commit, and discard table edits.
- Preserve SQL value types, including NULL and blobs.
- Provide useful errors without obscuring partial execution or transaction state.

### Deferred scope

Cloud connections/sync, a retained TUI, encrypted files, schema-design tools,
profiling, schema diffing, and complete
tursotui feature parity are not first-release requirements. Multiple database
windows/tabs can follow the first single-database vertical slice; identities and
session ownership must support them from the beginning.

The subsequent UI redesign adds a bounded, read-only native-metadata schema diagram
and responsive SQL/Data/Diagram presentation modes; independent database/query tabs
remain deferred. See [UI implementation and capability map](docs/ui-redesign.md).

Linux is the proposed first desktop target. Keep the design portable; confirm
macOS/Windows release requirements before promising support.

## 2. Evidence and engine baseline

Source review performed for this plan:

- tursotui reference revision: `8a4504f75b12ab9102c715b3bf4c73e1cd26b23c`.
- Published `turso` 0.8.3 crate source, rather than the moving `main` branch.
- Upstream `v0.8.3` tag resolves to
  `607bff9fd0e547a25a97b3bf06e83bcc5d99d53c`.

Verified API facts in the published crate:

| API / behavior | Consequence for the rewrite |
| --- | --- |
| `Builder::new_local(...).read_only(true)` | Use engine-level read-only access, not just disabled GUI buttons. |
| `Connection::prepare`, typed parameters, `Rows::next` | Build typed execution and bounded result delivery around prepared statements. |
| `Connection::transaction` and explicit commit/rollback | Preserve a connection for the lifetime of a transaction. |
| `Connection::transactional_batch` | Useful for bounded generated edit batches; joins an existing transaction rather than necessarily committing it. |
| Batch implementations buffer returned rows | Do not use batch APIs as the general large-query streaming path. |
| `Connection::is_autocommit`, `busy_timeout` | Track transaction state and implement a deliberate contention policy. |
| Dropped transactions defer cleanup to later connection use | Explicit rollback and shutdown cleanup are required; dropping a future is not proof of rollback. |
| Triggers and strict tables are always enabled | Do not inherit old experimental-feature assumptions. |
| Custom types, multiprocess WAL, and other features have experimental flags | Make capabilities explicit and validate them before enabling them. |

No public interruption API was found in the inspected Rust `Connection` and
`Statement` interfaces. Cancellation semantics remain a spike item, not a promised
feature. Source inspection is not proof of runtime behavior.

Initial dependency policy: use `turso = "=0.8.3"`, commit `Cargo.lock`, and select
Cargo features explicitly after the spike. Do not silently track later releases.
0.8.3 is the requested baseline; future upgrades require the compatibility suite.

References:

- https://github.com/mikeleppane/tursotui/tree/8a4504f75b12ab9102c715b3bf4c73e1cd26b23c
- https://docs.rs/turso/0.8.3/turso/
- https://static.crates.io/crates/turso/turso-0.8.3.crate
- https://github.com/tursodatabase/turso/tree/v0.8.3

## 3. Proposed architecture

```text
Cargo.toml                    # Workspace and dependency policy
crates/
  tursobench-core/            # Application types, commands/events, edit model
  tursobench-db/              # New Turso 0.8.3 adapter and session workers
  tursobench-ui/              # eframe binary, egui state and rendering
contracts/                   # Backend, editing, and compatibility guarantees
tests/fixtures/              # Reproducible fixture generators and provenance
```

Dependency direction:

- `core` has no Turso, egui, or Ratatui dependency.
- `db` depends on `core` and Turso; owns engine connections and error translation.
- `ui` uses `core` and the backend service API, never raw engine connections.
- Engine values are mapped into a small owned value model: NULL, integer, real,
  text, and blob. Preserve custom-type metadata separately where available.

Do not introduce alternative database adapters, engine-selection abstractions,
or a generic multi-database plugin framework. Define only the boundary needed to
test and maintain this Turso-only application.

### Sessions, execution, and lifecycle

- A database session owns its database handle and managed connection lanes.
- A SQL editor session uses a persistent, serialized connection so explicit
  `BEGIN`/`COMMIT`, temporary objects, and connection-local settings are coherent.
- Metadata and table browsing use separate managed connections where safe;
  document that they do not see an editor's uncommitted changes.
- GUI edit commits use a dedicated clean connection and an owned transaction;
  never accidentally join a SQL editor's transaction.
- Every operation carries stable database/session/request IDs and a generation.
  Late results from replaced queries or closed sessions cannot update current UI.
- Use bounded queues and bounded row chunks, with explicit row/byte budgets and
  backpressure. Limit individual-cell payloads too; a row limit alone is not a
  memory bound. Spike how oversized values can be handled safely.
- Keep engine work off the GUI thread. Validate `Send`/`Sync` and runtime needs
  before choosing worker threads versus a local async executor.
- Completion wakes egui for repaint. Rendering does not poll continuously at a
  fixed busy-loop rate.
- Separate cancel-requested, result-discarded, execution-stopped, and rolled-back
  outcomes. A timeout must not imply that writes stopped or were undone.
- Closing a session coordinates active work and explicit transaction cleanup.
  Never report successful rollback when the engine outcome is unknown.

### Query and schema contracts

- Use engine metadata for result shape; do not classify queries solely by their
  first keyword. Include `RETURNING`, CTEs, PRAGMAs, and empty results in tests.
- First ship single-statement execution. Script support needs a separately tested
  parser/execution policy; never split SQL on semicolons naively.
- Distinguish explicit user-managed transactions from application-owned atomic
  batches. Report statement failures and already-committed work honestly.
- Prefer supported schema introspection APIs/PRAGMAs; probe documented capability
  gaps against 0.8.3 before implementing SQL-text fallbacks.
- Do not mutate user databases during capability probing. Use disposable fixtures
  for destructive probes.
- Table browsing uses deterministic ordering and bounded pages, preferably keyset
  pagination with a unique tie-breaker. Define behavior for tables without usable
  keys and for concurrent changes; do not promise snapshot consistency implicitly.
- Arbitrary SQL uses bounded result delivery, not automatic SQL rewriting to add
  paging. Virtualized painting and database paging are different responsibilities.

### Editing and file safety

- Opening an existing path must not silently create a new empty database on typo.
  Creation, if implemented, is a separate explicit action.
- Read/write access is opt-in; changing mode requires a managed reopen after
  settling active queries and transactions.
- Staged edits are local data until commit. Rendering, focus changes, and selection
  must never trigger writes.
- Bind typed values; quote identifiers separately. Do not turn displayed blob
  summaries or formatted numbers into stored values.
- Distinguish omitted/default insert values from explicit NULL. Treat generated
  columns as non-editable when applicable.
- Initially permit editing only through controlled table-browse queries with a
  proven row identity. Views, ambiguous projections/joins, and keyless tables stay
  read-only. A parsed source-table name is not sufficient proof of editability.
- Preserve original keys and original values for optimistic conflict detection.
  Validate affected-row counts within the transaction; conflicts must not silently
  overwrite another writer's changes.
- Keep drafts on failed commits. Clear them only after confirmed success.
- Validate constraints and operation ordering without disabling foreign-key safety.
- Do not automatically migrate/checkpoint/repair user files. Test old-engine files
  on copies and document compatibility limits.
- Concurrent access by another process is a release gate, particularly because
  multiprocess WAL is experimental in this API. Do not assume SQLite tooling and
  Turso can safely access the same active database.

## 4. Delivery phases and acceptance gates

### Phase 0 — Engine compatibility and safety spike

Build a disposable/headless harness against exactly 0.8.3 before substantial GUI
work. Record results and decisions in `contracts/engine-compatibility.md`.

Test:

- Open/reopen existing files, missing paths, corrupt files, and permission errors.
- Read-only enforcement and any file/WAL side effects.
- Typed values and bindings, large results, blobs, `RETURNING`, and schema metadata.
- Transaction success/failure, commit failure, rollback failure, dropped-operation
  cleanup, locks, timeouts, and connection reuse.
- Simultaneous access from two processes, including a writer and viewer, with
  explicit engine feature configurations. Never use valuable databases here.
- Cancellation feasibility and worker responsiveness during long-running queries.
- Fixture copies from the old tursotui engine baseline and representative standard
  SQLite files, opened exclusively through Turso to establish its file-compatibility
  limits. This is not SQLite engine support; do not add a SQLite adapter or fallback
  engine, or equate file-format similarity with complete compatibility.

**Gate:** choose runtime/worker ownership, feature flags, supported file modes,
transaction policy, and truthful cancellation behavior. If safe live access is not
established, restrict the initial release to databases not in use; design a safe
snapshot workflow separately rather than copying a live main file without its WAL.

### Phase 1 — Workspace and new backend

Create the workspace, CI, pinned dependencies, domain types, structured errors,
request lifecycle, and new adapter. Implement open/close, schema loading, prepared
execution, bounded results, and read-only enforcement. No tursotui backend imports.

**Gate:** headless integration tests against real 0.8.3 databases; deterministic
unit tests for stale events, backpressure, and lifecycle transitions. Formatting,
linting, and locked builds run in CI.

### Phase 2 — Read-only egui vertical slice

Implement file selection, schema navigation, table grid, cell inspection, SQL
editor with parameters, and result/error presentation. Add visible distinction
between full completion and a limited result. Large cells must not break layout.

**Gate:** real local files can be browsed without UI freezes or unbounded result
retention. Switching queries/closing a database during execution is tested. SQL
writes fail at the engine boundary in read-only mode. Add automated egui interaction
tests and a repeatable desktop smoke test.

### Phase 3 — Read/write execution and transaction sessions

Add explicit read/write opening, persistent SQL-editor connection semantics,
transaction state, and safe close behavior. Introduce multi-statement scripts only
with the parser and partial-failure contract covered by tests.

**Gate:** explicit transactions work across editor executions; unrelated browsing
and generated edits cannot share them accidentally. Commit, rollback, contention,
shutdown, and uncertain outcomes are correctly surfaced.

### Phase 4 — Typed staged table editing

Implement the new edit model, cell editors, insert/delete, preview, discard,
parameterized atomic commits, optimistic conflict detection, and refresh.
Individually port useful tursotui test cases as regression inputs, not as proof that
its string-based editing model is correct for the rewrite.

**Gate:** composite/changed keys, NULL versus empty text, blobs, defaults, constraints,
conflicting external writes, and mid-batch failures are covered. A failed commit
retains drafts and never masquerades as success. GUI tests prove no writes before
explicit commit.

### Phase 5 — Usability and first release

Add query history/bookmarks, streaming exports with a documented typed encoding,
connection settings, and packaging. Keep preferences/history in application-owned
storage, never in the inspected database. Revisit advanced reference features only
after the core is reliable.

**Gate:** install/run smoke tests on supported platforms, documented compatibility
and known limitations, reproducible release builds, license notices, and measured
large-database responsiveness. Exports state whether they contain a page, limited
query result, or complete dataset; never silently label truncated data complete.

## 5. Reference-code policy

- Do not depend on `tursotui-db` or `tursotui-sql`, and do not preserve their public
  APIs merely for compatibility.
- Reimplement session management, execution, schema access, value handling,
  transaction management, cancellation, and result transport around 0.8.3.
- Consider isolated algorithms/tests for autocomplete, quoting, history, export,
  and editing only after review against the new contracts.
- Preserve MIT notices and record provenance for any copied/adapted code or tests.
- Do not import the old `AppState`, dispatcher, terminal components, or outdated
  engine-compatibility claims.

## 6. Implementation status

Follow-up capability decision: enable Turso's explicit `fts` Cargo feature and
`experimental_index_method(true)` for both access modes. Native FTS is validated
against source-derived local-project schemas; other experimental flags remain
disabled. See `docs/fts.md` for usage, dependency patch provenance and test results.

Follow-up functionality review: the single-database workbench now closes practical
schema/editor/results/editing/history/export gaps against tursotui. The explicit
remaining differences, evidence and shortcuts are in `docs/tursotui-review.md`.
This does not silently promote the deferred scope above to complete parity.

Phases 0–5 now have a Linux preview implementation, with a separate commit for
each phase. No upstream backend or GUI feature-parity import was performed.

- Phase 0: reproducible disposable 0.8.3 harness and recorded safety decisions.
- Phase 1: pinned workspace, CI, typed core and bounded session backend.
- Phase 2: read-only egui workbench, table pages, typed queries and inspection.
- Phase 3: opt-in writes, persistent editor transactions and managed close.
- Phase 4: typed staged edits, preview, atomic commits and optimistic conflicts.
- Phase 5: local history/bookmarks/settings, typed displayed-result exports,
  Linux preview packaging, license inventory and real desktop smoke automation.

Implementation does **not** mean all original acceptance gates are satisfied.
See `docs/release.md` for exercised tests, measurements, scope reductions and
outstanding production gates, and `contracts/engine-compatibility.md` for runtime
evidence. In particular, live multiprocess access is unsupported, file/engine
memory safety is not certified, cancellation does not interrupt execution,
paging uses OFFSET, and fault-injected transaction failures, legacy-engine
fixtures, complete license review and cross-host reproducibility remain open.
No stable production release or support for another database engine is claimed.
