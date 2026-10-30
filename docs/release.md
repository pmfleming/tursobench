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
- Native Turso FTS and experimental index methods enabled, including read-only
  opening and generated edits of FTS-indexed tables. See `docs/fts.md`.
- A tursotui workbench gap-closing pass: schema/DDL/index/FK inspection and navigation,
  SQL highlighting/completion/selection, named parameters, EXPLAIN, read-only
  diagnostics, typed editing/undo tools, richer exports/copying, timed scoped history
  and named bookmarks. See `docs/tursotui-review.md` for evidence and remaining gaps.
- Responsive SQL/Data/Diagram workspaces, activity rail, docked/pop-out inspector,
  readable grid and staged review, system-font fallback, and bounded native schema
  diagrams. See `docs/ui-redesign.md` for the capability map and limits.
- Formatting, warning-free Clippy, 43 workspace unit/integration tests.
- Four exact local-project schema fixtures (scratchpad, agenda, notifications,
  clip-daemon): synthetic Turso-created data, read-only byte preservation, paging,
  typed edits, FTS transaction consistency, rendering/exports, and native Xvfb smoke.
- Native eframe/OpenGL smoke under Xvfb/Mesa against a closed 100,000-row fixture:
  open, schema, browse first page, typed blob query, engine read-only write rejection,
  acknowledged close. The redesign adds actual data/diagram render frames to this
  sequence; debug runs observed ~0.90–2.56 s for the main smoke and ~0.41–1.31 s
  for project smokes on this host. Both-theme egui layout tests cover laptops, widescreens and 1.5× UI scale;
  native Xvfb screenshot review covers schema/data, diagram and compact panes.
  Earlier, after the workbench additions, this Linux x86_64 Nix host's
  debug sequence took ~814 ms; the four local-project desktop smokes took ~207–234 ms.
  An optimized workbench-validation run took ~133 ms; local-project smokes took
  ~95–98 ms. That pre-redesign preview package was rebuilt; rebuild the archive
  to include the subsequent UI redesign. Earlier phase-5 validation also
  included a temporary-prefix install smoke. These are single-host observations,
  not performance guarantees.
- Engine-only 100,000-row drain measured ~0.95 s in debug. Application retention
  budgets are tested; total engine/process memory is not bounded or certified.

## Deliberate scope reductions / remaining release gates

- Exclusive-use local files only; no safe live multiprocess writer claim.
- No interrupt/timeout stop guarantee. Discard is delivery suppression only.
- OFFSET paging, not keyset; no snapshot consistency or deep-page latency guarantee.
- Exports stream retained results to disk, not unlimited complete-table queries.
- No script execution (multi-statement buffers can run one selection/statement at
  a time), custom types, WITHOUT ROWID or generated-column editing. Experimental
  index methods are enabled for native FTS; other experimental flags remain off.
  No other database engines, ever.
- FTS dependency audit: the scoped zstd patch removes three vulnerability findings.
  `cargo audit` passes but reports upstream `lru` panic-safety warning
  RUSTSEC-2026-0253; no advisory is suppressed. This is still a release-review item.
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
  immutable URL/hash provenance. Five crates omit license texts in inspected
  releases/upstream trees (`crc32c`, `genawaiter`, `genawaiter-macro`, `htmlescape`,
  `pack1`);
  `MISSING-LICENSE-TEXTS.txt` flags them for manual review before public release.
  License collection is not legal review or permission to ignore missing notices.

## Package workflow

`bash scripts/package-linux.sh` builds with `--locked --release`, collects notices,
and creates `target/tursobench-0.1.0-preview-linux-x86_64.tar.gz` plus SHA-256.
The manual GitHub workflow produces preview artifacts, not an automatic stable
release. Extract and run `bash install.sh /desired/prefix`; default is `$HOME/.local`.
The desktop launcher expects `tursobench` on PATH. Validate on the intended Linux
baseline before distribution. Check the bundled missing-notice report first.
