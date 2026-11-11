# Quality audit and refactor — 2026-11-11

Baseline: `1d7da3c12f8b7151de3687235ae8612763cd91df` after the six performance optimizations.
Measured with the locally available `/home/laufan/Projects/rust-quality-lens`
at `824f7b7118a3f30348e1184dc1a4f80762ad58df`. The frozen executable's SHA-256 is
`a9a7f6ac89b7f3a060d26a3b179b6fbed5f9338b8eb4815f93a5beecc0da53a3`.
Both measurements use the existing source roots (`crates` and
`tools/engine-spike/src`), including tests/examples; no new exclusions, lint
suppressions or stack-limit overrides were introduced.

| Measurement | Before | After |
| --- | ---: | ---: |
| Maximum cognitive complexity | 102 | 55 |
| Sum of cognitive complexity | 6,057 | 5,908 |
| Maximum cyclomatic complexity | 59 | 58 |
| Sum of cyclomatic complexity | 6,617 | 6,609 |
| Sum of function Halstead effort | 48,385,494 | 47,093,159 |
| Duplicated source lines | 3,269 | 3,092 |
| Sum of module locality risk ↓ | 433.5 | 430.5 |
| Mean observed module leverage ↑ | 27.78 | 27.94 |
| Authored Rust modules | 99 | 97 |
| Physical Rust lines | 42,525 | 42,389 |
| Literal `.clone()` calls | 459 | 455 |
| Literal `.cloned()` calls | 24 | 25 |
| Observed panic paths, including tests | 512 | 500 |
| Test/helper function lines | 12,278 | 12,192 |
| Wildcard imports | 36 | 31 |
| Unsafe blocks | 1 | 1 |
| Catalogued tests (including one ignored) | 244 | 238 |
| Executed workspace tests | 243 | 237 |
| Lines coverage (%) | 85.06 | 85.13 |
| Regions coverage (%) | 84.62 | 84.69 |
| Functions coverage (%) | 86.74 | 86.87 |

The changes address these findings:

- **Schema control flow:** share the wide/compact section dispatcher, isolate
  loading and tab rendering, and borrow column metadata through a row model.
  Find the first relevant foreign key instead of allocating every match. Preserve
  pending actions when a section returns no action: previously the compact
  Indexes and Triggers tabs lost the “Load structure” click. The existing action
  test now exercises loading across all four applicable tabs.
- **Frame-time copies and reuse:** render open-database rows from borrowed
  workspaces instead of owned snapshots, and clone browse state only when a jump
  is requested. Share sidebar target eligibility between rendering and activation.
  Text painting belongs to common presentation, SQL highlighting to the editor;
  result viewers and editors share their text-edit configuration.
- **Ownership and dead paths:** session and result initialization now belong to
  their state owners, removing redundant resets after default construction.
  Profiling/administration share the Database checks module; large-value viewing
  belongs alongside result editing. Remove obsolete untiled grid branches after
  checking production callers, and keep capture stages together in the native
  smoke state machine. The application hub remains; locality and leverage gains
  are modest advisory improvements, not a claim that coupling is solved.
- **Profile query depth:** the retained profile test now covers empty input,
  sampling, typed statistics, frequencies, invalid requests, the 24-column limit
  and single-column fallback for wider tables. Executing the supported 24-column
  case reproduced a stack overflow in the previous 360-arm UNION. A 14-row metric
  CTE and CASE expression reduce this to two UNION arms per column while retaining
  one aggregate source pass per column and the narrow materialized projection.
  The regression passes with the normal test stack.
- **Coverage with less test code:** combine overlapping sidebar keyboard,
  disconnected-workspace, staged-edit and quit-confirmation journeys, reuse click
  helpers and consolidate profile fixtures. Preserve their distinct assertions;
  the keyboard journey now uses the actual pane expansion API. Catalogued tests
  fall by six and test/helper function spans by 86 lines. Combined `.clone()` and
  `.cloned()` sites fall from 483 to 480; these literal counts are not allocation
  measurements. The existing unsafe locking boundary remains unchanged.

Line coverage changes from 25,092/29,500 to 25,007/29,376; removing dead/duplicate
code reduces both covered lines and the denominator. Region coverage changes
from 39,239/46,371 to 39,250/46,346, and function coverage from 2,349/2,708 to
2,375/2,734. Coverage percentages increase slightly; no branch or mutation
coverage was measured. No runtime benchmark speedup is claimed.

Syntax inventory remains partial because three `thread_local!` invocations and
one `compile_error!` are unexpanded, with zero unreadable files or syntax errors.
Semantic identity resolves 1,647/1,680 references before and 1,668/1,701 after;
33 references remain unresolved in each run. Leverage/locality therefore describe
observed dependencies. Dynamic coverage input is complete. Test/helper volume
counts the union of function spans in test modules, integration paths and
catalogued tests; it is a separate measure from physical Rust lines.

Validation passed: formatting, workspace Clippy across all targets with warnings
denied, workspace tests across all targets (237 passed, one pre-existing ignored
manual frame profile), workspace build, engine spike, native Wayland desktop,
startup discovery, and schema/desktop smokes for all four synthetic local projects.
The visual capture smoke also passed after the smoke-state refactor.

