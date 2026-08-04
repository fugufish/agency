# Agency contributor guidance

## Agency harness

Agency is an orchestrator for agentic application development that connects
agents such as Codex and Claude Code to work cooperatively on the user's
application. Keep instructions and work products provider-neutral and
interoperable. Use collaboration tools to delegate independent work in
parallel, exchange findings, and coordinate ownership. Agents share a
workspace: avoid overlapping edits, preserve user and agent changes, and
verify the combined result. Agency supplies session-scoped tools and identity
automatically; use available Agency tools for cross-agent coordination and
worktree operations, and never ask the user for an Agency session ID.
Worktrees may isolate concurrent tasks, while repository instructions and the
current worktree state remain authoritative. Follow the closest `AGENTS.md`,
report blockers clearly, and do not overwrite unrelated work.

## Work in a worktree

- Start every new feature, task, or bugfix by creating a worktree for it and
  doing the work there. Do not commit new work directly in the primary
  checkout.
- Creating the worktree is a pre-condition of the `superpowers:brainstorming`
  skill. When that skill is invoked, create the feature's worktree and its
  branch first, enter it, and only then begin the brainstorming dialogue, so
  the design conversation, any notes or specs it produces, and the
  implementation that follows all live on the same branch.
- Derive the worktree and branch name from the idea the user brought, before
  the design is settled; rename the branch later if the brainstorm reshapes
  the work. Do not delay the worktree until the design is agreed.
- Create and manage worktrees with Agency's worktree tools, which are
  session-scoped; never ask the user for an Agency session ID and never fall
  back to raw `git worktree` commands when a tool covers the operation.
- Name the worktree and its branch after the work it holds, so concurrent
  agents can tell one task's worktree from another's.
- One task per worktree. If a request turns out to cover unrelated work, split
  it across worktrees rather than mixing changes that must land separately.
- Before starting, list the existing worktrees and reuse the one that already
  belongs to this task instead of creating a duplicate.
- Repository instructions and the current worktree state remain authoritative
  once you are inside a worktree; follow the closest `AGENTS.md` there.
- Leave the worktree in place until its change has landed, then remove it so
  stale worktrees do not accumulate.

## Provider-neutral resolution

- The harness must not hardcode any provider's surface syntax. Command sigils,
  invocation grammar, prompt file formats, and naming conventions belong to
  that provider's translator and reach the harness only as data on translator
  API types, such as `AgentCommand.invocation`.
- Agency owns one neutral surface the user types against. `/` is Agency's
  command sigil no matter what an agent uses natively; the translator maps it
  to and from the provider's native form.
- Any code that resolves a user action to an agent — commands, skills, prompts,
  MCP entries, sessions, worktrees — must decide from translator-supplied data,
  never from a literal sigil or a `match` on which provider it is. Adding an
  agent should mean adding a translator, not editing resolution logic.
- Cover each resolution path with a test using a fabricated provider whose
  syntax matches no shipped agent, so a hardcoded assumption fails a test
  instead of the next integration.

This is scoped to resolution and syntax, not to the existence of the `Provider`
enum, which the harness legitimately matches on elsewhere — mapping a provider
to its translator ID, or wiring each agent's process at startup.

## UI color and theming

- All fixed application colors must come from
  `crates/agency-desktop/src/ui_theme.rs`. Do not add hexadecimal colors or
  one-off widget palettes in views or assets. User-configurable semantic mode
  colors belong in `config.rs` and are the only exception.
- New widgets must use the shared Agency theme or a semantic style helper from
  `ui_theme` (`rail`, `status_bar`, `icon_button`, `session_button`, etc.).
- Keep every surface in the same Tokyo Night hierarchy: `BACKGROUND` for the
  application canvas and rails, `SURFACE` for controls and bars,
  `SURFACE_RAISED` for hover/pressed states, and `SURFACE_SELECTED` for selection.
- Text on dark surfaces uses `TEXT`. Interactive focus and selection use
  `PRIMARY`; borders use `BORDER`. Success, warning, and error states use their
  matching semantic tokens.
- Icons and their surrounding controls must have the same background, border,
  hover, pressed, and focus treatment as equivalent text controls.
- Never rely on a light-system default inside the dark application theme.
  Check enabled, hovered, pressed, focused/selected, and disabled states when
  adding an element.
- Preserve readable contrast: normal text should target WCAG AA (4.5:1), and
  large text, icons, borders, and focus indicators should target at least 3:1.

## Confirmation modals

- Destructive actions must open a confirmation modal that names the affected
  item and clearly labels the destructive button.
- While a modal is open, it owns input and blocks interaction with content
  behind it. Enter confirms the primary action and Escape cancels and closes
  the modal. Always provide equivalent visible pointer controls.
- Confirmation modals must use shared semantic styles from `ui_theme`; use the
  danger token for destructive actions.

## Event-driven application state

- All application interactions and state transitions must be represented as
  typed application events. This includes pointer and keyboard actions, opening
  or closing bars and panels, focus changes, layout changes, agent lifecycle
  changes, questions, thinking and idle transitions, and switching agents,
  sessions, worktrees, or tools.
- Publish events through the application event bus. Do not directly mutate
  another feature's state from a view, input handler, or unrelated component.
- Each stateful feature must own a self-contained state facet and reduce the
  events it observes. Views render from facet state and emit intents; they do
  not coordinate other views.
- Keep event ordering deterministic. Follow-up interactions must be published
  as new events instead of recursively invoking handlers or depending on
  listener execution order.
- Treat process spawning, filesystem access, persistence, terminal I/O, and
  agent communication as effects. Effects must publish typed success, failure,
  or lifecycle events back to the bus so every interested facet can react.
- Add reducer and event-flow tests for new interactions, including cross-facet
  behavior such as focus following a panel open or agent switch.
