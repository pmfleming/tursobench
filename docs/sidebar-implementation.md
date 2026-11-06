# Sidebar database navigation implementation

The v9 HTML proposal is implemented in four independently validated commits.
Defaults: Ctrl+K stays actions-only, only the active database tree expands, and
Available shows all discovered and recent databases that are not open.

1. Open database rows: read-only workspace snapshots, RO/RW and transaction/draft
   status, select/close actions, four-slot meter. Keep legacy controls temporarily.
2. Unified search: application-level query and All open / This database /
   Databases scopes, keyboard selection, metadata loading for column search.
   Remove the floating object search and palette object entries.
3. Available and access review: compact discovered/recent rows, shared search,
   inline read/write consent, SQLite override, header checks, errors and retry.
   Remove the old chooser and switcher; retain discovery settings and forgetting.
4. Finish the sidebar: inline close confirmation, remove tabs while retaining
   database cycling shortcuts, update guide, desktop smoke checks and captures.

Validation: focused UI/engine regression tests per stage; the full UI suite,
workspace tests, formatting, native Wayland smoke and captures at completion.

Completed stages:

- `03393d1`: open database rows and capacity meter.
- `9f9e36e`: shared sidebar search and actions-only Ctrl+K palette.
- `e365bea`: Available files and inline access review; remove chooser/switcher.
- Final implementation stage: inline close, remove tabs, preserve cycling,
  update guides and add `scripts/capture-sidebar.sh` with 24 native captures.

Validation: formatting; warning-free workspace Clippy; 202 passing workspace
tests and one existing ignored test; native Wayland desktop smoke and visual
capture smoke. The final stage also fixes foreground selection checks being
overwritten by polling closed background workspaces, covered by reopening a
previously closed session while another closed workspace remains parked.
