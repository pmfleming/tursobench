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
