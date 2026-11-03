# Local database discovery

TursoBench starts one background search when the application launches. The Open
database screen immediately shows remembered files, then appends new candidates
without changing the selected path, access mode, keyboard focus or scroll
position. Recent successful opens appear first on the next launch. **Ctrl+O**
returns to this chooser and focuses its name/path filter. **Browse files…** beside
the search field opens the native file dialog; **Alt+Enter** does the same while
search is focused. Manual entry is available under **Enter a path…**, and a selected
file's path can be changed with **Edit path…**.

The centered chooser has a rounded search field above full-width filename/path
rows. Recent successful opens appear first and carry a **Recent** label; other
rows show their discovery or availability status. A selected file has a filled
row, while keyboard navigation uses a separate browsing marker. Down from search enters the first selectable
result; Up/Down browses results, skipping unavailable files. Up from the first
result returns to search. PageUp/PageDown moves through longer lists and reveals
the highlighted row. Typing or pasting while browsing edits the search at its
retained cursor/selection. Enter selects the highlighted file for access review;
clicking a row also selects it. Browsing alone does not replace a command-line or
previously selected path, and incoming discoveries preserve the highlight.

Selecting a candidate does not connect to it. A compact access panel directly
below the results shows the selected file, read-only/write choice, exclusive-use
acknowledgement and **Open database**. Long paths are truncated with the full path
on hover. Changing the selected path resets write access, acknowledgement, and
the SQLite classification override.
Command-line paths keep priority.

## Scope and controls

Search prioritizes added folders and recent-file parents, then `$XDG_DATA_HOME`
(normally `~/.local/share`) and `$XDG_STATE_HOME` (normally `~/.local/state`),
before the launch directory, files directly in the home directory, and common
development folders (`Projects`, `projects`, `Code`, `code`, `Developer`,
`Development`, `src`). The launch and development folders have a five-level
recursion limit, recent parents one, application data/state three,
and home zero. Launching from home never makes all of home recursive.

The **Search settings** icon beside the database count opens a compact scrollable
dialog containing all search-folder controls and search feedback. **At startup** toggles
automatic discovery. The refresh icon reruns the configured search and becomes a
stop control while searching. Icon buttons have tooltips and accessible names.

Under **Search folders**, use the folder icon to pick a location, or enter an
absolute path and press **Enter** or **+** to save and search it. Under **Excluded**,
enter a path and press **Enter** or **+** to exclude it. The **×** on each folder
row removes that entry; it never deletes files. Refresh after removing an
exclusion. Long paths show in full on hover. The footer shows header, folder,
unavailable and skipped counts with labels on hover; **Limit reached** means
only part of the configured locations was searched.

These controls are also available as Database discovery in Settings.
When a search reaches its budget, use the folder icon in **Search folders** to
search a narrower location directly.
**Add and search folder** saves an absolute folder and searches that folder
immediately, including extensionless files, to five levels. This manual search
also works when startup discovery is disabled. Use a narrower folder to look
past an automatic depth or resource limit. **Refresh databases** reruns the configured
search. **Stop search** requests cancellation; another worker cannot start until the
current one finishes. File browsing remains available during a search.

Automatic discovery skips system folders, network/FUSE/unknown filesystems,
automounts, standard removable-media locations and devices marked removable by
Linux. Browser profiles, dependencies, build output, caches, trash, VCS internals
and most hidden directories are excluded. Hidden `.data`, `.turso`, `.libsql` and
`.local` directories within searched roots remain eligible. `.gitignore` is not
consulted: real development databases are often intentionally ignored.

Browser detection also recognizes custom Chrome/Chromium folder names and profile
markers such as `Local State`, rather than relying only on standard folder names.
Certificate/key stores and known desktop address-book/file-manager stores are
excluded. Old cached discoveries in those locations are pruned on refresh or
startup; deliberately opened recent files remain available in history. Explicitly
adding a profile folder itself opts into searching it.

Added folders opt into otherwise skipped root locations, including network
locations. User exclusions always take precedence. No directory symlinks are
followed, even in a root's ancestors, and traversal never crosses a nested mount.
Add a mounted folder itself to opt into that folder. Symlink files and special
files are skipped; discovered hard links and overlapping roots are deduplicated.
On Linux, a missing/unreadable mount map disables automatic roots conservatively;
explicit folder searches and native file selection remain available.

## Work and identification limits

