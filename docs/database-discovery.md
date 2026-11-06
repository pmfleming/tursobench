# Local database discovery

TursoBench starts one background search when the application launches. The sidebar’s
**Available** section immediately shows remembered files, then appends new candidates
without changing the selected path or access mode. Recent successful opens appear
first on the next launch. Files that are already open appear under **Open databases**.

**Ctrl+P** or `/` focuses the sidebar's **Search** section. Its three scope
icons have descriptive tooltips and highlight the current selection. Choose
**Databases** to match filenames and paths, **All open** to search open sessions and matching available files, or
**This database** to filter the current session’s schema tree and matching columns.
Up/Down chooses results, PageUp/PageDown moves through longer lists, and Enter selects.
Typing or pasting retains the search editor’s cursor. Escape clears the query.
Selecting an open database focuses its existing session. Selecting an available file
starts access review. All open shows up to six object matches per database; a count identifies omitted matches.
The overall search is capped at 100 results; refine the query for omitted results.

The folder button in **Open database** or **Ctrl+O** opens the native file dialog. Manual entry is available
under **Enter a path…** when no file is selected. Choose a different file with the picker
above the list; the selected-file card reviews access and opens that file. Selecting a
new path defaults to read-only and resets the SQLite override. Opening a file from the
sidebar preserves other open database sessions. The slider in Settings → Resources sets
the retained-database count and matching memory allowance; the sidebar shows the limit.
Cancelling access review or dismissing the picker restores the previous database.
Available is collapsible, renders visible rows, and uses file identities cached by
discovery/opening. Repainting never probes candidate files.

Selecting a candidate does not connect to it. A compact access panel directly
beneath the selected row shows the selected file, path, lock/pencil access choice and
**Open**. A non-writing background check identifies missing/invalid files and
incompatible locks before enabling Open. Known blockers appear inline, including
the locking application when available. Blocked files are rechecked periodically;
**Check again** in the error panel reruns file and lock checks immediately. This action
is hidden for files with no blocking check failure. Failures from the actual engine open
stay visible with **Retry** until another attempt or selection/access change.
Admission locks are still checked authoritatively on Open. Long paths are truncated with the full path
on hover. Changing the selected path resets write access and
the SQLite classification override.
Discovery never replaces a command-line path.

## Scope and controls

Search prioritizes added folders and recent-file parents, then `$XDG_DATA_HOME`
(normally `~/.local/share`) and `$XDG_STATE_HOME` (normally `~/.local/state`),
before the launch directory, files directly in the home directory, and common
development folders (`Projects`, `projects`, `Code`, `code`, `Developer`,
`Development`, `src`). The launch and development folders have a five-level
recursion limit, recent parents one, application data/state three,
and home zero. Launching from home never makes all of home recursive.

All discovery preferences live in **Settings → Discovery**. Open the Settings
button at the bottom of the navigator, use **Ctrl+,**, or choose Settings in the
command palette. **At startup** toggles automatic discovery. The refresh icon
reruns the configured search and becomes Stop while searching. The same
Refresh/Stop actions appear beside **Available**; Available has no Settings link
or limit indicator. Icon buttons have tooltips and accessible names.

**Search folders** lists the effective search locations: default application,
home/development and launch folders, recent-file parent folders, and folders you
add. Duplicate paths appear once, preserving their search depth and explicit
opt-in rules. Excluded roots are omitted. The list scrolls independently.

The **Search folders** and **Excluded** tabs show counts and share one path input
and a list that fills the page. Browse on either tab fills the input; press **Enter**
or **+** to add the selected absolute path. Search folders saves and searches the
folder; Excluded adds an exclusion. Picker results return to the tab that opened
the picker, even if the tab changed while the picker was open. The **×** on each folder
row removes that search location; it never deletes files. Removed defaults and
recent-parent roots stay removed across refresh/restart; add a path again to
restore it. Up to 128 root removals are remembered. Other configured parent roots
can still search their children; use **Excluded** to prevent every search under a
path. Cached/recent files remain remembered and can still be checked directly.
Refresh after removing an
exclusion. Long paths show in full on hover. The fixed footer shows header,
folder, unavailable and skipped counts with labels on hover, plus search feedback.
It stays visible while folders scroll. **Limit reached** appears only here and
means only part of the configured locations was searched. Add a narrower location
on the Search folders tab to search it directly.
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
switches engines nor converts files. Access mode and coordinated admission checks
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
and page navigation, scrolling, the Browse button in the **Open database** section,
pointer selection, stable asynchronous result order, access resets and
native-folder-result routing. Engine lifecycle tests
verify successful-open-only history. Classification tests cover shared writer
versions, unknown headers, a real Turso fixture, and a SQLite-like classification
that the user overrides before a successful Turso open. Header inspection and
override are verified to leave fixture files unchanged.

`bash scripts/discovery-smoke.sh` runs normal native Wayland startup under headless Weston with an
isolated home, working directory, data and preferences. It verifies project and
extensionless app-data discovery, exclusions, stale-cache pruning, no automatic
opening, no database changes or new sidecars, and the persisted startup opt-out.
The existing desktop and local-project smoke modes deliberately disable discovery
so they never search the invoking user's home. Interactive native-dialog checks
still require a desktop session.
