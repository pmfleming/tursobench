# Shared database access

Baseline: `639355f`; all preceding UI and database changes were committed and
the working tree was clean before this implementation.

Implementation stages (one commit per completed stage):

1. Qualify the pinned Turso 0.8.3 multiprocess WAL protocol on disposable files.
   Verify actual coordination, concurrent writer/viewer behaviour, checkpoints,
   restart/recovery, incompatible opens, and read-only source preservation.
2. Add shared read-only sessions and best-effort process diagnostics. Replace
   the user's exclusive-use assertion with verified connection status.
3. Invalidate data/schema caches when external commits become visible without
   discarding SQL text, pending edits, or manual transaction snapshots.
4. Qualify shared editing and expose only supported operations. Preserve busy,
   conflict, partial and unknown outcomes; never automatically replay writes.
5. Run workspace verification and document the qualified support boundary.

Process enumeration is explanatory, never proof of exclusive use. Compatible
participants must use the same WAL coordination protocol. A successful legacy
read-only open is not evidence of safe sharing. No live user database is used
for qualification, and no other application's configuration is changed.

## Using shared access

The other application must explicitly enable compatible native multiprocess WAL,
for example `turso::Builder::new_local(path).experimental_multiprocess_wal(true)`
with the pinned Rust SDK. Open that application first, then attach TursoBench
read-only. Applications using the default legacy writer mode must close their
connection or change their configuration before sharing. TursoBench does not
reconfigure other applications. Shared editing uses the same protocol.

The qualified environment is Linux x86_64 on local storage with Turso 0.8.3 SDK
peers and ordinary WAL databases, including the tested native FTS indexes.
Standalone read-only access reserves the file until closed; later compatible
writers must wait or retry, and another TursoBench session reports Busy promptly.
Some core/CLI clients probe an exclusive main-file lock and therefore refuse
opening while TursoBench is connected, even with the experimental flag. Arbitrary
SQLite clients, other engine versions, network filesystems and MVCC are not
qualified. Hard-linked databases are rejected: two names can otherwise create
different WAL/coordination paths for the same inode. Do not rename or replace
source/sidecar files while connected, or disable advisory locking in peers.

Read-only protects database, WAL and FTS contents; the writable coordination
sidecar is separate from those contents and may be created. Process diagnostics
inspect open descriptors, so permission restrictions and mmap-only users can
hide applications. They are never a substitute for protocol-compatible locks.

## Stage 1 evidence

`cargo test --locked -p tursobench-db --test suite shared:: -- --test-threads=1`
passes on Linux x86_64 with separate processes. Twelve write/checkpoint/truncate
cycles remain visible to an open reader. Read-only SQL rejects writes without
changing the database/WAL bytes. A killed writer's pending transaction remains
invisible and a replacement writer recovers. Native FTS searches see external
updates. Closed read-only opens without a `.tshm` mapping report no coordination.

Qualification also reproduced a gap: the SDK's custom-storage open path admits
a multiprocess writer alongside a live legacy writer. The builder flag is not
sufficient admission control. Stage 2 must acquire and retain a Linux OFD shared
lock over the main file before any engine open; this conflicts with legacy
writers' whole-file POSIX locks, survives unrelated descriptor closes, and allows
compatible SDK peers. A read-only fallback retains that lock and must report
protected read-only access rather than shared WAL. Core/CLI opens that probe an
exclusive main-file lock may conservatively refuse while TursoBench is open.

## Stage 2 implementation

