# Modes and Keybindings

## Principle

Every Agency action must be available from the keyboard and represented as a named command. Mouse and touch interactions may invoke the same commands, but must not define separate capabilities.

Keybindings map input sequences to commands. Product code should depend on commands rather than hard-coded keys so Agency can support multiple binding schemes without duplicating workflows.

Leader initiation is mode-aware so Space remains text or terminal input in
insert-like modes. Once Agency enters LEADER, however, the suffix is resolved
globally before the active view receives it.

## Initial modes

### NORMAL

NORMAL is Agency's default navigation and command mode. Printable keys are interpreted as Agency commands rather than text input.

- `<Space>` begins a leader sequence.
- `<leader> t` toggles the terminal for the current workspace. Reopening it resumes the same shell session.
- `<leader> e` opens the file Explorer.
- `<leader> s` opens the sessions sidebar.
- With the sessions toolbar open, `j`/`k` select the next/previous session,
  `g`/`G` jump to the first/last session, and `Enter` resumes the selected session.
- `<leader> n` starts a new session using the configured default agent.
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
