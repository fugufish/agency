# Agency

A universal, worktree-driven desktop environment for agentic software development.

## Run

Building the Ghostty terminal engine currently requires Zig 0.15.2 to be available on `PATH`.

```sh
cargo run -p agency-desktop
```

This builds the single `agency` executable. With no arguments it opens the
desktop; Agency invokes the same executable with `--mcp` as a session-scoped
stdio connector for Codex and Claude Code. In NORMAL mode:

- `<Space> t` toggles the active terminal
- `<Space> e` toggles the file explorer
- `<Space> s` toggles the sessions activity
- `<Space> m` toggles the MCP activity
- `<Space> d` toggles the diffs activity
- `<Space> n` starts a new session with the default agent

Use `Esc Esc` or `Ctrl-\ Ctrl-N` to leave TERMINAL mode and `i` to return to it.

## Configuration

Agency reads common settings from `$XDG_CONFIG_HOME/agency/config.toml` (or
`~/.config/agency/config.toml`). Set `AGENCY_CONFIG_HOME` to use a different
directory. Global configuration owns user preferences such as keybindings and
mode colors:

```toml
default_agent = "codex" # or "claude"

[keybindings]
leader = " "
toggle_explorer = "e"
toggle_sessions = "s"
toggle_mcp = "m"
toggle_diffs = "d"
toggle_settings = ","
new_session = "n"
toggle_terminal = "t"
toggle_agent_menu = "a"
enter_active_view = "i"

[mode_colors]
normal = "#7a88cf"
terminal = "#9ece6a"
agent = "#7dcfff"
leader = "#e0af68"
visual = "#f7768e"
```

Every field is optional. The existing `AGENCY_MODE_COLOR_*` environment
variables remain supported and take precedence over the file. `visual` was
previously named `escape`; the old key and `AGENCY_MODE_COLOR_ESCAPE` are still
accepted.

Workspace-specific configuration belongs in the workspace's `.agency`
directory. Run `/init` in the agent prompt to create `.agency/config.toml`,
`.agency/config.local.toml`, and an empty `AGENTS.md` when they do not already
exist. It also creates `CLAUDE.md` as a relative symbolic link to `AGENTS.md`
unless that path is already present. Agency then starts the configured default
agent and asks it to merge a concise Agency collaboration and interoperability
preamble into `AGENTS.md`. Workspace settings are not loaded yet.

## Agent sessions

When Agency starts a Codex or Claude Code session, it captures the provider's
session ID and stores it in `.agency/sessions.json`. Those Agency-created
sessions appear in the sidebar and can be resumed from there, including after
restarting Agency. Codex sessions resume through the app-server thread API;
Claude Code sessions resume through its session-ID interface.

The sidebar prefers Codex's native thread name and tracks later name updates.
When a provider has not supplied a name, Agency derives a compact title from
the session's first prompt.

The registry is deliberately workspace-local and ignored by Git. Agency does
not discover or offer sessions started outside the app.

## Plugins

`/plugin install <url>` registers a plugin source with the agents Agency found
on `PATH`. The URL is anything the agents accept as a plugin source: an HTTPS or
SSH Git URL, an `owner/repo` reference, or a local path.

```
/plugin install https://github.com/owner/plugins
/plugin install --agent claude owner/plugins
```

Without `--agent`, Agency installs the source for every configured agent;
`--agent codex` or `--agent claude` limits it to one. Each install runs in a
headless terminal — a real PTY with no terminal pane — and its rendered output
is streamed into the transcript of the session that asked for it, where the
card reports whether the install is running, installed, or failed.

## Agent tools

Every agent launched or resumed by Agency receives a short-lived capability
bound to its Agency conversation and worktree. Agency configures the provider
to start `agency --mcp`, which forwards authenticated calls to the running
desktop application over a local Unix socket.

The initial tools are:

- `list_worktrees`
- `create_worktree`
- `remove_worktree`

The caller does not provide a session ID. Agency resolves the capability to the
conversation ID and records the active Codex or Claude session ID as attribution
metadata. Capabilities are revoked when Agency leaves the owning worktree.

Product direction and architecture decisions live in [`docs`](docs/).
