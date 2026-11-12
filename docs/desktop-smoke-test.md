# Repeatable Linux desktop smoke test

Run `cargo run --locked -p tursobench-ui -- /path/to/disposable.db` in a native
Wayland session (NixOS: `nix develop`). Hyprland is the primary acceptance desktop. Use only a
closed disposable fixture. The engine spike generates its own temporary fixtures;
for a retained fixture use `python3 tests/fixtures/generate.py /tmp/sample.db`.

1. Select a database and confirm Open becomes available after header inspection,
   with no exclusive-use acknowledgement. A legacy writer must produce a locking
   error; a compatible shared peer must report shared WAL in connection status.
   The chooser should show cached/recent files immediately and append startup
   discoveries while remaining responsive. Filter by name/path; Ctrl+O returns to
   the chooser. Down enters results, Up/Down moves the marker and Up from the
   first result returns to search. Enter selects the file for review; typing from
   results resumes search at the retained cursor. PageDown reveals later rows.
   The folder icon inside search (also Alt+Enter) opens the native file dialog
   and remains available during discovery. In Settings → Discovery, stop, refresh, add a
   search folder and exclude one. Restart to check cached entries, successful-open
   history and the startup toggle. Changing roots must not change the selected
   database or change write access.
2. Choose an existing file or type its path; changing the path must restore
   read-only mode. Open directly; admission locks determine access.
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
`bash scripts/desktop-smoke.sh` runs the native app under headless Weston/Mesa against a
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
- Reorder/hide columns and paste a spreadsheet rectangle with Ctrl+V. Verify exact
  integers, quoted multiline text, empty text, NULL (`\N`) and literal `"\N"`.
  Reject a mismatched rectangle or invalid number without changing earlier drafts.
  Use Edit cells → Paste as new rows to map columns and preserve unmapped defaults.
  Fill a range, stage NULL with Backspace, and undo each whole operation with Ctrl+Z.
  Check that the Stage button stays visible at 720×480 and that inline/SQL editors
  keep their normal clipboard and undo behavior. Commit a constraint-conflicting
  paste and verify the entire transaction rolls back with drafts retained.
- Name/search/rename/delete a bookmark; check scoped timed history and slow filters.
  Loading saved SQL must clear bindings. The heading trash clears only the selected
  database and preserves other files and older unscoped entries. Settings → Queries
  shows all-databases counts and independent clear actions, including with no file
  open. Verify disabled buttons when empty. Toggle theme and open F1 help.
- Open Settings from its separate footer button, Ctrl+, and the command palette.
  Check all four pages, automatic saving on focus loss/drag release, remembered page
  on reopen, and Escape without changing the selected database tool. Resize below
  480 px to check the icon navigation row. Check 20 pt text at 1024×600.
  In Discovery, switch tabs, browse/add/exclude a folder, scroll a long list and
  confirm the input and results remain visible. Available offers Refresh/Stop only.
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

Native dialog and interactive desktop checks still require a working Wayland
desktop and are not replaced by Weston tests. Record platform results in release notes.

## Wayland acceptance

All desktop, discovery, local-project, installed-package and capture scripts use
`scripts/wayland-run.py`. It starts an isolated headless Weston GL compositor,
unsets `DISPLAY`/`WAYLAND_SOCKET`, supplies a private Wayland socket and requires
the actual application display handle to be Wayland. Each run retains compositor
and client logs in `target/wayland-smoke/`; CI uploads these when a check fails.
Readiness, execution and cleanup are bounded. No user compositor configuration
is loaded or changed. Weston must provide its headless backend and desktop shell;
the desktop shell allows capture checks to resize the window between scenes.

On Hyprland, also verify the following using isolated preferences and fixtures:

- `hyprctl clients -j` identifies the app as native Wayland (`xwayland: false`)
  with class/app ID `tursobench`, matching `tursobench.desktop` and its icon.
- Real keyboard typing, shortcuts, Unicode and clipboard copy/paste work.
- The portal file picker opens, returns a selection and handles cancellation.
- Tiled/floating resize and fractional scaling keep text, pointer positions and
  all controls usable; restore the test window rather than changing user settings.
- Hide/show across workspaces, background pings, idle CPU and native close pass
  the presentation regression below. Do not disable compositor pacing.

The default app selects Wayland explicitly. `--no-default-features --features x11`
selects X11 explicitly; adding `--features x11` to the default build permits both
and uses upstream Wayland-first selection. A separate CI job checks the X11
build; primary smoke tests always run Wayland without an X server.

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

## Fixed workspace acceptance

- Relationships is above Table data; Record, Structure and SQL occupy the right
  column. All five tiles appear without a global ribbon or activity rail.
- Minimize every tile in turn, restore through its header, expand each tile and
  restore. Other minimized states persist; the database navigator stays visible.
  Dragging a ribbon must not move a tile. Repeat at 1366×768 and 3440×1440, in
  light and dark modes and with desktop scaling.
- Click between tiles, then immediately use their icon actions. Tooltips and
  accessible names identify each action; narrow ribbons move actions into More.
- Select different table rows and edit in Record. Data shows the same draft.
  Commit directly without opening review; separately inspect Diff / SQL. A failed
  commit retains edits. Minimized Data and expanded Record keep commit reachable.
- Query, select a result, rerun with Ctrl Enter and switch editors through More.
  Background results must never overwrite another table, editor or draft.
- Apply a filter matching a row outside the retained page. It must return that
  row from the whole table. Sampled facet counts remain labelled as samples.
- Override automatic status/priority styling in a column's context menu. Raw
  clipboard/export values and filter semantics must be unchanged.
- Exercise navigator footer tools and database switching. Return without losing
  drafts, SQL or camera position; cancellation must clear pending database paths.
- Close/reopen with minimized and expanded tiles. Visibility is restored while
  SQL text, bindings and row data are absent from saved layout preferences.

## Earlier acceptance checks (historical layout)

The following checks retain useful data and keyboard cases. Fixed workspace
behavior above supersedes the old rail, overlay and movable-window assumptions.

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
directories. Seed `{"discovery":{"enabled":false}}` into its test
`tursobench/preferences.db` using the `preferences` example (see
`scripts/discovery-smoke.sh`); the old JSON preferences file is no longer read.
Set `TURSOBENCH_EXPECT_BACKEND=wayland` to verify the native display handle.
Check idle behavior and native close as well.

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

## Gap-closure workflows

The native desktop smoke now also executes/renders a sampled profile and PRAGMA
report, opens a second database, switches both workspaces, captures/renders their
schema comparison, and closes both sessions. For interactive acceptance on disposable
files, verify these additional paths:

- Edit SQL, close/reopen, and recover text/cursor without execution or bindings;
  disable/clear recovery from Settings.
- Complete an alias/CTE with Ctrl Space; inspect EXPLAIN tree/raw/bytecode and open
  a suggested index draft without executing it.
- From Checks, compare exact and prefix-sampled profiles and read the sampling label;
  apply a bounded administration setting in writable mode and inspect readback.
- Keep an editor transaction and table drafts in different database tabs; switch
  with Ctrl PageUp/PageDown and search cached objects with Ctrl P.
- Press F7, select desired and target databases, capture and expand a changed object.
  Open an additive migration draft in the target, review it, then Run all. Change the
  target schema before executing another draft and verify stale-schema rejection.
  Inspect manual guidance for removed columns/objects and changed constraints.