Local evidence is under `target/quality-refactor-20261111/`: frozen `rqlens`,
`before/` and `after/` JSON, `comparison.json`, `compare.py`, `lens.sh`, final
measurement/validation logs, and the interactive `comparison.html` chart.
The unchanged project configuration is copied to `before.toml` and `after.toml`
with absolute project/output paths. Reproduce the after measurement with:

```sh
nix develop --command bash target/quality-refactor-20261111/lens.sh all after
nix develop --command python3 target/quality-refactor-20261111/compare.py
```

A fresh before run requires the baseline checkout. The launcher uses the existing local metadata wrapper to
omit non-library registry targets from metadata only and explicitly selects
LLVM 22 for this Rust 1.95 toolchain. No workspace sources or dependency edges are
excluded. Target evidence is local and ignored by Git; the table records the
results durably here.

---

# Post-parity quality refactor — 2026-11-05

Baseline: `b8fc358`, after the eight functional-parity stages. This review uses
fresh local RustQualityLens measurements over 94 authored Rust source modules,
including first-party tests/examples and the engine spike, excluding vendored
code. Earlier sections below describe older baselines and are retained as history.

| Measurement | Before | After |
| --- | ---: | ---: |
| Maximum cognitive complexity | 92 | 54 |
| Sum of cognitive complexity | 5,742 | 5,415 |
| Maximum cyclomatic complexity | 77 | 58 |
| Sum of cyclomatic complexity | 6,187 | 6,090 |
| Sum of function Halstead effort | 43,859,856 | 41,822,601 |
| Duplicated source lines | 2,461 | 2,284 |
| Sum of module locality risk ↓ | 345.75 | 342.75 |
| Mean observed module leverage ↑ | 25.32 | 25.32 |
| First-party physical Rust lines | 38,164 | 37,859 |
| Literal `.clone()` call sites | 448 | 425 |
| Observed panic paths, including tests | 478 | 468 |
| Wildcard imports | 27 | 26 |
| Test/helper function lines | 10,600 | 10,596 |
| Unsafe blocks | 6 | 1 |
| Deref / DerefMut implementations | 2 | 0 |
| Executed workspace tests | 211 | 206 |
| Lines coverage (%) | 82.93 | 83.49 |
| Regions coverage (%) | 82.29 | 82.91 |
| Functions coverage (%) | 84.62 | 85.17 |

The changes address the following findings:

- **Complex control flow:** SQL completion now separates lexical scopes, aliases,
  CTE projection and candidate selection. Filter parsing shares quote decoding
  and separates operator/value admission. Schema-comparison state renders its own
  controls and bounded differences; the application routes actions. The native
  smoke state machine separates its workbench scenario from the core journey.
- **Ownership and locality:** database state belongs to the session module;
  per-view tool state belongs to its document. The implicit `App` dereference
  facade is gone. Palette and cross-database search share object enumeration and
  fuzzy matching, borrowing metadata and allocating commands only when selected.
  These improve actual reuse, but the advisory mean module-leverage score remains
  unchanged. Aggregate locality risk falls; the shared `App` hub still exists.
- **Copies and escape hatches:** script outcomes move row/column buffers between
  their retained slot and the visible result instead of cloning payloads. The
  small completion record still clones. Linux advisory locking uses one checked,
  documented `fcntl` boundary, preserving explicit unlock and retrying interrupted
  calls; unsafe zero-initialization is gone. No lint suppression was added.
- **Dead paths:** remove the legacy single-pane renderer, editor toolbar, layout
  model and unused icons. Opened sessions always initialize the tiled workspace;
  chooser/opening/disconnected handling remains. Persisted inspector preferences
  remain readable for compatibility with existing configuration files.
- **Tests:** merge overlapping completion, layout/status and close/session
  scenarios; share fixtures, result access and bounded polling helpers. Retained
  journeys exercise the actual F7 comparison/capture/generated-migration flow and
  switch script outcomes repeatedly while checking retained rows, errors and
  sequential/atomic effects. Completion cases cover scope isolation, CTEs, quoting,
  Unicode and cursor limits. Lone double-quote/backtick cases first reproduced a
  slice panic, then passed after matching complete quote pairs. Existing lock,
  multiprocess, rollback, draft and stale-snapshot assertions remain.

Line coverage changes from 21,547/25,983 to
21,496/25,746. Removing unreachable code reduces the
coverage denominator as well as adding exercised paths; this is not a claim that
more absolute lines execute. Test/helper volume counts the union of function
spans in test modules, integration-test paths and catalogued tests, including
helpers but excluding imports/constants and inter-function blank lines. The
reduction in this stricter volume measure is small; five fewer test functions
execute. One manual profiling test remains ignored.

Validation passes: formatting, workspace/all-target Clippy with warnings denied,
206 workspace tests, workspace build, engine spike, native Wayland desktop smoke,
startup-discovery smoke and all four local-project schema/desktop smoke cases.
The spike's pre-existing fault-injected commit/rollback and old-engine fixture
checks remain unvalidated. No comparative runtime benchmark or mutation suite
was run; the reproduced parser regression supplies a focused failing-before /
passing-after check. Full-batch draft validation still clones staged values.

Measurement limits: coverage has complete line/region/function evidence but no
branch data. Syntax reports are marked partial because `desktop.rs` contains an
unexpanded `compile_error!` item. Semantic dependency identity resolves
1,493/1,520 collected references; architecture scores are advisory and this
resolution percentage is not proof of complete dependency inventory. No missing
measurement was treated as a passing quality gate.

