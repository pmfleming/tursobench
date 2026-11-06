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


Review remediation (five separate commits):

1. `86d6564` keeps disconnected drafts, exports and unknown outcomes reachable,
   including close confirmation without a session and retained-draft review.
2. `2b10356` restores database focus after close, cancelled access and picker
   dismissal; clean writable sessions close directly and the last close resets access.
3. `f5f7c71` corrects Escape, empty-query Enter and first-arrow/page selection.
4. `db866fa` caches discovery/open identities and virtualizes Available rows; a
   500-entry regression checks zero repaint identity probes and bounded row rendering.
5. Presentation alignment uses shared engine marks and filename/folder search rows,
   disables unavailable tools, names the active database, adds slot pips, collapse and
   close progress, and implements This database schema filtering. The source HTML
   proposal is now versioned alongside the implementation.

The review's seven saved reproductions are retained as regression tests in
`crates/tursobench-ui/src/app/interaction_tests/sidebar_review.rs`, with further
coverage for picker dismissal, last-close defaults, retained outcomes and keyboard paging.

Final review validation used an isolated checkout at `db866fa` plus these changes,
so concurrent record/structure edits in the shared checkout were preserved and
excluded from the sidebar verification. Formatting, warning-free workspace Clippy,
219 active workspace tests (one existing ignored test), the engine probe and workspace
build passed. Native Wayland desktop, startup discovery, all four local-project
fixture smoke tests and the refreshed 24-image capture set passed.
