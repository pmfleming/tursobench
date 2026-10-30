# Scoped FTS compression dependency patch

`tantivy-sstable/` contains the published `tantivy-sstable` **0.7.0** crate's
unchanged Rust sources, tests and benchmarks. It is used by Turso 0.8.3's
Tantivy 0.26.2 dependency. Source archive:

- https://static.crates.io/crates/tantivy-sstable/tantivy-sstable-0.7.0.crate
- SHA-256: `8a2cfc3ac5164cbadc28965ffb145a8f47582a60ae5897859ad8d4316596c606`
- Published VCS provenance is retained in `.cargo_vcs_info.json`.
- MIT license text is copied verbatim from the published Tantivy 0.26.2 crate.

Only two manifest changes are made: `zstd = "0.13"` becomes `"0.14.3"`, and the
unused `names` dev dependency is removed. No Rust implementation is altered.
This independently applies the scoped fix already used by the local clip-daemon
project, rather than importing its storage implementation.

Reason: enabling FTS otherwise introduces RUSTSEC-2026-0391 (`zstd`) and
RUSTSEC-2026-0403 / RUSTSEC-2026-0398 (`zstd-safe`). The new lockfile selects
released zstd 0.14.3 and zstd-safe 8.2.0. No advisory is suppressed. A remaining
upstream `lru` panic-safety warning is documented separately.

Remove the patch when an allowed Turso/Tantivy release supports the fixed
compression dependency range. The vendor package is excluded from workspace
formatting/linting to keep its source snapshot unchanged. Release license
collection must include this path dependency, not just registry packages.