Reproduction and evidence:

- Tool checkout: `Projects/rust-quality-lens`, commit
  `d137d8d8f219b4ee788935aff27115374626d2f8`; the frozen `rqlens` binary SHA-256 is
  `a9a7f6ac89b7f3a060d26a3b179b6fbed5f9338b8eb4815f93a5beecc0da53a3`.
- Both runs use the same Rust source roots, analyzer, compiler and scoring models.
  `rqlens.toml` now fingerprints UI assets and smoke scripts and allows 120 seconds
  for offline semantic identity. The preserved baseline configuration omitted
  `contracts` from extra fingerprint inputs; the final config restores it. No
  contracts changed, and measured Rust scope is identical.
- Rust 1.95.0, cargo-llvm-cov 0.8.5 and LLVM coverage tools 22.1.5 run in the Nix
  development environment. Provide `cargo-llvm-cov` on PATH and set `LLVM_COV` and
  `LLVM_PROFDATA` to the matching LLVM tools before running
  `target/quality-refactor-20261105/rqlens measure all --config rqlens.toml`.
- Ignored local artifacts in `target/quality-refactor-20261105/`: `before/`,
  `after/`, source inventories, baseline/final configs, `compare.py`,
  `comparison.json`, the frozen tool, environment and validation logs.
  `python3 target/quality-refactor-20261105/compare.py` recomputes the table inputs.

---

# Five follow-up improvements — 2026-11-05

Baseline: `14ee484`, the completed quality review below. Each improvement is
validated and committed separately. Evidence is under `target/quality-five/`.

1. **Edit-review presentation ownership.** Insert fields, insert submission,
   review-window actions, operation selection and cached diff/SQL/bindings
   rendering have focused owners. The presentation model renders its own cached
   review; SQL and parameter text share a read-only renderer. Selected statements
   and edits use checked paired access. The existing edit/commit interaction now
   clicks Remove operation in the real review window and verifies retained engine
   values before re-staging. `edit_windows` cognitive/cyclomatic complexity falls
   from 84/26 to 2/3; the largest extracted handler is 12/4. All 128 active UI
   tests, formatting and workspace/all-target Clippy pass.

2. **Bounded result delivery.** A dedicated buffer owns cell deferral, retained
   row/byte limits, chunking and tail-window eviction. SQL execution retains
   validation, engine reads and write draining. Tail entries retain their byte
   sizes, avoiding repeated size scans and the eviction `unwrap`. Partial chunks
   still flush on engine errors, and page completion is recorded only after
   successful row traversal. The existing real-engine budget scenario now checks
   exact byte boundaries, page/non-page outcomes and byte-evicted tail rows.
   `execute` cognitive/cyclomatic complexity falls from 75/55 to 25/32. Database
   tests (62), UI tests (128), formatting and all-target Clippy pass.

3. **Focused worker dispatch.** Prepared queries have named connection, SQL,
   binding and budget fields. Exhaustive public-operation routing delegates
   browse/reference planning, imports, exports, checkpoints and staged commits
   to focused handlers. Row, count and peek relationship reads share metadata
   admission through a private mode enum; invalidation events share request
   stamping. Terminal delivery and persistent-editor transaction reporting stay
   centralized. Dispatcher cognitive/cyclomatic complexity falls from 34/72 to
   4/45; relationship planning is 9/13. All database/UI tests, formatting and
   all-target Clippy pass. The public operation protocol is unchanged.

4. **Borrowed staged values and lazy previews.** Overlay caches retain edit/change
   locations and borrow the draft's value during painting, rather than owning
   another copy of each changed value. Cached SQL and bindings are generated only
   when the review UI requests them. Typed insert preparation borrows retained
   values directly for its field editors and JSON display, removing serialization
   followed by parsing, cloned parsed cells and a silent parse fallback. Existing
   interaction coverage now stages the actual typed clone and checks exact values;
   it also checks that an overlay value has the draft value's identity. The manual
   cache profile now measures the actual value lookup. All 128 active UI tests,
   formatting and all-target Clippy pass. No runtime speedup is claimed without
   comparative profiling; full-batch staging validation still clones drafts.

5. **Bounded history presentation and shared query actions.** A borrowed history
   row owns display flags and rendering; it has no dependency on persisted history
   types. History and saved queries share load/delete controls. Empty searches
   skip SQL case conversion. SQL previews stream at most 24 words and 120 Unicode
   characters, avoiding a large intermediate vector/join for a long first word.
   The two query-library scenarios share one fixture and retain their distinct
   history, saved-query and theme assertions. Added cases cover stable equal-time
   sorting, unknown durations, Unicode search, legacy/failed badges and long
   Unicode previews. `query_history_ui` cognitive/cyclomatic complexity falls
   from 50/22 to 14/17; one row is 11/4. All 127 active UI tests and Clippy pass.

Both full measurements use the same local RQLens binary, configuration, metadata
launcher and Nix shell. The preserved baseline artifacts were collected just
before committing `14ee484`; the final artifacts precede the fifth commit. Both
inventories contain 81 source modules.

