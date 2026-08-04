# `start_worktree_session` — design

Date: 2026-08-04
Branch: `feat/start-worktree-session-tool`

## Problem

Agency has no way for an agent to hand work to another worktree. An agent can
list, create, and remove worktrees, but the only way to act in one is for the
whole application to switch to it — and switching tears down every running
agent. `select_worktree` (`crates/agency-desktop/src/main.rs:2492`) revokes each
agent's RPC capability and clears `self.agents`, so worktrees cannot host
sessions in parallel today. Terminals already survive a switch, each carrying
its own working directory, so the application is inconsistent with itself.

Two things follow from that, and this spec covers both:

1. Sessions must belong to a worktree instead of to the application, so every
   worktree can run sessions at the same time.
2. A new MCP tool, `start_worktree_session`, starts a session in a named
   worktree with a given prompt on a given agent, and a system-prompt directive
   makes that tool the only way an agent reaches another worktree.

## Decisions

- Started sessions run **in parallel**; the caller keeps running in its own
  worktree and neither the active worktree nor the focused agent changes.
- The agent rail shows **only the active worktree's sessions**. Sessions in
  other worktrees keep running unwatched, mirroring terminals.
- The tool is **fire-and-forget**: it returns once the session is spawned and
  the prompt is sent. `handle_rpc_calls` replies synchronously inside the event
  loop, so a blocking variant would freeze the application. Reporting results
  back to the caller is out of scope and would need an asynchronous reply path.
- The target worktree is addressed by **branch name and must already exist**,
  matching `create_worktree` and `remove_worktree`, which both key on `branch`.
  A missing worktree is an error naming the branch, not an implicit create.
- The directive is **strict**: a session may never enter another worktree, and
  that includes the worktree it just created for the user's current task. The
  repository's own `CLAUDE.md` guidance is updated in the same change so it does
  not contradict the harness.

## Architecture

### Agents own their worktree

- `AgentView` gains `workspace: PathBuf`, set once at spawn. Every value read
  from `self.cwd` at spawn or resume time reads `agent.workspace` instead: the
  child process working directory, the `SessionContext.workspace` issued in
  `issue_rpc_capability` (so a delegated session's own worktree tools act on its
  own worktree), the session directory, and `DiffSessionState`.
- A new `crates/agency-desktop/src/workspaces.rs` holds
  `WorktreeState { registry: SessionRegistry, mcp_servers: Vec<McpServer> }` and
  a map keyed by worktree path. It replaces `App`'s single `sessions` and
  `mcp_servers` fields. Naming a delegated session writes into that worktree's
  own `.agency` sessions directory. `main.rs` is already over 8,000 lines, so
  the facet lives in its own module; no other refactor is in scope.
- `self.cwd` keeps one meaning: what the explorer, file viewer, terminals, and
  status bar are looking at.

### Switching worktrees stops tearing down

`select_worktree` sets `cwd`, points the toolbar at that worktree's registry,
and re-selects `active_agent` to an agent belonging to the new worktree if there
is one — the same re-selection terminals already perform at `main.rs:2521`. It
no longer revokes RPC capabilities and no longer clears `agents`.

The agent rail filters by `agent.workspace == self.cwd`. `active_agent` remains
an index into the full `agents` vector so nothing downstream shifts; the rail
renders a filtered list of indices.

MCP-server changes stay scoped to the active worktree: the reconnect loop at
`main.rs:2277` filters to agents in that worktree, so adding a server cannot
restart a delegated session elsewhere.

Consequence to accept: agents no longer die when the user switches away, so a
busy background session can consume tokens unwatched. The `is_busy` aggregation
at `main.rs:2258` already scans every agent, so quit confirmation still accounts
for them.

### The tool

MCP surface, added to `tools()` in `crates/agency-mcp/src/lib.rs`:

```
start_worktree_session
  worktree (string, required)  — branch name of an existing worktree, as listed by list_worktrees
  prompt   (string, required)  — the first message for the new session
  agent    (string, required)  — which agent to run, e.g. "claude" or "codex"
```

`call_tool` maps it to a new RPC method `worktree.start_session`, alongside the
existing `worktree.*` entries.

The handler in `handle_rpc_calls` follows the `worktree.create` precedent — do
the work inline, emit a typed event:

1. Resolve `worktree` against `worktrees::discover(&call.context.workspace)`.
   Not found is an error naming the branch and listing the known ones.
2. Resolve `agent` through `Provider::from_name`
   (`crates/agency-agents/src/lib.rs:76`), the existing entry point for "the
   name a user types for a provider", so adding a provider does not touch this
   handler. An unknown name is an error listing the accepted names.
