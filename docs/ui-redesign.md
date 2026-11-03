# TursoBench tiled workspace

The v5 redesign keeps the native egui controls and painter icons. The
[HTML design plan](redesign-v5-tiling.html) records the approved proposal.

## Appearance and windows

Follow system is the default for new preferences. Linux reads the desktop portal's
scheme, accent and contrast and supported named colors in GTK 3/4 configuration,
off the UI thread, with live updates. System fonts are reused. Explicit Light and
Dark remain available; legacy saved dark/light choices are preserved until changed
in Settings. Unsupported GTK color expressions use the neutral semantic fallback.

Every Data, Structure, Map, SQL, Result and pinned Record view is a window. The
right navigator remains visible when a window expands. Click a window or its
navigator entry to focus it; Ctrl Tab cycles, Ctrl Alt Enter expands/restores and
Escape restores after dismissing any active edit/dialog. Ctrl J opens SQL;
Ctrl 1/2 opens Data/Structure. Ctrl Enter runs SQL; Ctrl Shift Enter runs selection.
SQL execution opens a linked result without overwriting its editor. History and
bookmarks load into a new editor. Space from the grid opens a pinned record.

Windows split recursively with minimum sizes and data-width preferences. If a
split would make views unreadable, it becomes a tab group. Resizing back restores
the split tree. Drag a divider to resize; Retile recomputes an arrangement from
current content. Expanded view state never changes the saved split proportions.
The session retains at most 16 views. Closing an SQL draft, staged edit or active
request asks before discarding that view; SQL execution continues in the worker.

The top ribbon targets the focused window. Large labelled commands group Open
view, contextual actions and Workspace. On narrow windows secondary commands move
to More. Active transaction and staged-review controls remain visible regardless
of focus. All views share one session and transaction. Result cancellation targets
only that view's delivery; it never interrupts execution or rolls back writes.

Columns measure headers and up to 200 retained rows, bounded to 56–300 points;
spare viewport width no longer stretches a text identifier. Manual widths survive
resizing. Structure uses two columns only when its own tile has at least 720 points.
The map packs disconnected components compactly, fits occupied bounds, and keeps
manual pan/zoom and node positions. It never invents relationships.

Layouts save on managed close for up to 20 databases: split ratios, tab selection,
focused/expanded view and reopenable Map/Data/Structure/SQL identities. Data and
metadata reload sequentially after opening; no SQL executes on restoration.
SQL buffers, bindings, filters, row buffers and pinned records are session-only.
Use Saved queries for SQL that should survive restart. Invalid saved geometry is
rejected. Legacy preferences continue to load.

Native captures using synthetic fixture data: [system theme](assets/ui/v5/tiled-system.png),
[expanded map](assets/ui/v5/expanded-map.png), [light override](assets/ui/v5/tiled-light.png).

## Earlier v4 behavior (historical)

The following documents the earlier layout and unchanged grid mechanics. The v5
window, ribbon, theme and sizing behavior above supersedes its dock/overlay rules.

The v4 implementation moves actions to their scope and adds direct grid
interaction. Automatic background counts remain deferred: Turso 0.8.3's public
connection API has no query interruption/progress callback, and display budgets
cannot bound the execution of COUNT. Counts remain explicit full scans.

## Navigation and surfaces

| Surface | Behavior |
| --- | --- |
| Activity rail | Tables, Map, SQL toggle, History, Saved, Checks; Settings and Help at the bottom |
| Database name | Recent files, Open file and managed Close; selecting another file returns to access review after Closed |
| Sidebar | Tables/columns/declared links or History/Saved/Checks; tools no longer open floating windows |
| Table header | Stable Data/Structure labels, filters, retained range/table-total count, refresh, inspector and rare-action menu |
| Main workspace | Data, Structure, Map, SQL results or Settings; changing presentation does not discard drafts |
| SQL | Independent resizable split below the main grid, excluding the docked inspector |
| Inspector | Compact record fields, typed cell detail and declared Related list; explicit related counts |
| Status | Access, connection/unknown state, selection summary, completion, outcomes and staged Review; active transaction actions only |

The searchable catalog includes views even when the bounded map is incomplete.
The tree is the map's list alternative. Tree rows are left-aligned with disclosure
and column/key links. Schema refresh has its visible home in Tables' group menu.
Map retains Fit, Arrange, zoom/pan and original metadata completeness messages.
Settings has its own main page; leaving it restores the prior main route.

Ctrl K searches loaded objects and actions. Ctrl J toggles SQL; Ctrl 1/2 switches
Data/Structure; Ctrl F focuses filters (opening their compact popover as needed);
Ctrl E exports retained rows; Ctrl Shift C copies the retained result. F5 refreshes
the focused tree/table/map, or executes SQL while editing. F1 opens Help, Ctrl O
returns to database selection through managed close, and Alt Left restores Back.
BEGIN is available through SQL/the palette. Active read-only transactions still
offer Commit/Roll back with SQL hidden. Unknown outcomes remain explicit.

## Responsive layout

Breakpoints are logical egui points, not physical pixels:

- Below 1440: inspector overlays the right edge; Space from the grid toggles it
  and Escape closes it. The persistent sidebar stays available.
