# Repeatable Linux desktop smoke test

Run `cargo run --locked -p tursobench-ui -- /path/to/disposable.db` in a desktop
session (NixOS: `nix-shell --run 'cargo run --locked -p tursobench-ui'`). Use only a
closed disposable fixture. The engine spike generates its own temporary fixtures;
for a retained fixture use `python3 tests/fixtures/generate.py /tmp/sample.db`.

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

Automated headless egui render/pointer-interaction tests run in `cargo test`.
`bash scripts/desktop-smoke.sh` runs the native app under Xvfb/Mesa against a
100,000-row disposable fixture and isolates preferences. With Nix:

```sh
nix-shell --run 'cargo build --locked -p tursobench-ui'
nix-shell --run 'nix shell nixpkgs#xvfb-run -c bash scripts/desktop-smoke.sh'
```

For writes, reopen a disposable file with read/write enabled. BEGIN, change a row,
verify browse does not see uncommitted changes, then ROLLBACK. Stage a cell, insert
with an omitted default and delete; verify no write before preview/COMMIT. Create a
key collision and verify atomic failure with drafts retained. Export a page and a
limited SQL result; inspect JSONL scope and end records. Check history/bookmarks
persist only under the application configuration directory. Close with an active
transaction and confirm cleanup.

Native dialog and interactive desktop checks still require a working desktop and
are not replaced by Xvfb tests. Record platform results in release notes.