| Measurement | Series baseline | After five improvements |
| --- | ---: | ---: |
| Maximum cognitive complexity | 84 | 58 |
| Sum of cognitive complexity | 5,168 | 5,081 |
| Maximum cyclomatic complexity | 72 | 60 |
| Sum of cyclomatic complexity | 5,513 | 5,555 |
| Sum of function Halstead effort | 38,468,150 | 37,960,351 |
| Duplicated source lines | 2,158 | 2,137 |
| Observed panic paths, including tests | 434 | 436 |
| Wildcard imports | 21 | 21 |
| Sum of module locality risk | 279.75 | 279.75 |
| Mean observed module leverage | 25.93 | 25.93 |
| Physical first-party Rust lines | 34,054 | 34,526 |
| Literal `.clone()` call sites | 379 | 377 |
| Test/helper function lines | 9,752 | 9,889 |
| Executed tests | 195 | 194 |
| Line coverage (%) | 83.30 | 83.59 |
| Region coverage (%) | 82.85 | 83.15 |
| Function coverage (%) | 85.04 | 85.39 |

Peak complexity, total cognitive complexity, effort, duplication and line coverage
improve. The global cognitive maximum now belongs to grid orchestration (58);
the cyclomatic maximum is the vector-icon painter (60). Runtime algorithms avoid
redundant value copies, preview generation and repeated tail-size scans; speedups
were not benchmarked.

The tradeoffs are explicit: this series adds 472 physical Rust lines and 137
lines of test/helper function spans, while executing one fewer test. More focused
helpers and regression assertions increase summed cyclomatic complexity by 42.
Mean module leverage and aggregate locality risk remain unchanged. Three new
unwrap findings are in integration assertions, while the worker eviction unwrap
is removed; no production panic path or lint suppression was added. This series
is not a further overall LOC/test-code reduction.

Coverage rises from 19,130/22,965 to 19,431/23,245 lines. Test/helper volume uses
the same union of function spans as the preceding review, including helpers but
excluding imports/constants and inter-function blank lines. One manual profiling
test stays ignored. Branch/mutation coverage and comparative runtime profiling
were not collected. Architecture identity remains partial at 1,375/1,401 resolved
references (baseline 1,370/1,396); its locality/leverage scores remain advisory.

Final checks pass: formatting, workspace/all-target Clippy with warnings denied,
194 workspace tests, workspace build, engine spike, native Wayland desktop smoke,
startup-discovery smoke and all four local-project schema/desktop smoke cases.
The engine spike's existing fault-injection and old-engine compatibility gates
remain unvalidated. Full-draft cloning during staging validation and the shared
`App` dependency hub remain worthwhile follow-ups.

Evidence is in `target/quality-five/`: `baseline/`, `final/`, `comparison.json`,
`compare.py`, source inventories, per-improvement hotspot/test/Clippy logs, build,
engine and native smoke logs. These are ignored build artifacts. Reproduce the
measurements with `nix develop -c bash target/quality-five/lens.sh all` using the
same local metadata launcher and coverage environment described below.


---

# Quality review — 2026-11-05

Reviewed the code at `0b7c4347c1a8f41320daccc5e372b5c2d25b5dc8` and refactored
against fresh measurements from the local `Projects/rust-quality-lens` checkout.
Both runs use the same binary, `rqlens.toml`, metadata launcher and Nix shell.
They cover 81 authored modules; vendored dependencies are excluded. Older reviews
below cover earlier, smaller versions of the project and are not this baseline.

| Measurement | Before | After |
| --- | ---: | ---: |
| Maximum cognitive complexity | 96 | 84 |
| Total cognitive complexity | 5,228 | 5,168 |
| Maximum cyclomatic complexity | 72 | 72 |
| Total cyclomatic complexity | 5,542 | 5,513 |
| Total function Halstead effort | 39,021,252 | 38,468,150 |
| Duplicated source lines | 2,212 | 2,158 |
| Wildcard imports | 23 | 21 |
| Observed panic paths, including tests | 446 | 434 |
| Sum of module locality risk ↓ | 283.5 | 279.75 |
| Mean module leverage ↑ | 25.93 | 25.93 |
| Measured source modules | 81 | 81 |
| First-party physical Rust lines | 34,061 | 34,054 |
| Literal `.clone()` call sites | 383 | 379 |
| Test and helper function lines | 9,846 | 9,752 |
| Executed workspace tests | 197 | 195 |
| Line coverage (%) | 83.13 | 83.3 |
| Region coverage (%) | 82.49 | 82.85 |
| Function coverage (%) | 84.67 | 85.04 |

The main findings and resulting changes are:

- **Repeated request policy.** Commands, paging, filters, sorting, restored views,
  external refresh and transfers repeated the same ready/closing/draft/commit
  checks. They now share `App::requests_enabled`; operation-specific checks stay
  with each caller. Five duplicate palette commands use ribbon action dispatch.
  Transaction operation construction belongs to the SQL executor. Shared grid,
  column and ribbon alignment constants belong to presentation, removing
  dependencies on the grid renderer. Locality risk fell; measured mean leverage
  stayed flat, despite the additional reuse of policy and test support.
- **Mixed UI responsibilities.** Ribbon action rendering and fixed window
  controls have separate handlers. Discovery folder lists share row rendering;
  settings sections and summaries have focused owners. The editor separates its
  toolbar, bindings and buffer rendering. Transfer preview rendering belongs to
  transfer state. Cognitive complexity fell from 82 to 31 in `pane_ribbon`,
  from 61 to 24 in `discovery_settings_ui`, and from 96 to 45 in `transfer_ui`.
