# Modes and Keybindings

## Principle

Every Agency action must be available from the keyboard and represented as a named command. Mouse and touch interactions may invoke the same commands, but must not define separate capabilities.

Keybindings map input sequences to commands. Product code should depend on commands rather than hard-coded keys so Agency can support multiple binding schemes without duplicating workflows.

Dispatch is resolved by mode first and the focused element's semantic context
second. Focusable elements attach a stable focus ID to a context through the
shared focus tracker. Window IDs are numeric and ordered left to right; every
window owns its visibility state, and hidden windows are skipped during focus
traversal. Contexts are open string IDs, not framework enums. Any built-in,
plugin, or future surface can register a context, an arbitrary set of local
submodes, and declarative bindings without modifying the focus/keymap harness.

- `<C-w> w` focuses the next visible window to the right.
- `<C-S-w> w` focuses the previous visible window to the left.

Leader initiation is mode-aware so Space remains text or terminal input in
insert-like modes. Once Agency enters LEADER, however, the suffix is resolved
globally before the active view receives it.

Modes are application-global while focus is not, so the two must move together.
Bindings that enter an insert-like mode resolve to a command (`composer.enter`,
`terminal.enter`) rather than setting the mode directly; the application focuses
the owning element first and only then enters the mode. If focus and mode ever
disagree — the focused element cannot bind keys in the active mode — the mode
falls back to NORMAL instead of silently swallowing every key.

## Initial modes

### NORMAL

NORMAL is Agency's default navigation and command mode. Printable keys are interpreted as Agency commands rather than text input.

- `<Space>` begins a leader sequence.
- `<leader> t` toggles the terminal for the current workspace. Reopening it resumes the same shell session.
- `<leader> e` opens the file Explorer.
- `<leader> s` opens the sessions sidebar.
- `<leader> d` opens the selected file read-only in a 50/50 right split.
  Markdown files default to a rendered GitHub-flavored preview with inline
  Mermaid diagrams; `Tab` switches between rendered and raw views.
- `<leader> f` opens the current session's diffs.
- `<leader> ,` opens the application settings.
- With the sessions toolbar open, `j`/`k` select the next/previous session,
  `g`/`G` jump to the first/last session, and `Enter` resumes the selected session.
- `<leader> n` starts a new session using the configured default agent and
  focuses its composer.
- `<leader> a` pops the agent menu, a floating switcher anchored to the status
  bar's agent chip. It lists the configured agents, opens on the currently
  selected one, and owns input while it is up: `j`/`k` (or the arrow keys)
  select the next/previous agent, `g`/`G` jump to the first/last, `Enter`
  switches to the selected agent, and `Esc` closes without switching. Repeating
  `<leader> a` closes it too. Closing the menu returns focus to the surface it
  floated over. Clicking the agent chip is the equivalent pointer control.
  Switching rebinds the current Agency session to the chosen agent rather than
  jumping to another session that happens to be running under it. The Agency
  conversation, its session directory, and its diffs stay put; only the agent
  behind them changes. A conversation keeps a separate binding per agent, so an
  agent it has already run under resumes with its own history, and one it has
  not starts clean.
- `Ctrl+.` cycles the selected agent between Codex and Claude Code. With an
  active, completed session it translates the canonical conversation and
  continues in the selected client; otherwise it only changes the preference.
  The selected agent is shown in the status bar.
- `i` returns to agent input from NORMAL mode.
- `i` enters TERMINAL mode when a terminal is open.

### LEADER

LEADER is a transient state while Agency waits for the next key in a leader sequence. It is shown in the status bar so sequences remain discoverable.

The initial leader is `<Space>`.

### TERMINAL

TERMINAL sends keyboard input to the active terminal session.

- `Esc` is sent to the terminal after a short delay.
- `Esc Esc` enters NORMAL mode without sending either Escape.
- `Ctrl-\ Ctrl-N` returns to NORMAL mode.

The Escape delay is 150 milliseconds. The status bar shows `ESC…` during this pending window. If another key besides Escape arrives, Agency sends the pending Escape followed by that key so terminal key sequences retain their ordering.

`Ctrl-\ Ctrl-N` remains the immediate, zero-delay path out of the terminal.

### COMPOSER

COMPOSER mode edits the prompt in Agency's native agent interface. The interface is provider-neutral: Codex and Claude Code messages, activity, status, and errors are normalized before rendering.

- COMPOSER starts in its own INSERT submode.
- The first `Esc` enters COMPOSER NORMAL; a second `Esc` enters application
  NORMAL.
- In COMPOSER NORMAL, `h`/`l` move the prompt cursor, `i` returns to INSERT,
  `v` enters VISUAL, and `p` pastes.
- In COMPOSER VISUAL, `h`/`l` extend the selection and `y` copies it.
- `Enter` submits the prompt.
- `Backspace` edits it.
- `Ctrl-V` (or `Cmd-V` on macOS) pastes clipboard text or attaches a clipboard image.
- When the agent asks a multiple-choice or permission question, `1`–`9` selects the
  corresponding answer. Multi-part questions advance after each selection.
- `Esc` enters NORMAL mode.
- `i` from NORMAL returns to COMPOSER mode.

## Binding notation

Documentation uses familiar Vim notation:

- `<leader> t`
- `<C-\> <C-n>`
- `<Esc>`
- `<S-Tab>`

This notation is a presentation format, not the internal representation. Internally, bindings should use normalized physical/logical keys, modifiers, mode, context, and a command identifier.

## Command model

A future binding entry should describe:

```text
mode      = normal
sequence  = <leader> t
command   = terminal.open
when      = workspace.available
```

Commands should provide:

- a stable identifier
- a title and optional description
- whether they are currently available
- their active bindings
- an execution handler

This enables keyboard remapping, a command palette, contextual help, menus, and accessibility actions to share one source of truth.

## Terminal prototype

The current prototype:

- starts the platform's default shell in a cross-platform PTY
- sets its working directory to Agency's current directory
- parses terminal output with Ghostty's `libghostty-vt`
- renders a plain text snapshot through Iced
- forwards text, control keys, Enter, Backspace, Tab, Escape, and arrow keys
- supports layered `Esc Esc` mode switching with a 150 millisecond timeout

It does not yet provide color and style rendering, a visible cursor, selection, mouse reporting, dynamic PTY resizing, multiple terminals, or persisted configurable bindings.
