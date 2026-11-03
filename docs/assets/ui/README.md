# UI screenshots

## Current fixed workspace (v6)

The README uses native captures from the fixed workspace implementation in
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

The README banner contains the current [app logo](../../../packaging/tursobench.png).
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
produces the v6 filenames listed above; it no longer captures movable-window layouts.
