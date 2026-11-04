# Turso 0.8.3 Rust wrapper patch

Source: published turso 0.8.3 crate, https://static.crates.io/crates/turso/turso-0.8.3.crate
SHA-256: f1cf9c519a3a971e845a42c870930557cb53c2368c5737d4c78f8fbfd0444e98
Upstream revision: 607bff9fd0e547a25a97b3bf06e83bcc5d99d53c

Only behavioral additions: public Connection::interrupt and Connection::query_timeout
forward to methods already present in turso_sdk_kit 0.8.3. No engine implementation,
version, feature or file-format changes. The upstream MIT text is included in
LICENSE.md from the repository's existing immutable license supplement. Registry
packaging markers and the package-local lockfile are omitted. The workspace uses
its committed lockfile. Real-engine deadline/cancel tests live in
crates/tursobench-db/tests/cases/session.rs.