- **Redundant work and escape paths.** Table selection borrows candidate names
  and only owns the chosen navigation target. Built-in SQL completions borrow
  static strings. Reverse-reference validation retains checked column metadata,
  avoiding repeated searches and three `unwrap` paths. Storage shutdown owns its
  sender while draining and no longer unwraps it. Bundled SVG icons dispatch once
  through their geometry renderer, removing an `unreachable!` branch. Removed
  the redundant selection-execution forwarding method and two test glob imports.
- **Overlapping tests and repeated drivers.** Folder persistence and keyboard/
  pointer behavior share one scenario. Row-slider and hidden-field navigation
  share one real-database scenario, retaining draft, ticket and selection checks.
  UI fixtures, key events, accessibility traversal, header-inspection waits and
  exact retained-value assertions share support. Existing scenarios now also
  exercise invalid completion offsets/large buffers, actual NULL-binding
  generation, positional-binding guidance and discovery summary/error rendering.

The project-size reduction is small: seven physical Rust lines and 19 nonblank
source lines. Extracting readable transfer-preview code offsets much of the
removed duplication. Test/helper function spans shrink by 94 lines while two
fewer active tests execute. That volume measure unions function line ranges in
RQLens test/support modules and test files, including shared helpers; it excludes
module imports, constants and blank lines between functions. `.clone()` counts
are literal call-site inventory, not allocation measurements. Runtime allocation
or speed improvements were not benchmarked.

Coverage increases from 19,067/22,936 to 19,130/22,965 lines. The same all-target
coverage command runs both times; one manual profiling test remains ignored.
RQLens inventories 198/196 tests, including the ignored test; ordinary workspace
runs execute 197/195. Branch and mutation coverage were not collected.

Remaining priorities are the edit-review window orchestration (cognitive 84),
worker execution (75), and the operation dispatcher (cyclomatic 72). The dispatcher
still determines the global cyclomatic maximum. The shared `App` remains a
large dependency hub. Architecture identity remains partial: 1,302/1,328
references resolved before and 1,370/1,396 after. Locality and leverage are
heuristics, not verified isolation guarantees. Syntax/reliability inventories
remain partial around unexpanded macros, including the existing desktop
`compile_error!`; panic totals include test assertions/helpers. No lint
suppressions or unsafe code were added.

Validation passed: formatting, workspace/all-target Clippy with warnings denied,
195 workspace tests, workspace build, engine spike, the 100,000-row Wayland
desktop smoke, startup-discovery smoke and all four local-project schema/desktop
smokes. The engine spike still identifies fault-injected commit/rollback failures
and old-engine compatibility fixtures as unvalidated existing gates.

Reproduction and evidence are retained in ignored build output under
`target/quality-review-current/`: `lens.sh`, `baseline/`, `final/`,
`baseline-source.json`, `final-source.json`, `comparison.json`, test/Clippy/build
logs and individual smoke logs. Run the launcher within `nix develop`; it uses
this machine's pre-existing Cargo-metadata workaround documented below.

---

# Incremental quality improvements — 2026-11-03–04

The initial review/refactor was committed as `0051473`. The next priorities,
ranked using RQLens hotspots and inspection of allocation/ownership costs, are:
status-bar policy, record-field presentation, grid-cell painting, selection
statistics, and contextual relationship reads. Icons' large match is mostly
vector geometry; splitting those drawing constants would have less value than
simplifying stateful code and repeated work on UI paths.

1. **Status bar and shared command policy.** Connection, transaction and result
   status rendering have focused handlers. Transaction buttons and the tile
   ribbon now use the same enablement and dispatch rules. Removed the obsolete
   pre-workspace delivery control: execution starts after workspace initialization,
   and the active ribbon already owns discard. The existing read-only transaction
   scenario now clicks the actual rollback control instead of calling SQL directly.
   UI tests (111), workspace/all-target Clippy, formatting and RQLens hotspots pass.
   `status_bar` cognitive/cyclomatic complexity fell from 73/30 to 10/5.

2. **Borrowed record-field presentation.** Separate field labels, values,
   parent navigation and inline submissions. Checked column metadata now drives
   key badges and relationship colors in the record inspector as well as the
   grid, so unrelated query/schema metadata cannot decorate a field. Tooltip
   detail/JSON formatting happens only on hover. Double-click starts an editor
   without cloning the displayed value; typed actions distinguish starting an
   edit from submitting one. Removed a redundant reference-list forwarding method.
   The 111 UI tests, all-target Clippy, formatting and hotspot measurement pass.
   Record orchestration cognitive/cyclomatic complexity fell from 72/30 to
   8/8.

3. **Grid-cell appearance and deferred text layout.** A borrowed presenter owns
   accessibility text, clipping, alignment, status styling and staged/deleted/FK
   markings. The grid method retains editing and action routing. Plain-text
   formatting/font layout is skipped when a status pill already paints the cell,
   and inline editing no longer prepares an unused painted text value. The existing
   column scenario renders a styled cell and checks retained engine data. All 111
   UI tests, all-target Clippy, formatting and hotspots pass. `grid_cell`
   cognitive/cyclomatic complexity fell from 57/51 to 37/34; paint is 17/13.

