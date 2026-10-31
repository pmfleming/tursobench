# Local database discovery

TursoBench starts one background search when the application launches. The Open
database screen immediately shows remembered files, then appends new candidates
without changing the selected path, access mode, keyboard focus or scroll
position. Recent successful opens appear first on the next launch. **Ctrl+O**
returns to this chooser and focuses its name/path filter. **Browse for another
file…** opens the native file dialog; manual path entry remains available.

Selecting a candidate does not connect to it. The existing access review and
exclusive-use acknowledgement still precede Open. Changing the selected path
resets write access and acknowledgement. Command-line paths keep priority.

## Scope and controls

Search prioritizes added folders and recent-file parents, then the launch
directory, files directly in the home directory, common development folders
(`Projects`, `projects`, `Code`, `code`, `Developer`, `Development`, `src`), and
`$XDG_DATA_HOME` (normally `~/.local/share`). The launch and development folders
have a five-level recursion limit, recent parents one, application data three,
and home zero. Launching from home never makes all of home recursive.

**Search folders and exclusions** in the chooser, also available in Settings,
provides a startup toggle, folder picker, typed folder entry and exclusions.
**Save and search this folder** saves an absolute folder and searches that folder
immediately, including extensionless files, to five levels. This manual search
also works when startup discovery is disabled. Use a narrower folder to look
past an automatic depth or resource limit. **Refresh** reruns the configured
search. **Stop** requests cancellation; another worker cannot start until the
current one finishes. File browsing remains available during a search.

Automatic discovery skips system folders, network/FUSE/unknown filesystems,
automounts, standard removable-media locations and devices marked removable by
Linux. Browser profiles, dependencies, build output, caches, trash, VCS internals
and most hidden directories are excluded. Hidden `.data`, `.turso`, `.libsql` and
`.local` directories within searched roots remain eligible. `.gitignore` is not
consulted: real development databases are often intentionally ignored.

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

The SQLite-format header is shared by Turso and SQLite. A matching header is only
a **candidate**, not proof of the creating engine, integrity or compatibility with
the pinned Turso version. Engine validation happens on Open. Encrypted files,
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
Headless egui tests exercise filtering, focus, pointer selection, stable result
order, access resets and native-folder-result routing. Engine lifecycle tests
verify successful-open-only history.

`bash scripts/discovery-smoke.sh` runs normal native startup under Xvfb with an
isolated home, working directory, data and preferences. It verifies project and
extensionless app-data discovery, exclusions, stale-cache pruning, no automatic
opening, no database changes or new sidecars, and the persisted startup opt-out.
The existing desktop and local-project smoke modes deliberately disable discovery
so they never search the invoking user's home. Interactive native-dialog checks
still require a desktop session.
