# Review remediation

Implementation follows the eight stages agreed on 2026-11-04. Each stage is
committed independently after its targeted checks. This document records actual
delivery and remaining work; it is not a production-readiness claim.

> Subsequent concurrency change: [shared access](shared-access.md) supersedes the
> exclusive-use acknowledgement and initial multiprocess restrictions recorded
> below. Current admission uses Linux OFD locks and verified native coordination.

> Subsequent storage change: preferences now use an application-owned local Turso
> database with no JSON/legacy migration. The original JSON recovery work below is
> historical; current behavior is documented in [data formats](data-formats.md).

## Stage 1 — correctness and recovery

- Replace the unsupported FK PRAGMA with parent-affinity/collation anti-joins,
  reporting violation counts and checked-table coverage. Metadata failures are
  errors, not clean results. Diagnostic scope is the committed browse connection.
- Give catalog discovery a separate 10,000-object / 8 MiB budget and 1 MiB cell
  budget, prioritize tables/views, and display incomplete delivery. Lazy DDL
  transport is part of the large-value work in stage 5.
- Use rendered row stride, including spacing, for eviction and keyboard scrolling.
- Parse table storage options and reject generated edits without proven rowid
  support, including WITHOUT ROWID, at the shared backend boundary.
- Recover valid preference fields and collection entries, preserve the original
  when recovering, retain a previous-file backup on save, and disable automatic
  persistence for unreadable or newer-version files.
- Flush successfully decoded partial rows when a later engine error occurs.

## Delivery sequence

2. File access safety and result freshness.
3. Execution lifecycle, scheduling and verified cancellation/deadlines.
4. Indexed browsing, navigation, typed/grouped filters and reference paging.
5. Deferred large cells, bounded value viewing and lazy DDL.
6. SQL selection/scripts/parameters and typed/bulk editing.
7. Streaming import/export and explicit maintenance.
8. Profiling, fault testing and release qualification.

Stage 1 verification: 147 workspace tests pass, including real orphan and
1,001-index/76 KiB-DDL regressions, recovery and row-stride cases. Workspace
all-target Clippy passes with warnings denied; formatting and diff checks pass.

## Stage 2 — access and freshness

Writable opening now performs read-only schema preflight. The access panel
explicitly discloses that writable opening can migrate journal mode to WAL;
Turso 0.8.3 does not expose a qualified preserve-journal opening mode. This is
an acknowledged writable-open effect, not a no-migration guarantee. Both modes
state that exclusive use is required and read-only access cannot exclude an
unrelated writer. Read-only execution rejects ATTACH and VACUUM (including INTO).

Committed SQL emits a session invalidation event; manual-transaction changes
wait until commit, while full rollback clears pending invalidation. Other tiles
keep their retained rows with a visible stale marker; paging and new staging
require refresh. Schema mutations refresh the catalog and invalidate inspection
metadata. Staged commits invalidate other tiles without discarding their drafts.

Stage 2 verification: all 149 workspace tests, formatting, and all-target Clippy
with warnings denied pass. New regressions cover no-file-output read-only SQL,
manual rollback, committed cross-tile invalidation and automatic DDL discovery.

## Stage 3 — execution lifecycle

A pinned local Rust-wrapper patch exposes existing SDK interruption and statement
wall-clock deadlines. Default deadline is 30 s (0 disables, maximum 1 hour), applied
on reopen to all managed connections. Discard/close interrupt SELECT/EXPLAIN reads;
budget-limited reads stop stepping. Writes/RETURNING still finish when delivery is
suppressed. Interrupt/deadline errors preserve observed transaction state.

The FIFO scheduler accepts 32 requests with a separate 16 MiB aggregate payload
budget. Superseded reads skip execution, catalog refreshes coalesce while pending,
and the UI distinguishes queued/running work. This preserves editor ordering and
provides room for normal multi-tile metadata without an unbounded queue.

Real-engine regressions cover CPU aggregation before the first row, manual
transaction rollback after timeout, atomic interrupted bulk insertion, connection
reuse and 15 queued reads after cancellation. All 151 workspace tests pass.
Formatting and workspace all-target Clippy with warnings denied also pass. The
engine patch provenance, source archive SHA-256 and license are in vendor/turso.

## Stage 4 — browsing and navigation