- 1440–1919: inspector defaults to a 320-point right dock.
- From 1920: inspector defaults to 360 points and Structure uses two columns.
- From 2400: selecting a parent can show a lower, independently retained related
  grid. Declared relationships select its tabs; no sample table names are assumed.

An explicit inspector visibility choice is saved per database, bounded to 20
paths. Older preferences load without migration intervention. Sidebar, SQL and
related panes resize; long fields have typed detail instead of tall repeated type
notes. Minimum window remains 720×480; both themes and scaled viewports are tested.

## Columns, filtering and selection

Columns initially measure headers and up to 200 retained values. Automatic widths
are 56–300 points; a non-manual text column fills remaining space with a 160-point
minimum. Drag edges to resize, double-click to refit, drag headers to reorder.
Header context menus provide fit, move and hide alternatives; More restores
hidden columns or refits all. Click sorting cycles ascending, descending, off.
Presentation order never changes canonical schema indices or stored edit/FK keys.
Exported complete results retain their canonical column order.

Type `column op value` and Enter to create AND chips. Quoted identifiers, escaped
quoted text, Unicode, exact integers, real numbers, NULL and explicit `x'00ff'`
blob literals are supported. Operators include `=`, `!=`/`≠`, `<`, `>`, `<=`, `>=`,
`contains`, `is null`, `is not null`. Declared affinity supplies a default; quoted
values remain text and chip editing exposes an explicit storage type. Invalid
input stays in the field without changing the query. Click chips to edit; use ×
or Backspace in an empty field to remove them. Filters remain bound values with
quoted identifiers and retain the 32-predicate / 1 MiB limit.

Right-click a cell to filter/exclude its exact stored value; Alt-click is the
shortcut. Header funnels show top values in at most 1000 **retained** rows, with
sample counts explicitly labelled. Facets are not whole-table statistics.

Drag or Shift-click to select a rectangle. Arrows move the grid cursor;
Shift+arrows extend; Page Up/Down moves 25 retained rows and can fetch at a window
edge. Ctrl C copies selected stored values in visible order as plain TSV, quoting
tabs/newlines/quotes and preserving empty fields, with a 2 MiB clipboard cap.
Status aggregates use numeric cells only. Integer sums/extrema are exact; averages
and mixed-real aggregates are labelled approximate; nonfinite results are explicit.
Text editing and popup focus take precedence over grid shortcuts.

Double-click or Enter on an eligible non-FK cell starts typed inline staging;
Enter stages and Escape cancels. A resolved FK's Enter follows the relationship;
use its context menu for typed editing. Blobs and long values use the full typed
inspector. New row opens the existing insert flow, preserving DEFAULT versus NULL.
No grid edit writes before explicit Review/Commit. Query results, views, keyless
and engine-managed storage retain their existing generated-edit restrictions.

## Scroll retention and result ownership

Table browsing fetches 100 rows at a time using deterministic generated OFFSET
queries. A next/previous page has its own ticket and temporary result. Only
complete pages with matching metadata/primary ownership are merged. A short
complete page ends paging; failed/limited/discarded delivery does not imply EOF.
The last OFFSET remains capped at 1,000,000. OFFSET is not a snapshot and deep
pages may be slow.

The table retention setting has a 200-row minimum so one 100-row page can be
reserved in flight. The retained window uses at most `clamp(row_limit,200,10000)-100`
rows and 4 MiB; one pending page can hold another 100 rows / 4 MiB. SQL retains its
exact configured 1–10000 row limit. Each cell remains capped at 64 KiB. Old pages
are evicted away from scrolling and can reload when scrolling back. Selection
and scroll positions are rebased or cleared by retained identity. Drafts pause
fetching/eviction, and pending fetches disable starting edits.

The range shows the current absolute retained window, which need not begin at 1.
Counts show a **table total**, including when filters are active. `?` performs an
explicit full scan; unknown is never shown as zero. Counts invalidate on writes,
editor execution, schema refresh and reopen. There are no automatic tree counts.

The table/SQL editor still share one primary retained result. Auxiliary page,
FK-preview and related-row results cannot replace it. Back retains typed filters,
sort, starting offset, retained-window length, view, scroll and matching row
selection. Exports/copies never rerun SQL and exclude staged overlays; the browser
export scope is now `table-window` rather than `table-page`.

## Relationship reads and limits

FK hover waits 350 ms before requesting a validated, bounded parent preview.
Composite/implicit keys, parent affinity/collation, self-links and NULL/unresolved
keys keep the existing metadata semantics. Only one context request is pending.
A FIFO cache holds up to 32 reads / 8 MiB, including keys, metadata and values;
keys exceeding the preview budget are skipped. A parent preview retains one row
within 512 KiB. Related lookups retain at most 100 rows and have their own scope
and limit notice. The compact lower grid shows up to 12 columns, with a full-view
navigation link for the rest.

Related exact counts are explicitly requested and use the same validated
parent-comparison predicates as reverse navigation. A capped lookup's length is
never presented as an exact count. Schema refresh, primary navigation and writes
invalidate context caches; stale tickets do not replace parent/child selections.

The engine's execution, close/reopen, read-only enforcement and unknown-outcome
contracts remain unchanged. Discard suppresses delivery; it does not interrupt or
roll back. Automatic full scans cannot be enabled until an isolated cancellable
execution mechanism passes the plan's gate. See `contracts/backend.md` and the
implementation plan for that remaining feature.
