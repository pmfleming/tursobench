# Turso 0.8.3 compatibility and safety

## Baseline and reproduction

Run `nix-shell --run 'cargo run --locked -p engine-spike'` (or use Rust and
Python 3 directly). The harness creates and deletes only disposable fixtures.
No reference project code was copied. `turso = "=0.8.3"`, default Cargo features
OFF, all experimental builder flags OFF in production. Turso is the only engine.

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

## Decisions

- Dedicated serialized worker per database session, current-thread Tokio runtime.
  Persistent editor connection; separate browse and edit connections. GUI never
  holds engine handles. Enable and verify foreign keys on writable connections.
- Existing regular files only. Default read-only. No implicit creation, repair,
  checkpoint, or migration. Mode changes require clean close/reopen.
- **Only files not in use by another application are supported.** User must affirm
  exclusive use. A viewer opening successfully is not a concurrency guarantee.
  Do not copy a live main file without its WAL. No automatic snapshot feature.
- No engine interrupt exposed by this version. “Discard results” means suppress
  delivery, NOT stop execution or rollback. Drain statements to completion,
  including RETURNING, and report transaction state afterward. Shutdown waits for
  work and explicit rollback; a non-yielding engine call can delay it indefinitely.
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
Generated columns, WITHOUT ROWID, custom types, sync, encryption and other
experimental features are disabled. This implementation is a Linux preview,
not a production-safety certification. New evidence must update this document.