Browse ordering uses selected sort columns plus proven primary-key/rowid ties,
without the all-column sorter for ordinary tables. Inline INTEGER PRIMARY KEY
rowid aliases are recognized structurally (DESC is excluded). Non-null PKs use
forward/backward keyset paging; nullable keys and views retain an OFFSET fallback.
The silent million-row clamp is removed. Shift-click adds secondary sort columns.

The browser adds row jumps, explicit filter-aware counts and bounded grouped
AND/OR expressions with typed IN lists. Bare numeric literals remain numeric on
untyped/view columns; quote values to compare as text. Incoming-reference full
views now use normal paging and counts; bounded previews probe one extra row and
report Limited. Scalar references use an index-friendly IN query only when parent
and child affinity match and parent collation is BINARY; other cases retain the
parent-governed comparison semantics.

Stage 4 verification: all 154 workspace tests, formatting and all-target Clippy
pass. New cases cover forward/backward seek, NULL/collated multi-column ordering,
251-row reference traversal, truncation status, grouped filters and matching
counts. Real EXPLAIN plans confirm indexed first-page/sorted browsing needs no
temporary B-tree. Concurrent changes invalidate cursors rather than promising a
snapshot across page requests.

## Stage 5 — large values

Oversized text/blob cells now carry a typed deferred handle; they cannot become SQL
bindings, staged originals or silently exported placeholder text. The worker backs
values from the original execution in private temporary files: at most 64 MiB per
value, 256 MiB / 1,024 files per session with FIFO expiration. Values outside these
limits remain visibly deferred/unavailable. Close drops the backing before its
acknowledgement. This bounds application backing, not the engine's incoming row.

The selected-cell viewer reads bounded 16 KiB text/hex chunks, searches the current
chunk, and atomically saves exact original bytes to a new file. It never reruns SQL.
Large DDL is excluded from catalog payloads and can be loaded separately into a
result/large-value viewer. A browse page has an explicit continuation flag when its
byte budget ends before 100 rows; forward/reverse keyed paging uses actual page
length. Deferred rows require complete originals before staging edits.

Stage 5 verification: all 157 workspace tests, formatting and all-target Clippy
pass. New tests preserve a 100 KB cell at row 50, read/save its original bytes,
refuse overwrite, continue 50 KB-wide rows, and reverse a short byte-limited page.

## Stage 6 — SQL and editing

Run, selection and EXPLAIN share cursor/selection semantics. Lexer boundaries
preserve comments, strings and trigger/CASE bodies while allowing a valid statement
past an unrelated malformed one. Run-all prevalidates up to 100 statements/1 MiB,
then executes sequentially with stop-on-error or in an explicit application-owned
atomic transaction. Atomic mode rejects embedded transaction controls and an active
editor transaction. Named script parameters are scoped per statement. Per-step
results/outcomes are retained within an aggregate 8 MiB budget; the selected result
can use another bounded 8 MiB display copy. Commit/rollback uncertainty is reported
as UNKNOWN, never successful rollback.

Bindings accept ordinary positional/named JSON alongside tagged values, rejecting
out-of-range integers and ambiguous nested arrays. Inserts have a typed form with
separate Default/Omit, NULL and Value choices plus optional tagged JSON. A grid
selection can stage unique-row deletions through the existing atomic review/commit
path. Original positive/negative infinity values can be matched for updates and
deletes; newly supplied non-finite values remain rejected.

Stage 6 verification: all 162 workspace tests, formatting and all-target Clippy
pass, including sequential partial success versus atomic rollback, trigger/script
boundaries, convenient bindings, staged bulk deletion and infinity row matching.

## Stage 7 — transfer and maintenance

Complete-table and execute-once query JSONL exports stream original engine values
without display retention limits. Version 2 headers permit an unknown initial row
count; exact end records and atomic no-overwrite publication remain mandatory.
Write execution finishes even after an output failure, and its actual state is
reported separately from publication. No retained result silently reruns SQL.

Typed JSON/JSONL and CSV/TSV imports provide bounded previews, explicit column/type
mapping, validation, atomic rollback or opt-in confirmed batch commits. CSV quoting,
multiline fields, literal versus unquoted NULL markers and blobs are covered.
Standard CSV/TSV has no metadata preamble and preserves text; annotated and
spreadsheet-protected variants are separate explicit choices. REAL display keeps
1.0 distinct, SQL export checks exact engine numeric conversion, and table headers
recover declared type parameters from parsed DDL.

