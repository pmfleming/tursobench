# Assistant contract

The assistant runs on the agent CLI the user already installed and signed in to.
TursoBench starts that vendor's unmodified binary as a child process and serves
it database tools. It has no network, TLS or model-API code of its own.

## Credentials and vendors

TursoBench never opens credential files or supplies, stores or refreshes tokens.
Settings can discover local CLIs and ask for sign-in state, method and plan;
email is discarded and these fields stay in memory. Checks use separate bounded
processes with no database tools or prompts. ACP checks create an empty session,
close it when supported and may leave it in the vendor's history.

Sign in uses Codex’s managed browser flow (with cancellation), an advertised ACP
OAuth method, or Claude Code’s terminal login. A copyable terminal command is
available if a browser or terminal cannot be launched. Sign out is available for
Claude Code and Codex; ACP sign-out stays in the agent’s own CLI. Every
conversation runs the selected vendor, so a sign-in or sign-out of that vendor
ends all workspace conversations before changing the sign-in, and chat sends
stay disabled until the job finishes or is cancelled. Account jobs for other
vendors leave the chat running. Account jobs have no database tools and kill
their process group on cancellation or timeout; a terminal opened for sign-in
is the user’s window and is left open when the terminal process exits.

| Agent | Command | Protocol |
| --- | --- | --- |
| Claude Code (default) | `claude -p` with stream-json input and output | Claude Code headless |
| Copilot CLI | `copilot --acp` | Agent Client Protocol v1 |
| Antigravity | `agy acp` | Agent Client Protocol v1 |
| Codex | `codex app-server` | Codex app-server JSON-RPC |
| Other | Any command line the user enters | Agent Client Protocol v1 |

Claude Code starts with `--restricted --tools ""` (no built-in tools),
`--strict-mcp-config` with only TursoBench's server, `--allowedTools` naming only
TursoBench's tools, `--disable-slash-commands` and `--system-prompt`; history and
telemetry follow the Privacy choices below. It does not use `--bare`, which skips
the subscription sign-in. The working directory of every agent is TursoBench's private runtime
directory, not the database's folder.

Codex starts as `codex app-server` with its shell, unified exec, apps, plugins,
browser, computer use, image tools, helper agents, goals, hooks, shell snapshots
and web search switched off. The thread runs in Codex's `read-only` sandbox with
the `untrusted` approval policy; whether it is saved follows the Privacy choices.
TursoBench's instructions replace Codex's base instructions, and TursoBench's
server replaces every MCP server in the user's Codex configuration. The relay
and its token travel in `thread/start`, not on the command line. Every command,
file-change, permission and elicitation request is declined with a visible
notice. Other requests, including `account/chatgptAuthTokens/refresh`, are
answered with "method not found". The conversation driver never calls account
or token methods; Settings uses a separate account process. Codex still offers its `apply_patch` tool and MCP
resource listing; patches cannot write under the read-only sandbox and their
approval requests are declined.

The ACP client advertises no file-system or terminal capability and answers such
requests with "method not found". It approves permission requests only for
TursoBench's own tools and declines every other request with a visible notice.
Antigravity explicitly selects its default permission mode before the first
prompt; model changes are acknowledged before prompts are sent. It starts with
`--disable-telemetry` when requested; history follows its own settings. Some
Antigravity versions omit tool output from ACP updates; the UI shows the
completion state without inventing a result.

An ACP agent keeps whatever built-in tools its vendor gives it; some may read
local files without asking. The notice above an empty conversation says so; it
names who receives the data and shows the first time each agent is used, then
again after every four sessions that use it without it.

## Reasoning level

The chat's reasoning picker offers the agent's default and the levels it accepts.
Each provider keeps its own choice. It applies from the next message of every
running conversation; nothing restarts. Choosing the default again restores the
level the agent started with.

| Agent | Levels | How it is set |
| --- | --- | --- |
| Claude Code | `low`, `medium`, `high`, `xhigh`, `max` (`--effort`); the CLI caps them at the model's maximum | `apply_flag_settings` control request with `effortLevel`. A `get_settings` request at start records the level in use; only its applied effort is read |
| Codex | Each model's `supportedReasoningEfforts` from `model/list` | `effort` on every `turn/start`; the thread's starting `reasoningEffort` is sent to undo a choice |
| ACP agents | The session's `thought_level` config option, when offered | `session/set_config_option` before the first prompt, then on each change |

