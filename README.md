# Agency

A universal, worktree-driven desktop environment for agentic software development.

## Run

Building the Ghostty terminal engine currently requires Zig 0.15.2 to be available on `PATH`.

```sh
cargo run -p agency-desktop
```

The bootstrap opens the initial Agency application chrome. In NORMAL mode:

- `<Space> t` toggles the active terminal
- `<Space> e` opens the file explorer
- `<Space> a` opens the sessions tool
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
show_explorer = "e"
show_sessions = "s"
new_session = "n"
toggle_terminal = "t"
enter_active_view = "i"

[mode_colors]
normal = "#7a88cf"
terminal = "#9ece6a"
agent = "#7dcfff"
leader = "#e0af68"
escape = "#f7768e"
```

Every field is optional. The existing `AGENCY_MODE_COLOR_*` environment
variables remain supported and take precedence over the file.

Workspace-specific configuration belongs in the workspace's `.agency`
directory. That directory is currently a placeholder; no workspace settings
are loaded yet.

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

Product direction and architecture decisions live in [`docs`](docs/).