Explicit checkpoint validates native status. Backup checkpoints and closes the
source, rejects remaining WAL data, verifies a private closed-file copy and
publishes a new destination. The app exposes transfer/maintenance controls from
Diagnostics and retained export, preserving the existing retained-only path.

Stage 7 verification: 166 workspace tests, formatting and all-target Clippy pass.
Integration tests export/import 1,205 rows including a 100 KB value, detect missing
end records, distinguish atomic rollback from 1,000 confirmed batch commits, map
quoted multiline CSV/NULL/blob values, execute INSERT RETURNING only once, and
restore recent WAL data from a verified backup. A 512-value floating-point probe
checks accepted SQL spellings against actual engine result bits. Unknown script,
import and generated-commit outcomes block further mutations until reopen.


## Stage 8 — profiling, hardening and qualification

Draft presentation indexes original rows once per draft revision/result identity,
then uses per-row overlays. SQL preview is revision-cached; the review list is
virtualized and formats bindings only for the selected operation. Value previews
inspect at most 101 characters. These changes preserve original typed values and
perform no database writes during rendering.

Final hardening also covers filtered-count ownership and late replies, automatic
schema-diagram refresh, bracket-aware grouped IN parsing, adaptive small-window
paging, exact row eviction and reverse suffix delivery for wide/keyless/deferred
cursor rows. Preference reads/saves are capped at 2 MiB, including files changed
while running; corrupt/newer data cannot replace the original or previous backup.
Anonymous large-value backing is reclaimed by the OS on Linux process termination.

Fault coverage includes real engine transactions with injected failures before
COMMIT, after durable COMMIT acknowledgement, and during rollback; uncertain
outcomes retain drafts and prevent retry. A deterministic failing output writer
verifies INSERT RETURNING finishes exactly once and a failed header prevents
execution. An actual worker unwind verifies disconnection reports UNKNOWN, retains
local drafts and cancels automatic window exit. Spool tests exhaust the 256 MiB
quota and 1,024 handles, reject >64 MiB values and fail expired handles safely.
Other regressions retain a 69-row valid prefix after a decode/query error and
traverse 301 wide/keyless rows in both directions without skipping a short page.
The terminal-event callback test now acknowledges the prior wake before arming its
barrier, removing a test-side race discovered by the full verification run.

Final validation: **174 regular workspace tests**, formatting, warning-free
all-target Clippy, debug desktop smoke, startup-discovery smoke and all four
local-project fixture/desktop smokes pass. The manual 1,000-draft profile also
passes. The release archive is rebuilt, extracted, checksum-verified and installed
to a disposable prefix; its installed binary passes the 100,000-row native smoke.
No push, public release or external database write is part of this work.

### Performance evidence

On this Linux x86_64 Nix host, debug egui frames with 1,000 staged updates and a
1400×900 viewport, after one warm-up frame:

| Scenario | Baseline | Final |
| --- | ---: | ---: |
| Grid, mean of 20 frames | 4.82 ms | 4.54 ms |
| Review dialog, mean of 3 frames | 118.71 ms | 5.34 ms |

The baseline is stage 7 with the same manual fixture/profile added; final timings
include revision caching and virtualized review. Review improves about 22× in
this scenario. The small whole-grid difference is not a general performance claim.
A separate final-code microbenchmark compares the old linear draft lookup with
the cached path on the last 20 rows × 2 columns, repeated 1,000 times: 398.89 ms
versus 6.66 ms. It isolates lookup cost and is not a whole-frame speedup.

The disposable `review_bench` example creates 1M small rows, an integer PK and a
name index. Release-build medians of seven warm-cache SQL runs (100 returned rows,
no GUI/transport), comparing the former all-column sorter with the supported
minimal ordering/keyset SQL:

| Query | Legacy sorter | Indexed alternative |
| --- | ---: | ---: |
| First page | 261.60 ms | 0.023 ms |
| Sort on indexed name | 272.94 ms | 0.032 ms |
| Offset 500,000 | 308.99 ms | 6.60 ms |
| Seek after key 500,000 | — | 0.025 ms |

These are single-host observations with deliberately favorable indexed fixtures,
not universal latency guarantees. Real worker integration tests separately check
EXPLAIN plans, paging, NULL/collation semantics and reference traversal.

