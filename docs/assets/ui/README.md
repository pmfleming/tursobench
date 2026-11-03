# UI screenshots

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

## v5 tiled workspace

The `v5/` images are native egui captures at 1800 × 1000 on 2026-11-03,
using the synthetic 100,000-row `tests/fixtures/generate.py` fixture, read-only,
an isolated preferences directory and the existing painter icons. System captures
use this workstation's GTK black/blue named palette and system fonts; the light
capture uses the app's explicit Light override. No personal database was opened.

- `v5/tiled-system.png`: Data, Structure and Map with contextual ribbon and left navigator.
- `v5/expanded-map.png`: expanded map; left navigation remains visible.
- `v5/tiled-light.png`: the same tiled workspace in Light.

Capture support is opt-in for the existing native smoke runner: set
`TURSOBENCH_CAPTURE_DIR` to an output directory and run `--smoke-test` against a
fixture generated with 100,000 rows under Xvfb (1920 × 1080 screen). To reproduce a
system palette, copy only GTK `gtk.css`/`settings.ini` into the smoke runner's
isolated `XDG_CONFIG_HOME`; otherwise the semantic fallback applies. Normal smoke
runs do not read the user's appearance files or save screenshots.
