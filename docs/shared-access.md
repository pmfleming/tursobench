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
