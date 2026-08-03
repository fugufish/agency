# Worktree integration

## Problem

Worktrees are the foundation of Agency's execution model, but only two thirds of
the loop exist. `crates/agency-desktop/src/worktrees.rs` can `discover` and
`create`; there is no way to remove one. The MCP surface in
`crates/agency-mcp/src/lib.rs` exposes `list_worktrees` and `create_worktree`
against the same gap. An agent can therefore accumulate worktrees but never
retire them, and the tab strip only ever grows.

Three further problems sit behind that gap.

`create` writes to a sibling directory, `../{repository}-{hint}`, while
`.gitignore` already reserves `.agency/worktrees/**`. The repository disagrees
with itself about where worktrees live.

Session history is stored away from the worktree it belongs to.
`worktree_sessions_directory` in `crates/agency-desktop/src/sessions.rs`
resolves every worktree's history to the *primary* repository root, keyed by
url-encoded branch name with the literal `root` for the primary and
`detached-<sha>` for a detached head. Sessions carry image caches, diffs, and
artifacts, so a worktree that is removed leaves history behind that nothing will
ever collect.

The `worktree.create` RPC handler at `main.rs:2647` mutates `self.worktrees` and
`self.active_worktree` inline, which the event-driven rule in `CLAUDE.md`
forbids. Startup discovery reaches the same fields by a third path.

## Approach

Make the worktree directory the unit of work: one directory that holds the
checkout *and* its session history, so removing the worktree removes everything
that belongs to it. Git already deletes a worktree directory wholesale,
including ignored files, so cleanup needs no code at all — no `remove_dir_all`,
no garbage collection pass, no orphan-detection heuristic.

This depends on a git behavior worth stating explicitly, because the whole
design rests on it and it is not obvious from the documentation. `git worktree
remove` refuses a worktree containing modified or untracked files, but *ignored*
files neither block the removal nor survive it. Verified against a scratch
repository: a worktree nested inside the primary's ignored `.agency/worktrees/`
leaves the primary's `git status` clean, 200 KB of ignored session data inside
that worktree does not require `--force`, the removal deletes the session data
with the checkout, and the branch survives.

### Layout

```
<repo>/                                   # primary worktree
  .agency/
    config.toml                           # tracked
    sessions/<conversation-id>/session.json
    worktrees/
      feature%2Ftabs/                     # the checkout — this IS the worktree root
        .agency/
          config.toml                     # tracked, arrives from git
          sessions/                       # this worktree's history — dies with it
```

The directory name is `path_component(branch)`, the encoding `sessions.rs`
already applies. There is no intermediate `tree/` level: the checkout is the
directory.

`.gitignore` gains `.agency/sessions/`. That line is load-bearing. It is what
keeps status clean in the primary *and* in every nested worktree, since a nested
worktree checks out the same `.gitignore`, and a worktree reporting untracked
files is a worktree `git worktree remove` will refuse.

`.agency/worktrees/**` stays as it is. `.agency/config.toml` stays tracked, so
every worktree receives the workspace configuration from git.

#### Why not `.agents/`

`.agents/` is the cross-agent shared convention and a live skill root:
`crates/agency-translators/src/codex/commands.rs:23` scans
`workspace/.agents/skills` as a project origin. Placing checkouts under
`.agents/` would put full repository copies, each with its own `.agents/skills`,
inside a directory agent tooling is expected to walk. `.agency/` is Agency's own
namespace, already established by `WORKSPACE_CONFIG_DIRECTORY` in `config.rs:7`.

Skill discovery needs no change to support the nesting. Neither translator walks
up the tree: `codex/commands.rs:17` and `claude/commands.rs:230` are both
`catalog(home, workspace)`, reading exactly the roots under the `workspace` they
are handed plus the ones under `home`. Each tab already treats its worktree as
the true root. The only code that reached past a worktree to the primary was
`worktree_sessions_directory`, which this design removes.

### Sessions follow the worktree

`worktree_sessions_directory` becomes:

```rust
workspace.join(".agency").join("sessions")
```