Reproduce in the pinned Nix development shell (or prefix each command with
`nix-shell --run`):

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --locked -p tursobench-ui app::interaction_tests::profile_one_thousand_staged_edits -- --ignored --exact --nocapture --test-threads=1
cargo run --release --locked -p tursobench-db --example review_bench
cargo build --locked -p tursobench-ui
bash scripts/desktop-smoke.sh
bash scripts/discovery-smoke.sh
bash scripts/local-project-smoke.sh
bash scripts/package-linux.sh
bash scripts/package-smoke.sh
```

Local verification logs are ignored artifacts under `target/review-stage8-*.log`;
benchmark/profile sources and semantic regressions are tracked. The source is the
reproducible evidence, not a promise that local timing logs exist on another host.

### Finding and gap closure map

| Original finding | Delivery |
| --- | --- |
| High #1 FK false all-clear | Stage 1 supported anti-joins, coverage/errors |
| High #2 oversized cells / wide-page dead end | Stages 5/8 deferred originals and byte-aware continuation |
| High #3 invisible catalog truncation | Stages 1/5 independent catalog budget and lazy DDL |
| High #4 full-table sort per page | Stage 4 minimal indexed ordering/keyset paths |
| High #5 scroll row-height mismatch | Stages 1/8 shared stride and visible-row retention |
| Medium #6 draining reads / queue saturation | Stage 3 verified early stop, cancellation, deadlines and bounded scheduler |
| Medium #7 numeric filters on untyped/view columns | Stage 4 explicit typed literals |
| Medium #8 stale rows/catalog/diagram | Stages 2/8 committed invalidation and schema refresh |
| Medium #9 reference truncation / child-index avoidance | Stage 4 accurate preview limit, paged full views and safe indexed predicates |
| Medium #10 hidden million-row clamp | Stage 4 checked offsets / explicit range errors |
| Medium #11 cursor/Explain buffer handling | Stage 6 shared token-aware selection |
| Medium #12 WITHOUT ROWID editability | Stage 1 shared proven-rowid eligibility |
| Medium #13 writable-open WAL changes | Stage 2 preflight, explicit disclosure and corrected contract |
| Medium #14 no protection against external writer | Stage 2 prominent exclusive-use requirement; no false locking claim |
| Medium #15 preference overwrite | Stages 1/8 tolerant recovery, versioning, backups and bounded safe saves |
| Low read-only file creation / original infinity / partial rows | Stages 2/6/1, with fault regression in 8 |
| Low CSV metadata/protection and REAL/SQL fidelity/type names | Stage 7 standard formats, explicit variants, exact checked decimals and parsed declarations |
| Low repeated draft/review/string work | Stage 8 measured indexing/caching/virtualization and bounded preview |
| Large-data gaps | Stages 5/7 viewer, complete table/query exports and mapped imports |
| Navigation gaps | Stage 4 row jump, total/filtered counts, multi-sort, grouped OR/IN |
| SQL gaps | Stages 3/6 deadlines, scripts and convenient JSON parameters |
| Editing gaps | Stage 6 typed insert form and staged bulk selection deletion |
| Maintenance gaps | Stage 7 explicit checkpoint and coordinated verified backup |

### Qualification still required before production distribution

The implementation work is complete; external release qualification remains
explicit. Deterministic transaction/output failures are not physical storage
syscall/disk-full certification. Engine OOM/resource safety, old-engine fixtures,
multiprocess writers, additional desktop/platform baselines, portal dialogs and
cross-host binary reproducibility are not qualified here.

A fresh audit exits successfully with four unsuppressed upstream warnings: LRU
panic safety RUSTSEC-2026-0253 and D-Bus maintenance RUSTSEC-2026-0370/0371/0372.
The fixed LRU release is outside the pinned engine's dependency range; migration
needs separate compatibility review. Immutable-source notice collection still
finds five missing license texts and fabricates none. These dependency/licensing
items require maintainer review before public distribution. See `release.md` for
exact packages and the audit's crates.io-index refresh warning. This completion
does not claim a stable production release or support for another engine.

Stage commits: 1 `50e9e9f`, 2 `962bd71`, 3 `14700b5`, 4 `a8600db`, 5 `ecda9ff`,
6 `34dbcc8`, 7 `ae9a1ea`; the final stage is the commit containing this record.
