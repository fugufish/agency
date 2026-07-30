# Headless Terminal Multiplexer

## Role

`agency-mux` owns terminal processes and PTYs independently of the desktop UI. It is responsible for:

- starting shells and explicit terminal commands in a workspace directory
- maintaining multiple concurrent sessions
- writing input and resizing or terminating sessions
- broadcasting raw process output to attached clients
- retaining session identity, provider, working directory, and status

The desktop client attaches to a session, feeds its raw output into `libghostty-vt`, and renders the resulting screen. The multiplexer has no dependency on Iced or Ghostty.

## Current boundary

The initial multiplexer is a headless Rust library hosted inside the Agency process. Sessions remain alive while Agency is running and can have multiple attachments, but they do not yet survive a Agency process restart.

The next persistence step is a `agency-muxd` process with a local authenticated transport. The library API should remain the semantic contract across that boundary.

## Agent boundary

Agents do not run as terminal sessions. Rich adapters use:

- Codex app-server for threads, turns, items, approvals, and streamed events
- Claude Code stream-json for structured input and output

The `agency-agents` crate translates both protocols into one provider-neutral event model consumed by Agency's native interface. The terminal multiplexer remains responsible only for user-facing terminals and terminal attachments.

## Keyboard entry points

- `<leader> t`: toggle the active terminal
- `<leader> c`: open a native Codex session
- `<leader> a`: open a native Claude Code session

Agent sessions are retained by the agent runtime, not the terminal multiplexer.