Each run has a cooperative two-second deadline, 20,000 directory entries, 1,000
directories and 500 header probes. It stops at the first exhausted budget. One
worker sends through a 32-event channel, the UI processes at most 64 events per
poll, and the candidate list renders only visible rows. There are no filesystem
watchers or recurring scans. Ordinary file operations can exceed the deadline;
cancellation cannot interrupt a filesystem syscall already in progress. The UI
does not wait for or join the worker.

Discovery reads metadata and at most 100 header bytes per probed file. It never
creates a database connection, reads schema/pages, performs integrity checks or
writes database/sidecar files. It first considers `.db`, `.sqlite`, `.sqlite3`,
`.db3`, `.s3db` and `.turso` names. Extensionless files are checked in application
data, recent-file parents and added folders. Remembered paths are checked
regardless of extension. WAL, SHM, journal and Turso `.db-log` sidecars are excluded.

Each row shows the database name and its two nearest containing folders, such as
`clip-daemon/history`. Hover shows only the full file path. The icons are:

- **Turso logomark with a check:** successfully opened with Turso. This is restored
  from successful-open history; it confirms past compatibility, not the creating
  engine or the current state of a file that may have changed.
- **Turso logomark:** likely Turso, based on the current header check.
- **Generic database:** another or unidentified database, including likely SQLite.

The marks use [Turso's official brand assets](https://turso.tech/brand), with aqua
on dark themes and dark teal on light themes. Status is also available to screen
readers. Missing/unreadable files retain a short status label. Right-click a row
for classification evidence, file size and Forget. Header classifications are not
cached; only successful-open history restores the checked Turso mark. The
SQLite-format header is shared by Turso and SQLite.

Classification uses the existing 100-byte header read. Turso 0.8.3 initializes
the writer version and version-valid-for fields to 3047000, with a change counter
of 1, WAL read/write versions and schema format 4. This combination suggests
Turso. A SQLite 3.x writer version with a version-valid-for field matching the
change counter suggests SQLite. Other combinations remain unknown. The writer
version alone is never treated as a Turso signature. These are heuristics, not
proof of the creating engine, integrity, or compatibility. The main-file header
can lag WAL state, and another engine may have modified it. Reference:
[SQLite database header specification](https://www.sqlite.org/fileformat.html).

Selecting any file, including Browse/manual/command-line selections, starts a
separate background header check. Open waits for that check. A **Likely SQLite**
file remains selectable, but requires **Open with Turso anyway** in its access
panel. The override applies only to the current selection and is not persisted.
Unknown files and failed inspections remain eligible for normal engine
validation on Open. This override uses the existing Turso engine; it neither
switches engines nor converts files. Access mode and exclusive-use acknowledgement
still apply. Engine validation happens on Open. Encrypted files,
empty databases, damaged headers, unusual extensions and files outside the
configured scope can be missed. This is a search of likely locations, never a
claim to have found every database on disk.

Cached entries are explicitly unverified until checked. Missing files and files
without recognized headers are removed from the candidate cache; recent history
remains visible with its failure status until forgotten. Unreadable and skipped
locations are distinguished from missing files. Every Open revalidates through
the existing engine path, including changes between discovery and selection.
Non-UTF-8 paths remain native while scanning and display an unsupported-path
label; the current Turso builder cannot open them, and they are not saved.

Saved paths and metadata live only in application preferences, alongside query
history. No schema or database content is cached. See [storage limits and path
privacy](data-formats.md).

## Verification

Unit tests use temporary roots and synthetic files, including budgets, ignored
directories, depth limits, permissions, mount boundaries, symlinks, hard links,
sidecars, native paths, cancellation and channel backpressure. A real Turso 0.8.3
fixture is checked byte-for-byte, including sidecars, before and after discovery.
Headless egui tests exercise filtering, native search-cursor retention, arrow-key
and page navigation, scrolling, the Browse button beside the search field,
pointer selection, stable asynchronous result order, access resets and
native-folder-result routing. Engine lifecycle tests
verify successful-open-only history. Classification tests cover shared writer
versions, unknown headers, a real Turso fixture, and a SQLite-like classification
that the user overrides before a successful Turso open. Header inspection and
override are verified to leave fixture files unchanged.

`bash scripts/discovery-smoke.sh` runs normal native startup under Xvfb with an
isolated home, working directory, data and preferences. It verifies project and
extensionless app-data discovery, exclusions, stale-cache pruning, no automatic
opening, no database changes or new sidecars, and the persisted startup opt-out.
The existing desktop and local-project smoke modes deliberately disable discovery
so they never search the invoking user's home. Interactive native-dialog checks
still require a desktop session.