The user assertion is removed. Every source session holds a Linux OFD main-file
read lock. Before any engine schema read, admission attempts an exclusive lock
on `DATABASE-tshm` (the pinned engine's actual sidecar name). When no native
coordinator is active, read-only sessions retain that reservation and use the
legacy read path. Both legacy writers and future native coordinators are then
excluded. A writer that waited on an empty reservation may need to retry its
open after the reader closes. TursoBench itself checks the native lifetime byte
nonblockingly and reports Busy instead of waiting in the engine's blocking open.

With an active coordinator, admission holds a temporary shared lock on its
lifetime byte until the engine has joined. Verified engine telemetry is required;
silent read-only fallback is rejected. Writable sessions validate the schema
read-only under admission before releasing the reservation and initializing or
joining shared WAL. Read-only database/WAL/FTS content remains unchanged, but a
coordination sidecar may be created and is intentionally retained to avoid
unlink/recreate races. The directory and coordination file must be writable.

Bounded asynchronous Linux process diagnostics compare device/inode identities
for the database and sidecars, report names/PIDs and acknowledge incomplete
visibility. They never decide admission. File locks, not process enumeration,
enforce the supported protocol. Raw file writes, lock-disabled clients, sidecar
replacement and other nonparticipating access remain unsupported.

Backup now requires an exclusive coordination reservation after engine close,
while the main-file admission lock is still held. It refuses publication if any
shared peer remains or WAL data remains. This prevents a main-file copy racing a
compatible writer. Workspace verification follows in subsequent stages.

## Stage 3 external changes

Shared sessions check main/WAL identity, nanosecond modification/change times and
length every 500 ms on the worker between operations. The pinned engine has no
public `data_version` pragma. File changes are conservative notifications (a
checkpoint or uncommitted WAL flush can also trigger them), not a commit log.
Committed schema versions are read on the autocommit browse connection. Busy or
interrupted polls retry on the next interval. Source removal/replacement closes
the session with an explanation. No polling occurs inside a long-running request;
it resumes when that serialized operation finishes.

Notifications invalidate retained pages, related-row caches, counts and metadata.
Clean table/related views refresh with retained focus and scroll/selection where
the original row still exists. SQL results are marked stale and never replayed.
Refresh waits during staged edits or a manual editor transaction; pending edits
and the editor's snapshot stay intact. Schema notifications refresh the catalog,
relationships and structure tiles, including another pass when a diagram load
was already pending. Metadata polling cannot certify every unusual filesystem;
explicit refresh remains available.

Real-process data/DDL notification and manual-snapshot tests pass. The GUI test
exercises automatic table refresh, retained SQL results/focus, and draft
preservation through external data and schema changes.

## Stage 4 shared editing

Shared writable sessions use the native WAL writer lock and bounded busy timeout;
there is no application write replay. Busy/snapshot errors retain distinct
messages and observed transaction state. Generated edits recheck schema and
original row values inside `BEGIN IMMEDIATE`; imports recheck their target schema
inside each batch's writer transaction. Successful generated commits invalidate
other views immediately. Partial and unknown commit handling remains intact.

The matching parser rejects journal/locking-mode setters, `BEGIN CONCURRENT`,
ATTACH/DETACH and VACUUM on single SQL, script and execution/export paths. Scripts
validate every statement before the first executes, so a forbidden later
statement cannot leave earlier writes behind. Read-only enforcement still applies.
The access-switch dialog uses close acknowledgement and fresh lock checks instead
of asking the user to assert exclusive use.

Separate-process tests pass for contended SQL, atomic scripts, imports and staged
edits; original-value conflicts; active-reader checkpoint refusal; live-peer
backup refusal without publication; and 80 simultaneous increment attempts with
exact accounting of successful writes. Forty rounds of updates from both writers
retain correct FTS query results. Both source and surviving peer remain usable
and pass integrity checks. These bounded scenarios qualify this Linux preview;
they do not certify arbitrary workloads or unqualified engines/filesystems.

## Final verification (2026-11-04)

- `cargo fmt --all --check` and workspace/all-target Clippy with warnings denied.
- `cargo test --locked --workspace`: 192 passed; one manual profile ignored.
- Native Xvfb/Mesa desktop smoke on the closed 100,000-row compatibility fixture.
- All four synthetic local-project schemas pass their database cases and native
  desktop smokes, including the clipboard native FTS schema.
- The isolated startup-discovery smoke passes without opening or modifying any
  candidate database; removed defaults, exclusions and opt-out remain effective.
- Immediate close/reopen releases every admission lock before acknowledgement;
  repeated protected reads preserve source contents. Hard-link aliases fail
  before any coordination sidecar is created.

The engine version/implementation is unchanged. No live user store was modified,
no other application was reconfigured, and no release archive was repackaged.
Extended soak, physical I/O/OOM faults, other engine versions, other platforms
and network filesystems remain outside this qualification.

## Open-panel feedback

The compact open panel shows selection and access controls without permanent
engine/protocol explanations. A background `check_access` queries existing Linux
locks with `F_OFD_GETLK`, validates basic file prerequisites, and creates no
sidecars. It neither acquires a coordination lifetime lock nor opens an engine.
Same-process POSIX owners are detected before opening another descriptor so the
probe cannot release their existing locks. Compatible active native WAL writers
remain eligible; a protected reader or legacy writer is reported as a blocker.

Known blockers disable Open and show a persistent inline reason. Blocked checks
repeat every two seconds, with a manual recheck action. Actual engine failures
are retained separately from transient status text, expose Retry, and clear on
new selection/access or another opening attempt. Preflight is advisory; actual
session admission still rechecks all safety conditions, including races after
inspection. Tests cover lock preservation, shared-writer eligibility, missing
file recovery, visible failures in both themes, and late engine failure during
close/exit.

Open-panel verification: 196 workspace tests pass (one manual profile ignored),
strict all-target Clippy and formatting pass, and the rebuilt application passes
the native desktop smoke on the 100,000-row disposable fixture.
