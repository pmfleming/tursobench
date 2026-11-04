# Turso 0.8.3 compatibility and safety

## Baseline and reproduction

Run `nix-shell --run 'cargo run --locked -p engine-spike'` (or use Rust and
Python 3 directly). The harness creates and deletes only disposable fixtures.
No tursotui backend code is imported. `turso = "=0.8.3"`, default Cargo features
OFF, `fts` explicitly ON, and `experimental_index_method(true)` enabled on every
production database open, including read-only opens. Other experimental builder
flags remain OFF. Turso is the only engine. See `docs/fts.md` for syntax, the
scoped compression dependency patch, and local-project validation.

Observed on Linux x86_64, Rust 1.95.0:

| Probe | Observed |
| --- | --- |
| Database/Connection/Statement Send + Sync | Compile-time assertions pass |
| Current-thread Tokio | Works on a dedicated OS thread |
| Typed bindings, NULL, text, blob, RETURNING | Round-trip passes |
| Empty CTE result metadata | Four columns available before stepping |
| table_info / table_xinfo | Supported |
| Foreign keys | OFF by default: application must enable and verify |
| Explicit transaction failure + rollback | Original row preserved; connection reusable |
| Dropped transaction | Autocommit false until reuse; deferred rollback observed |
| Contended connection | Busy after configured 50 ms; no retries performed |
| 100,000 recursive result rows | ~0.95 s drained in debug, no result retention |
| 1 MiB blob | Materialized by engine before application budget check |
| Two-process viewer, WAL flag off/on | Read succeeded with parent handle open; NOT proof of safe live writes |
| Read-only reopen | Writes rejected; main and existing WAL bytes unchanged |
| Read-only missing path | Error, no file created |
| Corrupt file / Unix permissions | NotAdb / PermissionDenied |
| Closed standard SQLite fixture | Basic schema/row read through Turso passes |
| Local scratchpad/agenda/notification schemas | All-table browse, paging/filtering, typed edits and close rollback pass on synthetic Turso fixtures |
| Local clip-daemon native n-gram FTS schema | Open/reopen, indexed queries, generated edits, conflicts, constraint rollback and read-only byte preservation pass |
| Native FTS creation through the editor | Prepared CREATE INDEX/RETURNING/fts_match work; index creation and updates rejected in read-only mode |

## Decisions

- Dedicated serialized worker per database session, current-thread Tokio runtime.
  Persistent editor connection; separate browse and edit connections. GUI never
  holds engine handles. Enable and verify foreign keys on writable connections.
- Existing regular files only. Default read-only. No implicit creation, repair or checkpoint. Writable opening may migrate a
  DELETE journal to WAL before SQL execution; the access panel discloses this
  before opt-in and writable opening performs read-only schema preflight.
  Mode changes require clean close/reopen.
- **Only files not in use by another application are supported.** User must affirm
  exclusive use. A viewer opening successfully is not a concurrency guarantee.
  Do not copy a live main file without its WAL. No automatic snapshot feature.
- The local Rust wrapper patch exposes the pinned SDK's existing interrupt and
  per-statement timeout methods. Default deadline: 30 seconds, configurable after
  reopen; zero disables. Discard/close interrupt SELECT/EXPLAIN reads; writes drain
  unless their engine deadline is exceeded. CPU aggregate interruption, interrupted
  insert atomicity, manual rollback and connection reuse pass real-engine tests.
  Interrupt reports observed transaction state; it is not proof that preceding
  work was rolled back. Engine deadlines are cooperative and do not certify a
  hard process-memory or shutdown bound.
- Bounded application retention and queues, not a hard process memory limit.
  Engine allocates a complete row before delivery; hostile huge values/queries
  can exhaust memory. Use trusted files/SQL only until process isolation exists.
- Single statements only, validated with the matching Turso parser. No scripts.
- Generated edit transactions use a separate clean connection. Any error triggers
  explicit rollback; failed rollback is reported as unknown, never success.

## Unpassed release gates

Fault-injected I/O commit/rollback failure, resource exhaustion, multiprocess
writer stress, old tursotui-engine fixtures, full file-format coverage, and
cross-platform behavior are **not validated**. No old engine compatibility claim.
Generated columns, WITHOUT ROWID, custom types, sync, encryption and experimental
features other than index methods are disabled. Only native FTS index methods
have been qualified here; enabling the flag is not a blanket compatibility claim
for every possible index method. Existing live application stores were not used. This implementation is a Linux preview,
not a production-safety certification. New evidence must update this document.