A remembered level the current model does not list is not sent; the agent uses
its own.

## Sharing

Each database shares one of two levels with the agent. Settings → Assistant →
Sharing sets the default, **Schema and query results** unless changed:

- **Schema only**: object names, columns, declared types, keys, indexes and DDL.
- **Schema and query results**: also read-only queries and their results.

Two icons in the chat panel header switch the open database between them, per
database (by absolute path) and provider; the choice persists in application
preferences, at most 50 databases. Legacy choices apply only to the previously
selected provider. Switching provider ends all workspace conversations; changing
a custom command or executable also forgets that custom agent's choices. A change
to a choice or to the default applies to a running agent at once, and the next
prompt tells the agent. Settings → Assistant lists every per-database choice and
can forget one, which returns that database to the default.

## Privacy

Settings → Assistant edits any agent's settings, privacy included, in one area
below the agent list; its "Settings for" choice is independent of the agent the
chat uses. Each provider
keeps its own choices, and every default is the private one. They apply to the
next conversation.

| Choice | Claude Code | Codex | Copilot CLI, other ACP |
| --- | --- | --- | --- |
| Block telemetry and error reports (on) | `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `DISABLE_TELEMETRY`, `DISABLE_ERROR_REPORTING` | `analytics.enabled=false`, `otel.exporter`, `otel.metrics_exporter` and `otel.trace_exporter` = `none`, `feedback.enabled=false` | Follows the agent's own settings |
| Save conversations in the CLI's history (off) | Off: `--no-session-persistence` | Off: ephemeral thread, `history.persistence="none"` | Follows the agent's own settings |
| Tell the agent the database's file name (on) | Off: the instructions say only that a database is open | Same | Same |

The Codex overrides are `-c` flags for that process only; `config.toml` is not
changed. With the defaults, a Codex conversation writes only Codex's model cache
to `~/.codex` (its shell snapshot feature is also off), and Claude Code writes no
session transcript.

These hold for every provider, so the settings area does not repeat them:

- Credentials stay with the CLI. Only sign-in status, method and plan are
  retained in memory; email is discarded. No credential files are opened and
  token refresh requests are refused.
- The database's folder is never sent. Queries that name
  `pragma_database_list`, which returns the full path, are refused.
- Row values are sent only when query results are shared; schema only sends
  none.
- The agent's own tools: none for Claude Code; for Codex no shell or web search
  in a read-only sandbox; ACP agents keep theirs (see below).

The notice above an empty conversation links to the section.

## Tools

The tool server speaks MCP (newline-delimited JSON-RPC: `initialize`, `ping`,
`tools/list`, `tools/call`).

| Tool | Behaviour |
| --- | --- |
| `list_tables` | Tables and views, excluding `sqlite_*` and `__turso_internal_*`; at most 2,000 |
| `describe_table` | Columns, primary-key order, defaults, indexes, foreign keys and DDL up to 16 KiB |
| `run_query` | Refused unless results are shared; otherwise one read, at most 200 rows / 256 KiB; never `pragma_database_list` |
| `propose_sql` | Opens the SQL, up to 256 KiB, in a new SQL editor tile without running it |

Every tool is annotated `readOnlyHint`, so agents that honour it (Codex) call
them without asking for approval.

Queries pass two independent guards. A parser gate accepts exactly one `SELECT`
(including `WITH … SELECT`), `EXPLAIN SELECT` or `EXPLAIN QUERY PLAN SELECT`,
and applies the coordinated-session rules. Referenced view definitions are checked
transitively for location access in the same read snapshot as the query. This
check is conservative: ambiguous references, including matching string literals,
can be refused. The worker then runs it on a dedicated
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

The chat panel can expand to fill the workspace in place of the tiles, as a tile
does; this changes only layout, not the conversation or its agent.

One agent process runs per workspace conversation, started on the first message.
Stop sends Claude's interrupt control request, Codex's `turn/interrupt` or ACP
`session/cancel`. New
conversation or opening a different
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
- Copilot CLI and custom ACP agents are covered by scripted protocol tests only.
  Claude Code, Codex and Antigravity have opt-in live relay tests:
  `cargo test -p tursobench-ui --test live_claude -- --ignored`,
  `--test live_codex` and `--test live_antigravity`. Status-only live checks use
  `cargo test -p tursobench-assistant --test live_accounts -- --ignored`.
  Browser/terminal sign-in completion is covered by scripted peers; completing
  a real vendor sign-in still requires a person.