4. **Streaming selection and statistics.** Selection bounds borrow the column
   order and preserve hidden-column, ticket and row-window rules. Grid membership,
   clipboard traversal and aggregate traversal no longer allocate coordinate lists;
   statistics no longer allocate value, integer, real and combined numeric vectors.
   Cached summaries return a borrowed string and compare visible columns through
   an iterator. Removed two extrema `unwrap` paths. A single numeric matrix covers
   empty/non-numeric selections, integer limits and precision beyond 2^53, mixed
   values, NaN, infinity and floating overflow. Integer sums/extrema remain exact;
   mixed-real sums remain explicitly approximate and now accumulate in selection
   order. All 112 UI tests, all-target Clippy, formatting and hotspots pass.

5. **Lazy relationship requests and bounded cache work.** A request plan borrows
   key values and metadata until session admission and cache checks pass. Rendering
   and cache lookup no longer build unused worker operations or clone key values.
   Hover processing moves its retained key when a request is due instead of cloning
   it on every frame. Cached entries calculate the existing retained-size estimate
   once; FIFO eviction and retry removal update a running byte total. Cache lookups
   share one helper. A cache matrix covers count eviction, byte eviction, retry
   removal, absent keys and an oversized error entry. The real worker scenario
   verifies a cache hit does not submit another request.

| Measurement | After initial refactor | After five improvements |
| --- | ---: | ---: |
| Maximum function cognitive complexity | 73 | 62 |
| Sum of function cognitive complexity | 3,879 | 3,812 |
| Maximum function cyclomatic complexity | 65 | 65 |
| Sum of function cyclomatic complexity | 4,028 | 4,054 |
| Sum of function Halstead effort | 28,365,131 | 27,892,242 |
| Duplicated source lines | 1,480 | 1,462 |
| Wildcard imports | 17 | 17 |
| Observed panic paths, including tests | 258 | 258 |
| Sum of module locality risk (lower is better) | 186.00 | 188.25 |
| Mean observed module leverage (higher is better) | 24.70 | 25.00 |
| Measured modules | 66 | 66 |
| Physical first-party Rust lines, including tests | 25,612 | 25,868 |
| UI test-module and interaction-suite lines, including helpers | 5,558 | 5,647 |
| Executed tests | 139 | 141 |
| Line coverage (%) | 83.93 | 84.11 |
| Region coverage (%) | 83.63 | 83.86 |
| Function coverage (%) | 85.81 | 86.08 |

The before column measures the code committed as `0051473`; its preserved baseline
artifacts were collected just before that initial commit. The after artifacts
were collected before committing improvement 5. The source scope, RQLens binary,
configuration and toolchains are unchanged; both inventories contain 66 modules.
The largest remaining cognitive hotspots are `pane_ribbon` (62),
`discovery_settings_ui` (59) and `editor_ui` (53). The maximum cyclomatic value
still belongs to the vector-icon match, which was left intact.

The clearest gains are focused rendering ownership, shared transaction policy,
lazy formatting and reduced allocation/cloning on UI paths. Runtime speedups were
not benchmarked. Aggregate cyclomatic complexity, total source size and locality
risk increased; these are recorded tradeoffs, not claimed global improvements.
Mean reuse/leverage improved slightly. Architecture identity remains partial
(1,001/1,018 dependency references resolved). Reliability findings include tests
and remain partial around unexpanded macros. Its observed panic-path inventory
is unchanged: the removed extrema unwraps were not reported in the baseline,
so the count does not reflect that source-level change. Branch and mutation
coverage were not collected.

The series adds two focused test matrices. UI test code is 5,647 lines,
compared with 5,558 after the initial refactor and 5,757 before
that refactor. The current series is not a further test-volume or LOC reduction.

Final validation passed: formatting, workspace/all-target Clippy with warnings
denied, 141 workspace tests, workspace build, the engine spike, the desktop,
startup-discovery and four local-project smoke cases, and native capture of all
workspace sizes/themes, navigator tools, staged edits, dialogs and chooser states.
The engine spike retains its existing unvalidated fault-injection and old-engine
compatibility gates.

Evidence is retained under `target/quality-next/`: `baseline/`, `final/`,
`comparison.json`, per-improvement hotspot/test/Clippy logs, final measurement
and smoke logs, and `captures/`. `lens.sh` records the same local metadata launcher
and coverage-tool setup used for the initial review. The evidence directory is
ignored build output. All five improvements are committed separately.

---

# Quality review — 2026-11-03

Follow-up review and refactor using the local `Projects/rust-quality-lens`
checkout, Rust 1.95.0, and fresh `rqlens measure all` runs. The Rust baseline is
`9a89c23a60676fb972faaac1b3308a06f4b6d880`. Concurrent documentation-only commits (`5046e1e`, `c8fd339`)
landed during review; their README and native image changes were preserved. An
interrupted analysis correctly refused publication and was rerun at the stable
revision. Both successful runs use the same tool binary and `rqlens.toml` scope;
vendored sources are excluded.

| Measurement | Before | After |
| --- | ---: | ---: |
| Maximum function cognitive complexity | 90 | 73 |
| Sum of function cognitive complexity | 3,966 | 3,879 |
| Maximum function cyclomatic complexity | 76 | 65 |
| Sum of function cyclomatic complexity | 4,012 | 4,028 |
| Sum of function Halstead effort | 30,066,830 | 28,365,131 |
| Duplicated source lines | 1,728 | 1,480 |
| Wildcard imports (observed escape hatches) | 27 | 17 |
| Observed panic paths, including tests | 281 | 258 |
| Sum of module locality risk (lower is better) | 188.25 | 186.00 |
| Mean observed module leverage (higher is better) | 24.55 | 24.70 |
| Measured modules | 66 | 66 |
| RQLens nonblank source lines | 25,048 | 25,036 |
| Physical first-party Rust lines, including tests | 25,618 | 25,612 |
| UI test-module and interaction-suite lines, including helpers | 5,757 | 5,558 |
| Executed tests | 141 | 139 |
| Line coverage (%) | 83.36 | 83.93 |
| Region coverage (%) | 82.82 | 83.63 |
| Function coverage (%) | 85.58 | 85.81 |

