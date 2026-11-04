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
