# Local Turso project schema fixtures

These are exact SQL schema constants captured from local scratchpad, bar-daemon
(agenda/notifications) and clip-daemon checkouts. They are **not database copies**.
`provenance.json` records repository revisions, source-file SHA-256 values,
working-tree modification flags, schema hashes and corresponding MIT notices.

`crates/tursobench-db/tests/common/local_fixtures.rs` creates fresh disposable Turso
0.8.3 databases and inserts synthetic data for tests and the manual fixture example.
No live stores, clipboard content or private session data are read. Application
payloads satisfy database types/constraints, not necessarily application-level
serialization/semantic contracts; do not restore these fixtures into those apps.

Refresh deliberately using `scripts/capture-local-project-schemas.py PROJECTS_DIR`
and review the diff. Reproduce tests with `nix develop -c bash scripts/local-project-smoke.sh`.
See `docs/fts.md` for the compatibility scope and remaining limits.