The principal changes are:

- Diagram presentation owns its camera and borrows filtered table metadata.
  Filtering no longer clones a second schema and its inspections. The shell
  routes selected-table actions; the event handler owns the database request.
  Node drawing and camera transitions have focused methods.
- Removed the obsolete diagram overlay toolbar. `opened()` initializes the
  tiled workspace before requesting diagram metadata, and a new open clears
  both workspace and diagram state. The old overlay required metadata while
  the tiled workspace was disabled. All active map controls remain in the tile
  ribbon and retain interaction coverage.
- Column descriptors share checked browse metadata with column measurement and
  the record inspector. Column painting, layout/menu operations, and grid
  context actions have separate owners. Foreign-key matches borrow metadata;
  navigation and the retained hover request copy only the chosen key. Removed
  the impossible zero-row branch under the nonempty-result guard.
- Pending context reads move out and back during delivery instead of using a
  completion-path `unwrap`. Ten test wildcard imports were made explicit.
  Screenshot layout stages are separated from the database smoke protocol.
- The existing App test module now provides shared egui frame, pointer and
  accessibility-control helpers. Close waits and staged-value assertions share
  support. Two overlapping tests were consolidated: range/precision checks now
  live with actual keyboard editing, and icon accessibility checks share one
  rendered fixture. Assertions for distinct behaviors were retained.

The existing column scenario now clicks all three sort states and column fit,
move and hide controls, and exercises status, priority, progress, null and
nonfinite presentation. Diagram layout checks now include self references,
missing targets, system-table filtering and identity of borrowed metadata.
Line coverage rose from 14,934/17,916 to
15,063/17,948, while two fewer tests execute.
UI test volume counts physical lines in the UI's inline test modules, the
interaction suite and their shared driver; database integration tests were kept.

Peak complexity improved in the targeted responsibilities: diagram orchestration
90/48 cognitive/cyclomatic became 28/22 in `DiagramState::show`, grid-header
orchestration 86/37 became 11/12 (one column is 11/13), and grid cells 81/62 became
57/51. Aggregate cyclomatic complexity rose slightly, despite the lower maximum,
as smaller helpers and additional exercised behaviors add independent paths.
Total Rust LOC is essentially flat (-6 lines);
this is not a large project-size reduction.

Architecture signals improved modestly with the same 66 source modules:
diagram locality risk fell from 3 to 0, aggregate locality risk fell, and mean
observed leverage rose. These remain heuristics with partial semantic identity:
907/925 references resolved before and 987/1,004 after. They are not a proof of
complete dependency isolation. Panic-path totals are observed source findings,
mostly in tests; reliability evidence remains partial around unexpanded macros.
No suppressions or unsafe code were added. Branch and mutation coverage were
not collected. The remaining largest cognitive hotspots are `status_bar` (73)
and `record_inspector` (72); the shared `App` state remains a dependency hub.

Validation: formatting, workspace/all-target Clippy with warnings denied,
139 workspace tests, workspace build, engine spike, the 100,000-row desktop
smoke, startup discovery smoke, and all four local-project desktop/schema smoke
cases passed. The engine spike still reports its existing unvalidated
fault-injection and old-engine compatibility gates. Native screenshot capture also passed: six workspace sizes/themes, navigator
tools, staged edits, dialogs, chooser and search settings. The captures are
retained under `target/review-2026-11-03/captures/`.

Local evidence is in `target/review-2026-11-03/baseline/`,
`target/review-2026-11-03/final/`, `target/review-2026-11-03/comparison.json`,
and the logs in that directory. These are ignored build artifacts. Reproduce
with the commands documented in the previous review below, using the same
Cargo metadata launcher and local coverage-tool environment. The launcher
workaround remains necessary for this RQLens checkout's extractor environment
size limit.

---

## Previous review — 2026-11-02

Reviewed and refactored against `c88e266811f58955df2792a736997f4771520d52`
using the local `Projects/rust-quality-lens` checkout, Rust 1.95.0, and
`rqlens measure all`. Configuration is in [rqlens.toml](../rqlens.toml).
Vendored dependencies are excluded from the authored-source measurements.

| Measurement | Before | After |
| --- | ---: | ---: |
| Maximum function cognitive complexity | 178 | 127 |
| Sum of function cognitive complexity | 2,148 | 1,985 |
| Maximum function cyclomatic complexity | 79 | 59 |
| Sum of function cyclomatic complexity | 2,049 | 2,047 |
| Sum of function Halstead effort | 15,051,662 | 13,301,600 |
| Duplicated source lines | 857 (7.15%) | 604 (5.05%) |
| Wildcard imports | 40 | 18 |
| Authored panic-path findings, including tests | 244 | 157 |
| RQLens nonblank source lines | 11,987 | 11,963 |
| Physical first-party Rust lines, including tests | 12,197 | 12,201 |
| Integration/harness and UI interaction test lines | 2,327 | 2,113 |
| Executed tests | 78 | 77 |
| Line coverage | 76.95% (6,306/8,195) | 77.76% (6,454/8,300) |
| Region coverage | 75.43% | 76.42% |
| Function coverage | 79.36% | 79.63% |

