# Transcript Events

## Role

Agency shows one transcript, whichever agent is working. Codex and Claude Code
describe the same work in different protocols, so each translator normalizes
what every agent does into one canonical vocabulary, and the transcript renders
that vocabulary with a single set of handlers. The same command, the same edit,
and the same read look identical no matter which agent performed them.

The vocabulary lives in `agency_translator_api::tools`. It is the contract
between translators and any surface that renders a conversation.

## Canonical tool calls

A `ToolCall` payload carries a canonical `input` whenever the call is work every
agent performs:

| Kind | Input | Rendered as |
| --- | --- | --- |
| `commandExecution` | `command`, `status`, `aggregatedOutput`, `exitCode` | Command card with its output |
| `fileChange` | `status`, `changes[{path, kind, diff}]` | File change card, and diff activity |
| `fileRead` | `path`, `status`, `lines` | Read card |

`status` is `inProgress` until the work finishes, then `completed`, `failed`, or
a provider's own terminal status such as `declined`. `diff` holds unified diff
hunks without file headers; the transcript adds those from `path`. `kind` is
`add`, `update`, or `delete` — Codex spells it `{"type":"update"}` and Claude
Code spells it `"update"`, so read it with `tools::change_kind`.

Tool calls outside this vocabulary — searches, plans, MCP tools — keep their own
name and input and render generically. Normalizing them would claim a shared
meaning they do not have.

## Reporting work once

Agents report long-running work twice: when it starts and when it finishes.
`ConversationUpdate::Append` therefore adds an event or replaces the event that
already carries the same id, so the finished report takes the place of the one
that started it instead of appearing beside it.

Which report a translator emits depends on when the work becomes describable:

- A command is reported when it starts, because its command line is already
  known, and again when it finishes with its output and exit code.
- A file change is only reported once it finishes, because only then is the
  patch that was actually applied known.
- A read is reported when it starts and completed with its line count.

## Provider notes

Codex sends app-server items that already match the canonical shapes; the
translator forwards them and applies the reporting rules above.

Claude Code streams a turn in pieces. Text arrives as deltas, keyed by message
and content block so each block is its own transcript entry. A tool's arguments
arrive empty in the stream and whole in the assistant message that follows, so
tool calls are read from that message. A tool's outcome — a command's output, an
edit's `structuredPatch` — arrives in a later user message, which completes the
call it belongs to. The translator holds the state of the turn in flight to
connect the three, so one translator instance serves one session.
