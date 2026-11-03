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
