# UI screenshots

## Assistant and panel visibility — native Wayland

The `assistant/` captures were taken on 2026-11-13 with synthetic data and
isolated preferences under headless Weston/Mesa at 1366×768. The conversation is
a seeded visual fixture: no agent CLI runs during the capture and nothing leaves
the machine.

| Capture | Behaviour |
| --- | --- |
| [Consent](assistant/assistant-consent.png) | First use per database: who receives the data and the two sharing choices. |
| [Conversation](assistant/assistant-conversation.png) | Tool rows, a reply with a SQL block, and proposed SQL opened unrun in the SQL tile. |
| [Without sidebar](assistant/assistant-without-sidebar.png) | Sidebar hidden, assistant shown. |
| [Light](assistant/assistant-light.png) | The same in the light theme. |
| [Both hidden](assistant/panels-hidden.png) | The workspace takes the full width; the status-bar toggles remain. |
| [Settings](assistant/assistant-settings.png) | Agent choice, model, Claude Code's privacy choices and each database's sharing choice. |
| [Privacy](assistant/assistant-privacy.png) | Codex's privacy choices and fixed promises, opened from the consent card's link. |

Reproduce with:

```sh
nix develop -c bash scripts/capture-assistant.sh target/assistant-captures
```

## Spreadsheet editing — native Wayland

The `spreadsheet/` captures were taken on 2026-11-12 with synthetic data and
isolated preferences under headless Weston/Mesa. Changes are staged locally;
the capture pass never commits them.

- [Fill selection](spreadsheet/fill-selection.png): explicit type and value for a
  selected range, with the Stage action outside the scrolling content.
- [Paste new rows](spreadsheet/paste-new-rows.png): clipboard text, column/type
  mappings, converted preview and a fixed Stage action.

Reproduce after building the desktop binary:

```sh
nix develop -c cargo build --locked -p tursobench-ui
nix develop -c env TURSOBENCH_SPREADSHEET_CAPTURE=1 TURSOBENCH_BINARY=target/debug/tursobench bash scripts/capture-v8.sh target/spreadsheet-captures
```

The pass checks native Wayland, captures the workspace sizes/themes, then the two
editing dialogs, and waits for session cleanup before closing. Headless interaction
tests cover mapping, type errors, exact-value round trips, whole-operation undo,
constraint rollback, editor focus and a 720×480 dialog with a visible Stage action.

## Application-wide Settings — native Wayland

