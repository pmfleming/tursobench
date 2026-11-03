# Repeatable Linux desktop smoke test

Run `cargo run --locked -p tursobench-ui -- /path/to/disposable.db` in a desktop
session (NixOS: `nix-shell --run 'cargo run --locked -p tursobench-ui'`). Use only a
closed disposable fixture. The engine spike generates its own temporary fixtures;
for a retained fixture use `python3 tests/fixtures/generate.py /tmp/sample.db`.

1. Select a database and confirm Open is disabled until exclusive-use acknowledgement is checked.
   The chooser should show cached/recent files immediately and append startup
   discoveries while remaining responsive. Filter by name/path; Ctrl+O returns to
   the chooser. Down enters results, Up/Down moves the marker and Up from the
   first result returns to search. Enter selects the file for review; typing from
   results resumes search at the retained cursor. PageDown reveals later rows.
   The folder icon inside search (also Alt+Enter) opens the native file dialog
   and remains available during discovery. In Search options, stop, refresh, add a
   search folder and exclude one. Restart to check cached entries, successful-open
   history and the startup toggle. Changing roots must not change the selected
   database or acknowledge exclusive access.
2. Choose an existing file or type its path; changing the path must restore
   read-only mode and clear the exclusive-use acknowledgement. Confirm it and open.
   Cancel the picker: keep the previous path and show a no-selection message.
   Try a typo: no new file. Close while an open is pending and fails, then retry;
   the app must not remain stuck closing. Picker failures must show feedback and
   leave manual path entry available.
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
100,000-row disposable fixture and isolates preferences. It also loads and renders
the native-metadata map and switches Data/Structure without replacing the retained table page. With Nix:

```sh
nix develop -c cargo build --locked -p tursobench-ui
nix develop -c bash scripts/desktop-smoke.sh
nix develop -c bash scripts/discovery-smoke.sh
```

For writes, reopen a disposable file with read/write enabled. BEGIN, change a row,
verify browse does not see uncommitted changes, then ROLLBACK. Stage a cell, insert
with an omitted default and delete; verify no write before preview/COMMIT. Create a
key collision and verify atomic failure with drafts retained. Export a page and a
limited SQL result; inspect JSONL scope and end records. Check history/bookmarks
persist only under the application configuration directory. Close with an active
transaction and confirm cleanup.

For the workbench additions, also check on a disposable database:

- Search table/index/trigger names, inspect/copy DDL, open columns/index/FK details,
  and run explicit row count and read-only diagnostics.
- Execute named bindings (including repeated names), missing/extra bindings, a
  selection and a statement at a Unicode cursor. Semicolons in strings/trigger
  bodies must not split execution; full-buffer Run still rejects scripts.
- Try highlighting/completion, including quoted text and comments; explain a
  DELETE in both plan and bytecode modes and verify rows were not deleted.
- Follow composite foreign keys, go back to the original page/filter, and try a
  NULL reference. Right-click headers/cells, click record detail, double-click typed editing, copy TSV
  and typed JSON, and inspect raw JSON text with large numbers/duplicate keys.
- Clone a row (keys omitted), stage/revert cells and deletions, remove an individual
  preview operation, and verify overlays disappear without issuing SQL writes.
- Name/search/rename/delete a bookmark; check scoped timed history and slow filters.
  Loading saved SQL must clear bindings. Toggle theme and open F1 help.
- Export every format to a NEW file, check scope notices and extension enforcement,
  and verify existing files cannot be overwritten. CSV/TSV require skipping the
  metadata line; SQL export must name its target explicitly. Exports/copies retain
  engine values, not staged overlays.

For the [responsive redesign](ui-redesign.md), also check both themes at 720×480, 1280×720,
1366×768, 1920×1080 and 3440×1440, plus a high-DPI/960×640 logical viewport:

- Active transaction actions, especially ROLLBACK, remain visible even with SQL
  hidden. the sidebar, header overflow menu and tool windows fit compact layouts.
- Select/copy/edit a cell in the dock, switch to record/structure, pop out and dock
  again. Verify stage/preview/remove/commit and DEFAULT/NULL remain accessible.
- Diagram includes disconnected tables, composite/self/cyclic FKs and engine-owned
  storage. Select/zoom/pan/arrange/find/fit; click to open the full-height Data/Structure workspace. Unresolved and
  limited metadata must be explicit, and generated edits to engine storage disabled.
- Keep local drafts while loading the diagram: rows/drafts must not be replaced.
  Browse links must respect the draft guard. Explicit DDL selection must work while
  Diagram is visible. Refresh metadata after SQL/DDL; never infer missing targets.
- Closing/discard must stay pending until actual worker acknowledgement. Unknown
  staged outcomes must remain unknown in the review window and must not auto-retry.

Native dialog and interactive desktop checks still require a working desktop and
are not replaced by Xvfb tests. Record platform results in release notes.

- Ctrl K searches tables/columns/actions; arrow keys and Enter activate matches.
  Ctrl J shows SQL without replacing the retained result. Query results have their own label.
- Combine typed filter chips, remove them, follow incoming/outgoing composite
  references and return to the prior page. NULL references must not navigate.
