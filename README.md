<p align="center">
  <img src="docs/assets/tursobench-banner.png" alt="TursoBench — a local-first database workbench. ‘Shut your eyes and see.’ — James Joyce, Ulysses. Linux preview, built with Rust and egui." width="960">
</p>

<p align="center">
  <a href="docs/guide.md">User guide</a> ·
  <a href="CONTRIBUTING.md">Contributing</a> ·
  <a href="CODE_OF_CONDUCT.md">Code of conduct</a> ·
  <a href="SECURITY.md">Security</a> ·
  <a href="LICENSE">MIT license</a>
</p>

**TursoBench** is a local-first desktop workbench for **Turso** databases,
built with Rust and egui. Browse tables, explore relationships, write SQL and
review edits before committing. Currently a **Linux preview**, pinned to Turso 0.8.3.

> **Independent project.** TursoBench is not affiliated with, aligned with,
> endorsed by, or maintained by Turso or the Turso team.

| 🔎 Explore | ⌨️ Query | ✏️ Edit |
| --- | --- | --- |
| Search tables, filter rows and map relationships. | SQL highlighting, typed parameters, EXPLAIN and native Turso FTS. | Stage changes, inspect SQL and commit with conflict checks. |

Read-only by default · Local history and bookmarks · JSON, CSV, TSV and SQL exports

## Get started

With the pinned Rust toolchain and Linux desktop dependencies installed:

```sh
cargo run --locked -p tursobench-ui -- /path/to/existing.db
```

Or use the pinned Nix environment:

```sh
nix develop -c cargo run --locked -p tursobench-ui -- /path/to/existing.db
```

Use trusted local files, close other applications using the database, and back up
before enabling writes. Live multiprocess access, cloud sync and other database
engines are unsupported. See the [setup and usage guide](docs/guide.md) for
dependencies, sample data, shortcuts and limits, and [release status](docs/release.md)
for preview caveats.

## Join in

Bug reports, documentation improvements and focused pull requests are welcome.
Read [Contributing](CONTRIBUTING.md) and the [Code of conduct](CODE_OF_CONDUCT.md).
Report vulnerabilities using the [Security policy](SECURITY.md).

Released under the [MIT license](LICENSE). Dependencies retain their own licenses;
see [third-party notices](third-party/README.md).