The test-line comparison covers `crates/tursobench-db/tests/**/*.rs` and
`crates/tursobench-ui/src/app/interaction_tests.rs`, including shared fixtures.
It does not claim every inline unit-test module shrank. Physical Rust LOC is
nearly unchanged: smaller harnesses and dispatch offset explicit imports,
readable rendering code, and additional boundary cases. Function effort is
summed over function records only; file-level effort is not added again.

The main changes are:

- The session facade delegates engine ownership, dispatch, execution, delivery
  budgets and cleanup to `tursobench-db/src/worker.rs`. Queries share completion
  handling and consistently report the persistent editor's transaction state.
  Runtime construction failures now return an error instead of panicking on
  the worker. The existing public operation variants and validation entrypoint
  remain available.
- Discovery separates remembered-file checks, directory admission, entry
  processing and traversal. Mount and symlink checks still precede filesystem
  probes. Queuing a child no longer clones the parent's path.
- Shell panels, result headers and result rows have separate rendering methods.
  Grid headers borrow column descriptors. Inspection, bookmarks and history
  borrow retained data; only selected navigation data is copied. Typed cell
  staging no longer serializes and reparses an unused JSON field. Redundant
  grid, inspector and JSONL forwarding paths were removed.
- Preference retention shares one count/byte-bounded prefix operation, replacing
  repeated full-vector sums and pops. Testing support shares real database
  construction, protocol checks and file snapshots across suites.
- Five database integration binaries became one suite with separate case
  modules and a shared client. Export tests exercise every format through one
  matrix while retaining the separate real-Turso SQL restoration test.
  The local-project smoke script uses the new suite selector.

The added cases exercise zero byte/cell budgets, wrong session identities,
SQL and parameter size limits, oversized column metadata, missing/empty/corrupt
files and directories, invalid timeouts and missing exclusive-use consent,
malformed/oversized edit batches, and failed-export cleanup. Existing tests
continue to exercise transaction isolation, read-only file preservation,
foreign keys, optimistic conflicts, backpressure, close cleanup and UI input.

The largest touched-module function maxima changed as follows:

| Responsibility | Cognitive before → after | Cyclomatic before → after |
| --- | ---: | ---: |
| Shell panels | 178 → 70 | 79 → 26 |
| Result grid | 152 → 54 | 51 → 23 |
| Discovery | 82 → 19 | 57 → 19 |
| Database facade and worker combined | 35 → 20 | 36 → 23 |

Architecture evidence needs a qualified reading. The database facade's locality
risk fell from 3.0 to 1.5 after worker extraction. Mean observed module leverage
rose from 12.43 to 19.02, but the module inventory changed from 37 to 41 and
explicit imports exposed many more dependency references. Global locality risk
rose from 15 to 36; there is no verified whole-project locality improvement.
The core types and `App` remain shared dependency hubs. Semantic resolution is
partial in both runs: 127/130 references before and 345/357 after. These are
architecture heuristics, not correctness gates or causal proof of improvement.

The remaining priorities are `diagram_ui` (cognitive 127, cyclomatic 59),
`database_chooser_ui` (82/36), the shared `App` state, and the remaining panic
paths. All 18 remaining wildcard imports are confined to test code. The unsafe
FIFO fixture remains test-only. No lint suppressions were introduced. Branch
coverage and mutation evidence were not collected, so line coverage does not
establish exhaustive decision or fault protection. RQLens's syntax catalog
also omits some module prefixes in suggested exact-test selectors; use Cargo's
actual suite names, for example `cargo test -p tursobench-db --test suite
session::`, when selecting tests.

Validation passed: formatting, workspace/all-target Clippy with warnings denied,
77 workspace tests, the engine spike, a desktop smoke run with 100,000 disposable
rows, startup discovery with isolated preferences, and all four local-project
schema/desktop smoke cases. The engine spike's previously documented unvalidated
fault-injection and old-engine compatibility gates remain open.

Local evidence is retained in `target/review/baseline/`, `target/analysis/`,
`target/review/comparison.json`, and the logs under `target/review/`. These are
ignored build artifacts, not checked-in evidence. Rerun with the same scope and
optional coverage tools available:

```sh
rqlens measure all --config rqlens.toml
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked -p engine-spike
cargo build --locked -p tursobench-ui
bash scripts/desktop-smoke.sh
bash scripts/discovery-smoke.sh
bash scripts/local-project-smoke.sh
```

This local RQLens build initially exceeded Linux's per-environment-variable
limit by passing registry dependency test/example target paths to its extractor.
Both measurements used the same temporary Cargo metadata launcher, retained at
`target/review/launcher/cargo`. It removes registry-only non-library target
entries while preserving workspace targets, dependency packages, and the resolve
graph; normal Cargo commands pass through unchanged. The original tool checkout
was not modified. For reproduction on this machine, prepend that launcher's
folder to `PATH` and expose `cargo-llvm-cov`, `LLVM_COV` and `LLVM_PROFDATA` from
the local RQLens development shell before invoking its built binary. The saved
launcher contains this machine's Cargo path and should be adapted elsewhere.