- Close/reopen to verify saved map camera/positions. Switch access via the badge;
  the old session must acknowledge closure before a new session opens.

- At 1280×720 with SQL and detail closed, at least 15 complete table rows should
  be visible. The sidebar stays visible with inspectors and every tool window.
- Run a query from a filtered/sorted table, then use Back. It must restore the
  browse destination; arbitrary query rows must remain noneditable.
- Count from More. The count appears in the sidebar without replacing table rows
  or drafts. A later write invalidates the count.
- Open a menu, press Escape, then Ctrl K. Menus must close, search must receive
  focus, and no result or draft should change. A read-only BEGIN remains visible
  with SQL hidden and still offers ROLLBACK.

## v4 acceptance additions

- Exercise each rail item. History/Saved/Checks replace sidebar content; Settings
  replaces main content. Return without replacing rows, drafts, SQL or map camera.
- Click the database name: recent selection waits for Closed and requires fresh
  access review. Cancel writable close, then select another file; no pending path
  may leak from the cancelled switch.
- At compact width, Space on the focused grid opens the inspector overlay; Escape
  closes it. Resize through 1440/1920 logical points. Explicit visibility survives
  restart; Structure has two columns from 1920. SQL excludes the right dock.
- Refit/resize/reorder/hide columns, including PK/FK fields. Sort through off.
  Edit/filter/follow a visually moved column and verify the canonical target.
- Enter `n >= 2`, quoted names/text, NULL and blob tokens; edit chip type, remove
  chips with ×/Backspace, and verify invalid input keeps the prior result.
  Header facets must label their retained sample; `?` counts a table total.
- Drag/Shift-click and Shift+arrows select a rectangle. Cross virtualized row
  boundaries with Page Down; Ctrl C preserves empty values, tabs, Unicode and
  full integers. Text inputs must keep their own arrows/Enter/clipboard handling.
- Enter/double-click stages an eligible inline edit; Escape cancels. No SQL write
  occurs until Review/Commit. Blobs and long values remain available in typed detail.
- Scroll through several pages beyond retention, scroll back to reload, and use
  Back from a query. The retained range, cursor and scroll must remain coherent.
  A failed/limited fetch must not imply EOF or clear the current grid. Drafts
  pause fetching; queries/navigation must ignore superseded page replies.
- Hover composite FKs to peek. Request Related counts explicitly. At 2560/3440
  logical points, select parent rows and switch lower-grid relationship tabs;
  delayed replies must not replace the primary grid. Try NULL keys and stale DDL.
- Idle transaction buttons are absent with SQL open. BEGIN through SQL/the
  palette still exposes Commit/Roll back globally, including a read-only session.
- Automatic counts remain intentionally absent until cancellable execution is
  available; no UI timer, busy timeout or retention cap claims to stop a COUNT.

## Wayland background workspace regression

TursoBench pins eframe 0.37.0. Its Glow presenter selects nonblocking EGL swaps
on the actual Wayland backend and calls `pre_present_notify` for compositor-paced
redraws. The event loop sleeps while waiting and runs logic-only updates when a
redraw is overdue. These pieces keep hidden windows answering desktop pings;
disabling VSync alone can cause uncapped rendering. Other backends retain
upstream VSync behavior. The theme observer also avoids repaint requests for
unchanged palettes, while retaining delivery/retry for real changes.

The opt-in presentation probe uses the same Glow configuration, requests
continuous redraws, reads no database/preferences, reports frame counts every
second, and requests close after 30 seconds:

```sh
nix develop -c cargo build --locked -p tursobench-ui --example presentation_probe
nix develop -c timeout 40 target/debug/examples/presentation_probe
```

On a native Wayland desktop, move only the probe window to an inactive workspace,
show it, hide it again, and show it again. It must resume rendering at the
compositor's pace without a hidden-workspace busy loop or unresponsive dialog.
Use `WAYLAND_DEBUG=client` to inspect `xdg_wm_base.ping`/`pong` pairs; retain logs
locally. Repeat with the real app using isolated XDG configuration/state/cache
directories and `{"discovery":{"enabled":false}}` in its test
`tursobench/preferences.json`. Check idle behavior and native close as well.

Local validation on 2026-11-03, Hyprland 0.56.2 on a 75 Hz output:

- The real app and probe both passed hidden startup, show, hide, show, native
  close. All 30 observed compositor pings had matching pong serials.
- Each settled seven-second hidden sample used at most one process CPU tick.
  The continuous-redraw probe resumed at 75 frames per second when shown.
- The app test changed only its isolated GTK configuration while hidden; it
  remained responsive. Existing user databases/preferences were not opened.
- Workspace tests, strict all-target Clippy, Xvfb desktop/discovery/project
  smokes, and the callback-based screenshot capture smoke passed.

These short native checks cover this compositor/driver, not every Linux desktop.
Local evidence is under `target/egui037-wayland/`, `target/egui037-validation.log`,
`target/egui037-desktop.log`, and `target/egui037-capture.log`.

Rebuild and restart the executable being used. An already installed preview
binary does not inherit the dependency upgrade from this checkout.
