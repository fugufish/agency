# Agency North Star

## Vision

Agency is a universal desktop environment for agentic software development. It gives developers one coherent place to plan, build, review, and manage work with coding agents—without making the workflow depend on a particular operating system or agent platform.

It competes in the same category as Conductor, but Agency's ambition extends beyond running agents in parallel. Agency is an opinionated, end-to-end development system built around repositories, issues, isolated worktrees, agent execution, review, and integration.

## Product principles

### Universal desktop app

Agency must work equally well across supported desktop platforms. The core experience, capabilities, and project model should remain consistent regardless of operating system.

Platform-specific integrations may improve the experience, but they must not redefine it. A workflow that is fundamental to Agency should not exist on only one platform.

### Agent-platform agnostic

Agency is not a wrapper around a single agentic coding platform. Codex and Claude Code are the first supported platforms, not permanent architectural boundaries.

Platform integrations should adapt each agent's capabilities into Agency's shared concepts and workflows. Provider-specific features may be exposed where useful, but the product must remain valuable without requiring users to organize their work around one provider's terminology or constraints.

### Worktree-driven

Git worktrees are the foundation of Agency's execution model. Each unit of active development should have an explicit, isolated working context that users and agents can inspect, operate on, and reason about safely.

Worktrees should make parallel work understandable: which task owns a change, which agent is working on it, what state it is in, and how it relates to the primary branch. Agency should manage this lifecycle without hiding the underlying Git model or taking ownership away from the developer.

### Comprehensive universal workflows

Agency should provide complete workflows for the software-development lifecycle:

- planning and task decomposition
- creating and assigning isolated work
- agent execution and collaboration
- monitoring progress and intervening when needed
- inspecting changes and reviewing code
- running validation and resolving failures
- integrating, handing off, or abandoning work
- preserving history, decisions, and context

These workflows must operate consistently across Codex and Claude Code. Their meaning should come from Agency's product model, while adapters translate them into the capabilities of each platform.

### Agency develops Agency

Agency is developed with Agency.

The project should use its own planning, worktree management, agent execution, review, validation, and integration workflows for day-to-day development. This is not a showcase mode or an occasional test—it is the primary way Agency is built.

Dogfooding keeps the product grounded in real development work. Friction encountered while building Agency is product feedback; missing capabilities expose workflow gaps; and improvements should benefit both Agency's maintainers and its users.

This principle also creates a practical standard: Agency must remain capable of supporting its own continued development.

## Foundational capabilities

The first useful version of Agency must make the complete development loop possible without forcing the developer to assemble it across unrelated tools.

### Repository integration

Agency should deeply understand the repository as a development system, not merely as a directory of files. It should surface branches, worktrees, commits, remotes, checks, pull requests, and repository state in the context of the work being performed.

GitHub is the first hosted repository integration. The underlying product model must remain host-neutral so that other forges can be added without redesigning Agency's workflows.

### Issue management

Issues are actionable units of development, not read-only links. A developer should be able to find, triage, refine, plan, assign, execute, review, and complete issue-driven work from Agency.

GitHub Issues is the first issue provider. As with repositories, Agency's workflow concepts must remain independent of GitHub-specific terminology and APIs.

### Embedded terminal

Every worktree should provide an embedded terminal with the correct repository context and environment. Developers must retain direct access to their tools and be able to inspect or intervene in agent work without leaving Agency.

The terminal is a first-class part of the workflow, not an escape hatch.

### Diff viewer

Agency should provide a high-quality diff and review experience tied to each worktree and unit of work. Developers should be able to understand what changed, why it changed, what remains unresolved, and whether the result is ready to integrate.

Review should connect changes to plans, issue requirements, agent activity, validation results, and developer feedback.

## Opinionated core workflow

Agency's foundational workflow is:

1. Connect a repository and its issue source.
2. Select or define a unit of work.
3. Refine the requirements and produce an explicit plan.
4. Create an isolated worktree for that work.
5. Assign one or more supported agents to execute within it.
6. Observe progress and intervene through structured controls or the terminal.
7. Review the resulting diff against the plan and issue.
8. Run validation and resolve failures or review findings.
9. Integrate the work through the repository's delivery workflow.
10. Update the issue and preserve the development record.

Individual platforms may execute these steps differently, but Agency owns the workflow and presents it consistently.

## Decision filter

When evaluating a feature or architectural choice, ask:

1. Does it provide a first-class experience on every supported desktop platform?
2. Is the core concept independent of any single agent provider?
3. Does it preserve a clear, inspectable relationship between work and its worktree?
4. Does it strengthen an end-to-end workflow instead of adding an isolated capability?
5. Can Codex and Claude Code users accomplish the same underlying goal, even when the implementation differs?
6. Can Agency's team use it to build Agency itself?

If the answer to any of these is no, the design should be reconsidered or explicitly treated as a temporary platform-specific exception.

## What Agency is not

Agency is not:

- a thin GUI for one agent CLI
- a collection of disconnected prompts and terminal sessions
- an abstraction that erases Git, worktrees, or provider capabilities
- a product whose essential workflows depend on one operating system

The goal is a durable, provider-neutral development environment that makes agentic work structured, observable, reviewable, and safe.
