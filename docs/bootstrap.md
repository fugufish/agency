# Agency Bootstrap

## Objective

Bootstrap the smallest self-hosting version of Agency: a desktop app that can connect to a GitHub repository, turn a GitHub issue into an isolated worktree, run an agent with an embedded terminal, review its diff, and hand the result back to GitHub.

This vertical slice should be good enough for Agency's team to use while continuing to build Agency.

## First vertical slice

### 1. Repository workspace

- open an existing local Git repository
- connect and authenticate its GitHub remote
- display repository, branch, and worktree status
- discover existing worktrees without taking ownership of them

### 2. Issue inbox

- list and filter open GitHub issues
- display issue details and discussion
- select an issue as the source of a new unit of work
- retain the relationship between issue, worktree, branch, and agent session

### 3. Worktree lifecycle

- create an isolated worktree and branch from a chosen base
- show its lifecycle and current status
- open, resume, archive, or remove it through explicit user actions
- detect conflicts and external Git changes safely

### 4. Agent session

- start Codex or Claude Code inside the selected worktree
- preserve the provider's native authentication and capabilities
- stream activity and expose clear running, waiting, failed, and completed states
- resume a session where the provider supports it

### 5. Embedded terminal

- open a real interactive shell rooted in the worktree
- support multiple terminal sessions
- handle resize, process exit, copy, paste, and links
- make agent and user terminal activity distinguishable

### 6. Review

- show uncommitted and committed changes for the worktree
- provide file navigation and side-by-side or unified diffs
- distinguish added, modified, deleted, renamed, and binary files
- connect review findings back to the active agent session

### 7. Validate and deliver

- run repository-defined validation commands
- show check results alongside the diff
- commit and push with explicit user confirmation
- create or update a GitHub pull request
- reflect delivery status back onto the unit of work

## Core product objects

- **Repository**: a local Git repository and its optional hosted-forge connection
- **Issue**: provider-neutral source work imported from an issue service
- **Workspace**: one unit of work backed by a Git worktree and branch
- **Agent session**: a provider-specific agent process operating in a workspace
- **Terminal session**: an interactive shell or process attached to a workspace
- **Review**: the diff, findings, validation, and approval state for a workspace
- **Delivery**: commits, push state, pull request, checks, and issue completion

Provider adapters translate GitHub, Codex, and Claude Code into these objects. Product surfaces should depend on the objects rather than directly on provider APIs.

## Bootstrap sequence

1. Establish the cross-platform desktop shell and application layout.
2. Implement local repository discovery and worktree management.
3. Add the workspace model and persistent local state.
4. Embed the terminal and process lifecycle management.
5. Add Codex and Claude Code adapters.
6. Build the diff and review surface.
7. Add GitHub repository, issue, and pull-request integration.
8. Close the loop with validation, delivery, and issue updates.
9. Use Agency to implement the next Agency feature.

## Initial success criterion

The bootstrap is successful when a Agency contributor can take a real Agency issue from selection through a reviewed pull request using Agency as the primary interface, with either Codex or Claude Code as the implementation agent.