3. Reject a blank `prompt` before spawning anything.
4. Issue an RPC capability whose `workspace` is the target worktree, load that
   worktree's `WorktreeState`, and spawn the agent with the target worktree as
   its working directory.
5. Push the `AgentView` with `workspace` set and the prompt preloaded, then
   `submit()` it so the message goes out on the first turn, and name the session
   from the prompt via `name_from_prompt` in that worktree's registry.
6. Leave `active_agent` and `self.cwd` untouched. Emit
   `AppEvent::WorktreeSessionStarted { conversation_id, worktree, provider }`,
   which drives the notice.
7. Reply with `{ caller, session: { conversation_id, worktree, agent } }`.

`start_agent` and this handler both become thin callers of one
`start_session_in(provider, workspace, initial_prompt) -> Result<String, String>`
that spawns, registers, and pushes the view. `start_agent` passes `self.cwd` and
`None` and focuses the result; the RPC handler passes the target worktree and
the prompt and does not. Keeping the two paths on one implementation is what
stops them drifting.

Starting a session in the caller's own worktree is allowed — it is just another
agent there, and forbidding it would be an arbitrary special case.

### The injected directive

`agency_harness_context` (`crates/agency-agents/src/lib.rs:841`) already feeds
both providers: Claude through `--append-system-prompt` (line 598) and Codex
through `developerInstructions` (lines 453 and 461). One edit covers both.
Appended text:

> Your session is bound to the worktree it was started in, and that worktree is
> its working directory. Do not move to another worktree: never `cd` into one
> and never use a provider-native worktree-entry or worktree-switch tool. To get
> work done in a different worktree, call `start_worktree_session` with that
> worktree's branch, a prompt, and an agent — it starts a separate session there
> that runs in parallel with yours.

`CLAUDE.md`'s "Work in a worktree" section is updated in the same change:
creating a worktree is followed by handing the work to it with
`start_worktree_session` rather than entering it, and a new bullet states that
`start_worktree_session` is the only way into a worktree.

This changes how interactive work begins. A session that is asked to build
something creates the worktree, then starts a session in it with a prompt
carrying the task; the user reaches that conversation by switching to the
worktree's tab, guided by the notice. The original session stays where it is.
The handoff is only as good as the prompt the first session writes, and that is
accepted.

## Errors

Every failure is returned to the caller as MCP `isError` text and surfaced as a
notice.

- Unknown branch, unknown agent name, and blank prompt are rejected before
  anything spawns.
- A spawn failure pushes no `AgentView`, revokes the RPC capability that was
  issued, and returns the spawn error.
- `worktree.remove` gains a guard: it already refuses a worktree with
  uncommitted changes, and must also refuse one that has a live session, naming
  it. Without that, removing a worktree leaves a delegated agent working in a
  deleted directory. This is a direct consequence of sessions outliving worktree
  switches.

## Testing

Existing desktop tests are pure-function and facet tests; nothing constructs a
running `Agency`, because spawning needs real agent processes. The design keeps
decisions separate from effects so the decisions can be tested.

- `resolve_start_request(params, &worktrees) -> Result<StartRequest, String>` is
  pure. Tests cover unknown branch, blank prompt, and agent-name resolution,
  including a fabricated agent name matching no shipped provider, which must
  fail with the accepted-name list rather than defaulting silently. This is the
  provider-neutral resolution test `CLAUDE.md` requires.
- `workspaces.rs`: recording a session name for worktree B does not touch
  worktree A's registry, and each worktree's MCP list stays its own.
- `active_after_switch(agent_workspaces, cwd, current)` is pure: switching
  worktrees selects an agent belonging to the new worktree or none, and never
  drops agents from the roster. This is the regression test for the
  parallel-session bug.
- `agency-mcp`: `tools()` lists `start_worktree_session` with its three required
  properties, and `call_tool` maps it to `worktree.start_session`.
- `agency-agents`: the existing `harness_context_identifies_the_agency_session`
  test extends to assert the directive names `start_worktree_session` and
  forbids entering another worktree.

Not covered by automated tests, verified by hand: the spawn-and-send path. Start
a session in another worktree from a tool call, confirm it runs, switch tabs and
see it, switch away and confirm it keeps running.

## Out of scope

- Returning a delegated session's results to its caller; that needs an
  asynchronous RPC reply path and a separate tool.
- Creating a worktree as part of starting a session.
- Any refactor of `main.rs` beyond extracting the per-worktree state facet.