The `v11/` captures show the
[application-wide Settings window](../../guide.md#navigator-tools), captured on
2026-11-06 using synthetic databases and isolated application preferences.
Search feedback and the extra folder paths are seeded visual fixtures; the capture
scenes do not traverse those locations.

| Capture | Behaviour |
| --- | --- |
| [Appearance](v11/navigator-settings.png) | One Settings window over the database workspace; the four database tools remain grouped separately. |
| [Discovery](v11/search-settings.png) | Counted tabs, one folder input and list, and fixed search results. Available has Refresh only. |
| [Excluded](v11/settings-excluded.png) | Exclusions use the same folder controls, including Browse. |
| [Resources](v11/settings-resources.png) | Database-count slider, recommendation tick, Undo and memory estimate. |
| [Queries](v11/settings-queries.png) | Limits, recovery and independent all-databases clearing actions with counts, with no database connected. |
| [Queries, light](v11/settings-queries-light.png) | The same settings in the light theme. |
| [Discovery, 20 pt](v11/settings-discovery-large-text.png) | The window scales to a 1024×600 viewport; the input and results remain visible around the scrollable list. |

Reproduce with:

```sh
nix develop -c bash scripts/capture-sidebar.sh target/settings-captures
```

The script also checks viewport dimensions and that all Settings captures exist.
Headless interaction tests cover the icon navigation row below 480 px, controls
at 280 px and 20 pt, automatic saving, picker destinations, search cancellation,
and the per-database and all-databases clearing rules.

## Shared access icons and sidebar controls — native Wayland

The five `resources/` images were captured on 2026-11-06 after `83f8088`, with
one shared definition for access-mode icons, colours, tooltips and accessible names:
a lock for Read only and a pencil for Read/write. Native captures use synthetic
databases, isolated preferences and headless Weston/Mesa.

- `database-access.png`: refreshed after `a3f18cf`, with the access icons at the
  top right beside the database name, the selected mode highlighted, and no Access
  caption. The file picker remains above the list; the card contains access review and Open/Cancel.
  Check again appears only with a blocking file-check error.
- `sidebar-open-databases.png`: shared access indicators and integrated close buttons.
- `navigator-settings.png`: database-count slider, memory estimate, Reset, and the
  shared access icon in the status bar. The captured recommendation is recalculated
  from available memory on each normal launch.
- `sidebar-search.png`: grouped database/object/column search results.
- `access-dialog.png`: explicit confirmation before the status-bar access switch
  closes and reopens the session, using the target mode's shared icon.

Reproduce with `nix develop -c bash scripts/capture-sidebar.sh /tmp/resource-captures`.
Native visual smoke checks passed across six workspace sizes/themes, navigator
panels, dialogs, staged edits and database controls; capture dimensions were checked.
Formatting, Clippy and all 229 active workspace tests passed (one existing ignored
test), including mode-icon selection, picker consent and confirmed access switching.
The access-switch interaction test also passed after shortening the dialog title.
After moving the card icons into its heading, all 154 active UI tests passed; the six
connection tests passed again after the final text alignment adjustment, along with
formatting, Clippy and the native capture checks.

## Sidebar database navigation v9 — native Wayland

The review fixes refreshed these 24 captures using the isolated `db866fa` baseline
plus the sidebar corrections. Concurrent record/structure changes made later in the
shared checkout are outside this capture set. The access capture now shows the
read-only default with a fresh per-file SQLite compatibility choice.

These 24 native captures were generated on 2026-11-06 from the sidebar implementation,
using isolated preferences and two disposable synthetic databases under headless
Weston/Mesa. The files in `v9/` are application screenshots without compositing or
retouching. No personal databases are read or modified.

```sh
nix develop -c bash scripts/capture-sidebar.sh
# Optional output directory is the first argument.
```

- `sidebar-open-databases.png`: two live read-only sessions, one expanded schema tree,
  the slot meter, close controls and Available.
- `sidebar-search.png`: grouped object/column results across those sessions.
- `database-access.png`: Available files and inline access/SQLite review.
- `close-confirmation.png`: inline managed close while an edit remains staged.
- The remaining 20 images cover the six workspace size/theme states, navigator tools,
  staged-edit review and insert, action palette, export, outcomes, keyboard help,
  access switching, cell detail and discovery settings.

The opening pass checks typed queries, read-only enforcement, session cycling and
schema comparison. The visual pass reopens the primary fixture writable, stages a
local change and closes without committing it. Capture dimensions and the sidebar
scene files are verified by the script. Historical captures below retain their
original provenance.

## Current README — native Wayland

The `current/` images were captured on 2026-11-04 from the native Wayland app,
using the current code after `584edac`, headless Weston/Mesa, the desktop GTK
palette and isolated preferences. All seven images are 1800 × 1000. They are
application screenshots, without compositing or retouching.

The source is the same self-contained, offline Scratchpad `session.db` snapshot
used for the earlier demo below. Its SHA-256 is
`366a9d5388bb5fc64e1fa438566a838a89c2cab6017a7f99dfc1d3d7cbfc12de`.
The capture script checks that the source has no pending WAL data, copies it into
a temporary directory and verifies matching hashes. The live Scratchpad database
is never opened. The source snapshot remains unchanged; no database is committed.

- `workspace-dark.png`: all five tiles, with real browsing and linked SQL results.
- `table-data.png`: expanded `document_history` metadata browsing.
- `relationships.png`: the ten-table graph and its declared foreign keys.
- `structure.png`: original DDL, declared types, indexes and key actions.
- `sql-results.png`: the README history/snapshot join and twelve result rows.
- `edit-review.png`: an increment to `edit_seq` staged on the temporary copy;
  the preview shows its diff, SQL and typed bindings. No edit is committed.
- `database-chooser.png`: explicit read-only access and current opening controls.

The first five scenes use engine-enforced read-only access. The edit scene
reopens only the disposable copy writable, stages one metadata change, then
closes without committing it. The final scene returns to the chooser.

Reproduce using a closed, checkpointed copy with the same session schema:

```sh
nix develop -c bash scripts/capture-readme.sh /path/to/offline/session.db
# Optional second argument selects the image output directory.
```

The README banner is authored in `../tursobench-banner.svg`, rendered to PNG at
2400 × 600 with Inkscape. It embeds the current canonical app icon and the chosen
Joyce quotation; it restores the earlier emerald header treatment. The previous
banner and screenshots remain as historical references.

## Earlier README — real session database

The `sessions/` images were captured on 2026-11-03 from the native TursoBench
application on this NixOS laptop. The captured binary is the v8 build from
`9a89c23`. The images use the desktop GTK palette and system fonts. Every image
is 1600 × 1000. These images predate the current opening, shared-access and native
Wayland updates.

The data comes from the local Scratchpad `session.db`, not a synthetic fixture.
The active database and its WAL were copied after consecutive byte-identical
reads. A backup of that offline copy supplied a self-contained capture database.
TursoBench opened the capture database read-only. No source application was
stopped, and no source database write was issued. Database files are not committed.

The captured session contains 10 tables, 6 explicit indexes, 130 buffer identifiers,
130 document-history pointers, 198 history entries, 130 snapshots and 139 blob rows.
The snapshot-size sum is 2,307,840 bytes. Its largest snapshot is 611,668 bytes.
Screens show actual identifiers, integer metadata and blob previews. The SQL example
reads size and digest metadata; it does not decode application payloads.

- `workspace-dark.png`, `workspace-light.png`: all five native tiles.
- `table-data.png`: expanded session metadata browsing.
- `relationships.png`: all ten tables and their declared FK edges.
- `structure.png`: original session DDL, columns, indexes and FK actions.
- `sql-results.png`: the README's history/snapshot join and its twelve result rows.

Captures use the normal desktop UI. Search, expansion, theme switching and SQL
execution were driven with native input events under Xvfb/OpenGL. The application
used isolated preferences and copied desktop theme inputs. The images were not
redrawn, composited or retouched.

## v8 interface reference captures

The `v8/` images are native egui captures from 2026-11-03, using this laptop's
Home Manager GTK palette and system fonts. The four interface roles use the
desktop accent, neutral surfaces, warning colour and relationship colour
(link/info/success). Here those sources are blue, black, amber and green.
The Light override adapts desktop neutrals and coloured text for legibility.
These are actual application screenshots, not HTML renders.

Reproduce all 22 captures with:

```sh
nix develop -c bash scripts/capture-v8.sh
# Optional output directory:
nix develop -c bash scripts/capture-v8.sh /tmp/tursobench-captures
```

The script copies only GTK theme inputs into an isolated configuration directory.
It creates a disposable SQLite compatibility fixture with 1,000 generated items,
three projects, two accounts, foreign keys, status/priority/progress fields and a
view. No personal database or application preferences are read or changed.
The initial smoke pass opens read-only and checks typed SQL, write rejection and
clean close. The visual pass reopens the disposable fixture writable, stages a
local change and shows editing dialogs, then closes without committing drafts.

- `workspace-dark.png`, `expanded-map.png`, `laptop-dark.png`, `laptop-light.png`,
  `minimized-tiles.png` and `widescreen.png`: six workspace theme/size states.
- `staged-workspace.png`, `review-dialog.png`, `insert-dialog.png`: local edits
  and their review/insert surfaces.
- `navigator-{history,saved,checks,settings}.png`: each navigator tool.
- `command-palette.png`, `export-dialog.png`, `execution-outcomes.png`,
  `keyboard-reference.png`, `access-dialog.png`, `cell-detail.png`,
  `close-dialog.png`: custom windows.
- `database-chooser.png`, `search-settings.png`: database opening and discovery.

The main workspace and expanded map use 1800 × 1000; widescreen uses 3440 × 1440.
All other captures use 1366 × 768. Screenshot generation is opt-in; ordinary
smoke runs retain their original checks and do not save images.

## Fixed workspace v6 (historical)

These historical native captures show the fixed workspace implementation in
commit `0afd0ba`, captured on 2026-11-03 under Xvfb/Mesa with isolated preferences.
These are actual application screenshots, not HTML mockups.

The disposable fixture starts with the 100,000-row
`tests/fixtures/generate.py` database. It adds three synthetic projects and
`project_id`, `status` and `priority` columns to `items`, including a declared
foreign key to `projects`. The first 200 items receive generated project links,
status values and priorities. The app opens the fixture read-only; no personal or
production data is used.

- `v6/laptop-dark.png` and `v6/laptop-light.png`: all five fixed tiles at
  1366 × 768. The README selects the image matching the page's color scheme.
- `v6/workspace-dark.png`: the full workspace at 1800 × 1000.
- `v6/expanded-map.png`: relationships expanded at 1800 × 1000, retaining the navigator.
- `v6/minimized-tiles.png`: Record, Structure and SQL collapsed to headers at 1366 × 768.
- `v6/widescreen.png`: all five tiles at 3440 × 1440.

Captures use the app's explicit Light/Dark modes and green fallback palette.
The app also supports following the desktop's scheme, accent and supported GTK
colors. Screenshot capture is opt-in: set `TURSOBENCH_CAPTURE_DIR`, isolate
`XDG_CONFIG_HOME`, and run `--smoke-test` against a disposable 100,000-row fixture
under Xvfb with a 3440 × 1440 screen. Normal smoke runs do not save screenshots.

The README banner embeds the current transparent [app logo](../../../packaging/tursobench.png).
Its source and generation provenance are recorded in
[packaging/icon-source.txt](../../../packaging/icon-source.txt).

## Earlier table browser (historical)

Captured from the native Linux application at commit `bf10b3e` on 2026-11-02,
at 1440 × 810 pixels, using the emerald dark theme and an isolated preferences
directory. These are actual app captures, not mockups.

All records in `team-projects.db` were generated for these screenshots. The
database contains three related tables (`people`, `projects`, `tasks`), four
fictional people, three projects and 24 tasks. It was opened read-only with the
explicit SQLite compatibility override; no personal or production data was used.

- `table-browser.png`: task rows, schema sidebar and foreign-key links.
- `relationships.png`: the three tables and their foreign-key relationships.
- `sql-editor.png`: syntax-highlighted SQL and its 18 result rows.

The query shown is:

```sql
SELECT t.title, p.name AS project, u.name AS assignee, t.status
FROM tasks t
JOIN projects p ON p.id = t.project_id
JOIN people u ON u.id = t.assignee_id
WHERE t.status != 'Done' ORDER BY p.name, t.id;
```

## v5 tiled workspace (historical)

The `v5/` images are native egui captures at 1800 × 1000 on 2026-11-03,
using the synthetic 100,000-row `tests/fixtures/generate.py` fixture, read-only,
an isolated preferences directory and the existing painter icons. System captures
use this workstation's GTK black/blue named palette and system fonts; the light
capture uses the app's explicit Light override. No personal database was opened.

- `v5/tiled-system.png`: Data, Structure and Map with contextual ribbon and left navigator.
- `v5/expanded-map.png`: expanded map; left navigation remains visible.
- `v5/tiled-light.png`: the same tiled workspace in Light.
- `v5/wide-table-below.png`: an 11-column table opens across the bottom of the
  existing group, keeping all columns visible. The capture fixture adds a
  synthetic `wide_records` table with 100 rows: integer ID, eight text fields,
  real amount and integer active flag. The v5 smoke capture recognized
  this table and recorded the extra layout. All four captures use this fixture.

These v5 captures predate the fixed workspace. The current smoke capture sequence
uses the workspace filenames above and adds the v8 interface states; it no longer
captures movable-window layouts.
