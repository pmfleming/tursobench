# Repeatable Linux desktop smoke test

Run `cargo run --locked -p tursobench-ui -- /path/to/disposable.db` in a desktop
session (NixOS: `nix-shell --run 'cargo run --locked -p tursobench-ui'`). Use only a
closed disposable fixture. The engine spike generates its own temporary fixtures;
for a retained fixture see `tests/fixtures/generate.py` when available.

1. Confirm Open is disabled until exclusive-use acknowledgement is checked.
2. Choose an existing file or type its path; open read-only. Try a typo: no new file.
3. Select a table; page, click column headings to sort, apply a typed equality filter.
4. Inspect NULL, empty text, Unicode and blob cells. Long cells must stay bounded.
5. Execute `SELECT ?1` with `[{"type":"Blob","value":[0,255]}]`.
6. Execute `SELECT * FROM sqlite_schema WHERE 0`; column headers still appear.
7. Attempt a write in read-only mode; engine rejects it. Try two statements: rejected.
8. Run a recursive large-result SELECT, switch queries, discard results and close.
   UI remains responsive, stale rows do not replace newer results. Closing waits
   for execution; discard never promises interruption or rollback.
9. Exit through the native close button. No fixed-rate render loop when idle.

Automated headless egui render tests run in `cargo test`. Desktop steps require a
working graphics stack and are not equivalent to those unit tests. Record actual
platform smoke results in release notes; do not infer them from compilation.
