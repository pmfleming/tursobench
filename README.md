<p align="center">
  <a href="packaging/tursobench.png">
    <img src="docs/assets/tursobench-banner.png" alt="TursoBench and its emerald Muirdris database logo. ‘Shut your eyes and see.’ — James Joyce, Ulysses." width="960">
  </a>
</p>

<p align="center">
  <a href="docs/guide.md">User guide</a> ·
  <a href="docs/ui-redesign.md">Workspace guide</a> ·
  <a href="CONTRIBUTING.md">Contributing</a> ·
  <a href="SECURITY.md">Security</a> ·
  <a href="LICENSE">MIT license</a>
</p>

**TursoBench** is a local desktop workbench for browsing Turso databases,
exploring relationships, editing records and writing SQL. Built with Rust and
egui. **Linux preview · Turso 0.8.3 · Read-only by default.**

> **Independent project.** TursoBench is not affiliated with, aligned with,
> endorsed by, or maintained by Turso or the Turso team.

<picture>
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/ui/v6/laptop-light.png">
  <img src="docs/assets/ui/v6/laptop-dark.png" alt="Current TursoBench workspace: relationships above table data, with Record, Structure and SQL alongside the database navigator" width="1366">
</picture>

*Native Linux app at 1366 × 768, using synthetic data. The screenshot follows
your page's light or dark appearance.*

## One workspace, five fixed tiles

Relationships sits above Table data. Record, Structure and SQL stay alongside
it. Select a row to inspect and edit the same live record in the Record tile;
Structure shows the table's columns, keys, indexes and original DDL.

Each tile has a compact icon ribbon. **Minimize** reduces it to a header;
**expand** fills the workspace while keeping the database navigator visible.
Tiles stay in fixed positions. History, saved queries, checks, settings and help
live in the navigator footer.

- **Browse tables:** searchable schema, sorting, typed filters and bounded
  100-row page fetches. Filters apply to the entire table before loading matching
  rows. Reorder, resize and hide columns; copy selected cells or export retained
  results as JSON, CSV, TSV or SQL.
- **Explore relationships:** declared foreign-key maps, composite keys, linked
  records and related-row lookups. Expand the map for a closer look.
- **Open and switch databases:** discover local files, search recent databases,
  choose search folders and review access before opening. Writes require explicit
  opt-in.
- **Edit records:** stage updates, inserts, clones and deletions locally. Commit
  explicitly from the edit dock, or open the optional Diff / SQL review first.
  Atomic batches check original values for conflicts.
- **Write SQL:** syntax highlighting, typed parameters, selection execution,
  completion, EXPLAIN and native Turso FTS. Editors and linked results share the
  SQL tile; history and named bookmarks keep useful queries accessible.

Follow the system theme or choose Light/Dark. Status pills and priority/progress
bars use automatic column detection with manual overrides. Foreign-key links
come from database metadata. Cell styling never changes stored or exported values.

## A closer look

**Expanded relationships — the database navigator stays visible.**

![Expanded relationship map showing the declared link between synthetic projects and items](docs/assets/ui/v6/expanded-map.png)

<details>
<summary>Minimized tiles and widescreen layout</summary>

**Minimized tiles become compact headers.**

![Record, Structure and SQL minimized to headers while relationships and table data remain open](docs/assets/ui/v6/minimized-tiles.png)

**The same fixed arrangement on a 3440 × 1440 display.**

![All five tiles visible on a widescreen display, including status pills and priority bars in the data grid](docs/assets/ui/v6/widescreen.png)

</details>

All screenshots show the native app, not the HTML mockup.
[Capture details](docs/assets/ui/README.md) · [App logo](packaging/tursobench.png)

## Run

Requires the pinned Rust toolchain and Linux desktop libraries:

```sh
cargo run --locked -p tursobench-ui -- /path/to/existing.db
```

Or use the pinned Nix development environment:

```sh
nix develop -c cargo run --locked -p tursobench-ui -- /path/to/existing.db
```

Omit the database path to open the chooser.

Use trusted local files, close other database clients, and back up before writes.
Live multiprocess access, cloud sync and other engines are unsupported. This is a
preview; discarding results does not cancel SQL execution or roll back writes.

[Setup and limits](docs/guide.md) · [Release status](docs/release.md) ·
[Code of conduct](CODE_OF_CONDUCT.md)

[MIT license](LICENSE). Dependencies retain their own licenses.
See [third-party notices](third-party/README.md).
