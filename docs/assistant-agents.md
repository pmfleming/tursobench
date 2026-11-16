# Assistant agent integration

Implementation record, 2026-11-15. Each stage is committed independently:

1. Verify vendor interfaces and decide account boundaries.
2. Move the collapsible assistant to the right.
3. Centralize providers, discover local binaries, and scope models and sharing choices.
4. Add Settings discovery/status and vendor-owned sign-in flows.
5. Add agent and model pickers to the conversation.
6. Complete regression coverage, native captures and user documentation.

## Account boundary

Settings may ask a CLI for sign-in status, method and plan and start its own
sign-in flow. These fields stay in memory; email is discarded. TursoBench never
opens credential files, accepts passwords/API keys, or supplies or refreshes
tokens. Account operations use separate, bounded child processes without
database tools. Conversation drivers continue to reject credential requests.

Changing provider ends conversations in every workspace; sharing choices belong to
the database and provider, falling back to the default in settings. Legacy
choices migrate only to the previously selected provider. Changing a custom ACP
command forgets its choices.

## Interfaces checked

Installed help checked against Claude Code 2.1.325, Codex 0.169.0 and Antigravity
1.5.9. Browser sign-in completion requires a human and is not exercised against
the user's real accounts during development.

| Provider | Status | Sign in | Models |
| --- | --- | --- | --- |
| Claude Code | `claude auth status` JSON, exits 0/1 | `claude auth login` in a terminal, copyable command fallback | Stable aliases plus a custom model field |
| Codex | `account/read` with `refreshToken: false` | `account/login/start` with `type: chatgpt`, open `authUrl`, await `account/login/completed`; cancel and logout via app-server | `model/list`, including pagination |
| Antigravity | ACP initialize then empty `session/new`; authentication-required is -32000 | Advertised browser OAuth method through ACP `authenticate`; terminal fallback `agy acp --login` | Session config option with category `model` |
| Copilot / custom ACP | Same ACP probe, distinguish authentication errors from other failures | Advertised OAuth methods; terminal fallback when unavailable | ACP model config options or availableModels |

Antigravity supports `session/close`; closing a probe is resource cleanup, not a
promise to delete vendor history. An empty session may appear in CLI history.
No probe sends a prompt or database metadata. Antigravity conversations explicitly
select the `default` permission mode and use `--disable-telemetry` when requested.
History otherwise follows that CLI's settings.

Discovery searches PATH and common user/system installation folders without
launching a shell, reading startup files, or recursively searching the disk.
Scans run off the UI thread on opening Assistant settings and on explicit retry;
sign-in completion triggers a fresh status check. Errors remain distinct from
signed-out status. Missing agents retain install/help links.

Local read-only probes confirmed Codex account status and eight models, and
Antigravity's four advertised authentication methods, model/mode config options
and successful `session/close`. No login or logout was performed.

## Implementation validation

All seven stages are implemented. Native Wayland captures show the right-side
panel and settings using synthetic data and seeded account states. The installed
Claude, Codex and Antigravity status probes pass. Antigravity's live relay test
also passes against a synthetic tool with no real database. Its current ACP
messages use exact `mcp_tursobench_` tool names and omit tool-result bodies;
the adapter handles both without broadening approvals beyond known tools.

Scripted peers cover Codex login completion/cancellation/logout, ACP advertised
method selection, token-request rejection, grouped model options and waiting for
permission-mode/model acknowledgements before sending prompts. Account changes
to the selected vendor stop workspace conversations and block chat sends until
they finish; changes to other vendors leave the chat running.

Browser/terminal sign-in completion with real accounts remains a manual check.
Copilot is unavailable here and has scripted coverage. No real account was
signed out or changed during validation.

Validation passed in the pinned Nix shell: `cargo fmt --all --check`, workspace
Clippy with warnings denied, workspace tests, workspace build, `engine-spike`,
desktop/discovery/local-project smoke scripts and all seven assistant captures.

## Primary references

- [Codex app-server](https://developers.openai.com/codex/app-server.md)
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference)
- [Antigravity ACP](https://antigravity.google/docs/cli/acp/)
- [Antigravity editor integration and authentication](https://antigravity.google/docs/ide/extensions/acp/)

Use scripted protocol peers to verify login, cancellation, timeouts, unknown
methods and token-request rejection without modifying real accounts. Live tests
remain explicit opt-ins.
