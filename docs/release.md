# 0.1.0 Linux preview status

The original six delivery phases and the eight-stage 2026-11-04 review remediation
have implementation commits. This remains a **Linux preview**. Completed features,
local verification and open production gates are separate below. The detailed
issue map and reproducible scenarios are in [review remediation](review-remediation.md).

## Implemented and locally exercised

- Rust 1.95, eframe/egui 0.37 and Turso 0.8.3 with explicit native FTS support.
  Pinned wrapper methods expose existing SDK interruption/deadlines; the engine
  version and implementation are unchanged. Vendor patches include provenance.
- Worker-owned connections, bounded admission/delivery, queue state, stale-event
  protection, explicit access consent, managed transaction cleanup and truthful
  partial/unknown outcomes. Committed writes invalidate other retained views.
- Supported metadata FK validation, visible catalog truncation, lazy DDL, indexed
  ordering/keyset paging where qualified, byte-aware continuation and stable scroll
  eviction. Grouped typed filters, multi-sort, row jumps and explicit counts.
- Bounded large-value text/hex viewer/save, cursor/selection execution, sequential
  or atomic scripts, convenient JSON parameters, typed inserts and staged bulk
  deletion. Draft review is cached and virtualized.
- Complete-table / execute-once query JSONL export; typed JSON/JSONL and mapped
  CSV/TSV imports; standard versus explicitly protected/annotated delimited files;
  lossless SQL-number validation; explicit checkpoint and coordinated verified backup.
- Formatting and all-target Clippy with warnings denied pass. **179 regular
  workspace tests pass** after the preferences and tile UI changes. The manual frame
  profile was exercised separately at the remediation commit.
  Application fault tests cover commit/rollback boundaries, lost durable-COMMIT
  acknowledgement, output writer failure, actual worker unwind, preference
  corruption, spool quota/handle exhaustion and oversized inputs/metadata.
- Native Xvfb/Mesa debug smoke against a closed 100,000-row fixture passes
  (open, schema, page, diagram, typed query, read-only rejection and close).
  Isolated startup discovery and all four exact local-project schema/desktop
  smokes pass. Existing headless UI coverage includes both themes, laptop and
  widescreen layouts, 1.5× scale, staged edits, dialogs and chooser outcomes.
- A Linux release archive is rebuilt, extracted and installed into a temporary
  prefix; the installed release binary passes the native desktop smoke. Notices,
  docs, lockfile, installer, launcher and icons are bundled. Archive metadata is
  deterministic; this is not proof of reproducible binary compilation.

The 1M-row release SQL benchmark measures the intended indexed/keyset paths;
the 1,000-draft profile measures review rendering and lookup costs. Exact host
observations, methodology and caveats are in the remediation record. Timings do
not certify general latency or total process-memory limits.

## Remaining release gates and scope limits

- **Exclusive-use local files only.** The checkbox does not exclude unrelated
  writers. Read-only protects the opened database; it does not establish a shared
  writer lock. Writable opening may migrate DELETE journal mode to WAL and says so
  before opt-in. Backup is coordinated checkpoint/close/copy/verification, not a
  live external-writer snapshot.
- Engine deadlines are cooperative. Application budgets do not bound engine
  allocations, a complete incoming row, process memory or shutdown latency.
  Process OOM, physical engine disk-full/I/O faults and every storage syscall
  failure have not been certified. Deterministic boundary/writer failures test
  application outcome handling and must not be described as full storage fault
  qualification.
- Keyset paging requires supported non-null keys; fallback OFFSET can be slow.
  Paging is not a snapshot. Deferred values expire at explicit quotas. Viewer
  search covers its current chunk. Imports are limited to documented record and
  decoded-row sizes. Generated editing excludes views/keyless tables, WITHOUT
  ROWID and hidden/generated columns. Other database engines are out of scope.
- No old tursotui-engine fixture or broad file-format compatibility claim. No
  live external application stores were used. Custom types, sync/encryption and
  other experimental engine features remain disabled/unqualified.
- Fresh `cargo audit` (2026-11-04) exits successfully with **four upstream
  warnings**, none suppressed: `lru` RUSTSEC-2026-0253 and the unmaintained
  `zvariant`, `zbus_names`, `zvariant_derive` notices RUSTSEC-2026-0370/0371/0372.
  The LRU fix requires >=0.18.2, outside the pinned engine's 0.16 dependency range;
  D-Bus notices require an upstream ecosystem migration. These remain explicit
  dependency review gates, not a clean security certification. The prior scoped
  zstd patch remains in place. Audit refreshed advisories but its crates.io-index
  update warned unavailable; locked dependency/advisory scanning completed.
- License collection verifies bundled/supplement hashes, including both path
  patches. **Five missing texts remain**: `crc32c`, `genawaiter`,
  `genawaiter-macro`, `htmlescape`, `pack1`. A fresh immutable-source supplement
  lookup found no additional texts. Do not invent copyright notices or treat SPDX
  metadata as replacement permission. Manual licensing review is required before
  public distribution; inspect bundled `MISSING-LICENSE-TEXTS.txt`.
- Native portal/file-picker and broader compositor/window-manager manual checks
  remain. Xvfb and headless interaction tests cannot qualify every desktop.
  Linux x86_64 is the local target; configured Ubuntu CI has not run here.
  Nix-linked artifacts require the Nix runtime environment. macOS/Windows and
  cross-host binary reproducibility need separate qualification.

## Package workflow

Subsequent preferences storage change: application settings/history/layouts now
use a local Turso `preferences.db`, with transactional current/previous snapshots,
worker-owned writes and shutdown draining. JSON files and legacy formats are not
imported. Persistence regression tests replace the former JSON migration tests;
see [current storage details](data-formats.md). Rebuild preview packages to include
this subsequent change.

`bash scripts/package-linux.sh` builds with `--locked --release`, collects notices
and creates `target/tursobench-0.1.0-preview-linux-x86_64.tar.gz` plus SHA-256.
The manual GitHub workflow produces preview artifacts, not a stable release.
Extract and run `bash install.sh /desired/prefix`; the default is `$HOME/.local`.
`bash scripts/package-smoke.sh` verifies the trusted local archive, installs into
a temporary prefix and runs the installed binary under Xvfb.
The desktop launcher expects `tursobench` on PATH. Validate on the intended Linux
baseline and clear the dependency/license gates before public distribution.
