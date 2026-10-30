# 0.1.0 Linux preview status

All six delivery phases have an implementation commit. This is a **preview**,
not a declaration that every production release gate in PLAN.md has passed.

## Implemented and locally exercised

- Turso 0.8.3 disposable engine probes; explicit feature configuration.
- Typed core, dedicated session worker, bounded queues/delivery, stale-event checks.
- Read-only desktop browsing, SQL parameters and cell inspection.
- Opt-in writes, persistent editor transactions, separate browsing, safe close.
- Staged atomic editing with conflict detection, constraint rollback and draft retention.
- History/bookmarks/settings, typed displayed-result export, Linux packaging scripts.
- Formatting, warning-free Clippy, 19 workspace unit/integration tests.
- Native eframe/OpenGL smoke under Xvfb/Mesa against a closed 100,000-row fixture:
  open, schema, browse first page, typed blob query, engine read-only write rejection,
  acknowledged close. On this Linux x86_64 Nix host the debug sequence took ~270 ms;
  optimized runs took ~96–149 ms (latest ~149 ms), including a temporary-prefix install smoke test. This is a single-host
  measurement, not a general performance guarantee.
- Engine-only 100,000-row drain measured ~0.95 s in debug. Application retention
  budgets are tested; total engine/process memory is not bounded or certified.

## Deliberate scope reductions / remaining release gates

- Exclusive-use local files only; no safe live multiprocess writer claim.
- No interrupt/timeout stop guarantee. Discard is delivery suppression only.
- OFFSET paging, not keyset; no snapshot consistency or deep-page latency guarantee.
- Exports stream retained results to disk, not unlimited complete-table queries.
- No scripts, custom types, WITHOUT ROWID, generated-column editing or other
  experimental features. No other database engines, ever.
- Fault-injected commit/rollback I/O failure and resource exhaustion remain untested.
  Unknown-outcome handling is implemented conservatively, not empirically certified.
- No old tursotui-engine fixture coverage or broad file-format compatibility claim.
- Native portal/file-picker manual checks and broader desktop UX checks remain;
  Xvfb smoke does not exercise every window manager or native dialog.
- Linux only. Ubuntu CI/package jobs are configured but have not run in this local
  environment. Nix-linked local artifacts require the Nix runtime environment.
- Locked dependencies, pinned Rust toolchain and deterministic archive metadata are
  supplied. Repeated archive assembly with the same binary produced matching SHA-256
  hashes locally. Cross-host/bit-for-bit binary reproducibility has not been established.
- License inventory includes verbatim bundled/upstream texts, font notices, and
  immutable URL/hash provenance. Four crates omit license texts in inspected
  releases/upstream trees (`crc32c`, `genawaiter`, `genawaiter-macro`, `pack1`);
  `MISSING-LICENSE-TEXTS.txt` flags them for manual review before public release.
  License collection is not legal review or permission to ignore missing notices.

## Package workflow

`bash scripts/package-linux.sh` builds with `--locked --release`, collects notices,
and creates `target/tursobench-0.1.0-preview-linux-x86_64.tar.gz` plus SHA-256.
The manual GitHub workflow produces preview artifacts, not an automatic stable
release. Extract and run `bash install.sh /desired/prefix`; default is `$HOME/.local`.
The desktop launcher expects `tursobench` on PATH. Validate on the intended Linux
baseline before distribution. Check the bundled missing-notice report first.
