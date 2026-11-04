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