This deletes roughly thirty-five lines of git plumbing — `rev-parse
--show-toplevel`, `worktree list --porcelain`, `branch --show-current` — along
with the `root` and `detached-<sha>` keying and the branch-rename fragility that
came with keying a directory by a name git lets you change. `path_component`
stays; it is still used for conversation IDs, and `worktrees.rs` now uses it for
the checkout directory name.

`SessionRegistry::load` and `SessionRegistry::new` are unchanged beyond calling
the simpler function. The existing `.agency/sessions.json` legacy fallback is
untouched.

A one-time migration runs in `Agency::build`, after worktree discovery resolves
`cwd` and before `SessionRegistry::load(&cwd)` reads from the new location: if
`.agency/worktrees/root/sessions/` exists and `.agency/sessions/` does not,
rename it and remove the now-empty `root` directory. It is a no-op on every
launch afterward and is deletable once existing history has moved. Without it,
the old `root` directory
would sit in the same namespace as checkouts and read as a worktree for a branch
named `root` — inert, since tabs come from git rather than from the filesystem,
but misleading.

### The recursion guard

`create` must always resolve the parent directory from the **primary** worktree,
never from the caller's workspace. Creating worktree B from inside worktree A
would otherwise nest B at `A/.agency/worktrees/B`, and removing A would silently
take B with it.

`worktrees.rs:52` already computes the primary as `discover()?.first()`. That
becomes the explicit contract of the function rather than an incidental
consequence of how the path is built, and it gets a test.

### Module surface

`worktrees.rs` remains the only place that shells out to git for worktree
operations.

```rust
pub fn discover(workspace: &Path) -> Result<Vec<Worktree>, String>
pub fn create(workspace: &Path, branch: &str, base: Option<&str>) -> Result<Worktree, String>
pub fn remove(workspace: &Path, branch: &str) -> Result<Worktree, String>
```

`create` loses the `path_hint` parameter and the hand-rolled hint validation at
`worktrees.rs:64`. The path is derived from the branch, and `path_component`
already guarantees a safe directory name, so the second identifier and its
validation rules both disappear. Branch validation via `check-ref-format` stays.

`remove` takes a branch, resolves it to a path through `discover`, and returns
the removed `Worktree` so the caller knows what disappeared without querying
again. It refuses when:

- the branch matches no worktree
- the branch is the primary worktree
- `git worktree remove` reports the checkout dirty

There is no `force` parameter. An agent should not be able to destroy
uncommitted work over MCP, and refusing on a dirty tree is what keeps removal
from being a surprise: the checkout is recoverable from git, but the session
history removal destroys is not. A forced removal is a UI affordance for a
later change, where it can carry the confirmation modal `CLAUDE.md` requires.

The branch is not deleted. Removing a checkout is not abandoning the work, and
git leaves the branch in place on its own, so this is the behavior with no code
behind it.

Every entry point keys on the branch — never a path, never an index — so one
identifier spans the module, the RPC layer, and the MCP schema.

### Events

Both RPC handlers become effects: they run the git operation and publish a typed
event rather than writing to `self`. Three new variants on `AppEvent`:

```rust
WorktreesDiscovered { worktrees: Vec<Worktree> },
WorktreeCreated { worktree: Worktree },
WorktreeRemoved { branch: String },
```

One reducer owns `worktrees` and `active_worktree`. Startup discovery, creation,
and removal all converge on it, replacing the three separate paths that reach
those fields today.

- `WorktreesDiscovered` replaces the list and re-resolves `active_worktree` by
  matching `self.cwd`, falling back to the primary at index 0.
- `WorktreeCreated` re-discovers and appends the tab **without** stealing focus.
  An agent creating a worktree in the background must not move the user's view.
- `WorktreeRemoved` drops the tab. If the removed worktree was the active one,
  the reducer publishes `SelectWorktree(0)` as a follow-up event rather than
  calling `select_worktree` directly, keeping ordering deterministic as the
  event rule requires.

Falling back to the primary tab is preferred over refusing to remove the active
worktree. Refusing would make an MCP call fail based on which tab the user
happened to be looking at, which is not a property the caller can reason about.

