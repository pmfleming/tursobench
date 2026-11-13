# Assistant contract

The assistant runs on the agent CLI the user already installed and signed in to.
TursoBench starts that vendor's unmodified binary as a child process and serves
it database tools. It has no network, TLS or model-API code of its own.

## Credentials and vendors

TursoBench never reads, stores, forwards or renders a sign-in. It does not open
vendor credential files, offer a login, call ACP `authenticate`, or bundle or
modify a vendor binary. When an agent reports that it is signed out, the panel
names the CLI's own sign-in command for the user to run in a terminal.

| Agent | Command | Protocol |
| --- | --- | --- |
| Claude Code (default) | `claude -p` with stream-json input and output | Claude Code headless |
| Copilot CLI | `copilot --acp` | Agent Client Protocol v1 |
| Codex | `codex app-server` | Codex app-server JSON-RPC |
| Other | Any command line the user enters | Agent Client Protocol v1 |

Claude Code starts with `--restricted --tools ""` (no built-in tools),
`--strict-mcp-config` with only TursoBench's server, `--allowedTools` naming only
TursoBench's tools, `--disable-slash-commands`, `--no-session-persistence` and
`--system-prompt`. It does not use `--bare`, which skips the subscription
sign-in. The working directory of every agent is TursoBench's private runtime
directory, not the database's folder.

Codex starts as `codex app-server` with its shell, unified exec, apps, plugins,
browser, computer use, image tools, helper agents, goals, hooks and web search
switched off. The thread is ephemeral (nothing is saved to Codex's history) and
runs in Codex's `read-only` sandbox with the `untrusted` approval policy.
TursoBench's instructions replace Codex's base instructions, and TursoBench's
server replaces every MCP server in the user's Codex configuration. The relay
and its token travel in `thread/start`, not on the command line. Every command,
file-change, permission and elicitation request is declined with a visible
notice. Other requests, including `account/chatgptAuthTokens/refresh`, are
answered with "method not found". TursoBench never calls Codex's login, logout,
account or token methods. Codex still offers its `apply_patch` tool and MCP
resource listing; patches cannot write under the read-only sandbox and their
approval requests are declined.

The ACP client advertises no file-system or terminal capability and answers such
requests with "method not found". It approves permission requests only for
TursoBench's own tools and declines every other request with a visible notice.
An ACP agent keeps whatever built-in tools its vendor gives it; some may read
local files without asking. The consent card says so.

## Consent and sharing

Nothing is sent until the user chooses, per database (by absolute path), either:

- **Schema only**: object names, columns, declared types, keys, indexes and DDL.
- **Schema and query results**: also read-only queries and their results.

The choice persists in application preferences, at most 50 databases. The
header toggles it and the change applies to a running agent at once; the next
prompt tells the agent. Settings → Assistant lists every choice and can forget
one, which also ends that database's conversation. Only the database's file name,
not its path, appears in the agent's instructions.

## Tools

The tool server speaks MCP (newline-delimited JSON-RPC: `initialize`, `ping`,
`tools/list`, `tools/call`).

| Tool | Behaviour |
| --- | --- |
| `list_tables` | Tables and views, excluding `sqlite_*` and `__turso_internal_*`; at most 2,000 |
| `describe_table` | Columns, primary-key order, defaults, indexes, foreign keys and DDL up to 16 KiB |
| `run_query` | Refused unless results are shared; otherwise one read, at most 200 rows / 256 KiB |
| `propose_sql` | Opens the SQL, up to 256 KiB, in a new SQL editor tile without running it |

Every tool is annotated `readOnlyHint`, so agents that honour it (Codex) call
them without asking for approval.

Queries pass two independent guards. A parser gate accepts exactly one `SELECT`
(including `WITH … SELECT`), `EXPLAIN SELECT` or `EXPLAIN QUERY PLAN SELECT`,
and applies the coordinated-session rules. The worker then runs it on a dedicated
connection opened with `PRAGMA query_only = 1`, which Turso enforces. That
connection is separate from the editor and browse connections. It sees committed
data, never joins the editor's manual transaction, and uses the configured
statement deadline. Text cells longer than 2,000 characters are shortened, blobs
report only their size, and deferred values are never read.

Tool replies travel over a per-call channel to the calling thread, not the UI
event stream. Request IDs start at 2^62 so they cannot collide with the UI's.
They count against the session's 16 MiB queue budget. A call waits at most
60 seconds, then interrupts its read and reports a timeout. Calls fail once the
session is closing.

## Relay and process lifecycle

The agent starts `tursobench --mcp-relay <socket>` as its MCP server. The relay
copies bytes between its stdio and a Unix socket owned by the running app, so
tools reach the app's existing session and no second process opens the database.
The socket (mode 0600) and a Claude `--mcp-config` file (0600) live in
`$XDG_RUNTIME_DIR/tursobench`, or a per-user temporary directory, which must be a
0700 directory owned by the user. Each conversation's endpoint has a random
192-bit token, passed to the relay in its environment and compared in constant
time before any MCP message is served.

One agent process runs per workspace conversation, started on the first message.
Stop sends Claude's interrupt control request, Codex's `turn/interrupt` or ACP
`session/cancel`. New
conversation, forgetting the database's sharing choice, or opening a different
database in the workspace kills the agent and removes its socket and config
file. Reopening the same database keeps the conversation.

## Limits

- The agent's vendor receives whatever the user shares and the agent reads. The
  vendor's terms and data handling apply.
- Hostile text stored in a database can steer the agent. With Claude Code it can
  then only call TursoBench's read-only tools or propose SQL that the user must
  run themselves. Codex keeps `apply_patch` inside its read-only sandbox; ACP
  agents keep their own vendor tools.
- Conversations are not saved; they end with the agent process. Up to 400
  entries are shown.
- Copilot CLI and other ACP agents are covered by scripted protocol tests only.
  Claude Code and Codex are also covered by opt-in live tests that use the
  signed-in CLI: `cargo test -p tursobench-ui --test live_claude -- --ignored`
  and `--test live_codex`.