`select_worktree` already tears down agents, revokes RPC capabilities, reloads
the session registry, and reseeds the slash catalog, so the follow-up event
needs no new teardown logic.

### RPC and MCP surface

| MCP tool | RPC method | Arguments |
|---|---|---|
| `list_worktrees` | `worktree.list` | — |
| `create_worktree` | `worktree.create` | `branch`, `base?` |
| `remove_worktree` | `worktree.remove` | `branch` |

All three return the same worktree shape — `path`, `label`, `branch` — through
the existing `worktree_json`. `create_worktree` drops `path_hint` from its input
schema. `README.md` lists the tools and gains the third.

### Explorer

`collect_explorer_entries` at `main.rs:5599` filters nothing and recurses into
any directory the user has expanded. It is called with `self.cwd` at depth 0
(`main.rs:2454`), so expansion is lazy and nothing walks the worktrees
unprompted — but expanding `.agency/worktrees` renders a full copy of the
repository inside the primary's own tree, once per worktree, and lets the same
file be opened under two paths.

The function takes a `root: &Path` parameter, threaded through the recursive
call, and skips the child at `root.join(".agency/worktrees")`. Matching the
resolved path rather than a directory name avoids hiding a `worktrees`
directory that happens to appear elsewhere in the repository.

## Testing

**`worktrees.rs`**, against real temporary repositories, since the module's
entire job is git behavior:

- `create` called from inside a worktree places the new checkout under the
  primary's `.agency/worktrees/`, not under the caller's worktree — the
  recursion guard
- `create` derives the directory from the branch, with a slashed branch such as
  `feature/tabs` encoded to a single path component
- `remove` deletes the checkout together with ignored session data written under
  its `.agency/sessions/`, and does not need `--force` to do it — the git
  behavior the design rests on, pinned so an upgrade that changes it fails here
  rather than in production
- `remove` leaves the branch in place
- `remove` refuses a dirty worktree, and the worktree still exists afterward
- `remove` refuses the primary worktree
- `remove` refuses an unknown branch

**`sessions.rs`**:

- `worktree_sessions_directory` returns `<workspace>/.agency/sessions` for any
  workspace, with no git invocation — asserted against a plain directory that is
  not a repository at all, which the current implementation cannot satisfy
- the startup migration moves `.agency/worktrees/root/sessions/` to
  `.agency/sessions/` and is a no-op when the destination already exists

**`main.rs`** reducer tests, through `Agency::for_testing`:

- `WorktreesDiscovered` re-resolves `active_worktree` from `cwd`
- `WorktreeCreated` appends a tab and leaves `active_worktree` unchanged
- `WorktreeRemoved` for a non-active worktree drops the tab and keeps focus
- `WorktreeRemoved` for the active worktree publishes `SelectWorktree(0)`

**Provider neutrality**, as `CLAUDE.md` requires: the RPC path is exercised
through a fabricated provider whose syntax matches no shipped agent, so a
hardcoded Codex or Claude assumption in worktree resolution fails a test rather
than the next integration.

## What does not change

- Tabs come from `git worktree list` via `discover`, not from the filesystem
  layout. A directory under `.agency/worktrees/` with no live worktree is not a
  tab.
- Worktrees created under the old `../{repository}-{hint}` scheme keep working.
  Git still reports them, so they still get tabs, and `remove` still handles
  them. No migration of existing checkouts.
- `select_worktree` and its teardown.
- `parse_porcelain` and the `Worktree` type.
- Skill and command discovery in both translators.

## Out of scope

- A `force` option on removal, and the confirmation modal that would gate it.
- UI affordances for creating or removing worktrees. This change is the MCP and
  state layer; the tab strip renders what the reducer holds.
- Deleting the branch alongside the checkout.
- Garbage collection of session directories orphaned by worktrees removed
  outside Agency. Nesting removes the common case; the residual case is not
  worth a heuristic that can delete history for a branch still in use.
- Moving existing sibling worktrees into `.agency/worktrees/`.
- Per-worktree agent state beyond sessions.
