# `start_worktree_session` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an MCP tool that starts a session in a named worktree with a given prompt on a given agent, and make sessions belong to worktrees so every worktree can run sessions in parallel.

**Architecture:** Per-worktree state (session registry + MCP servers) moves out of `Agency` into a keyed facet in a new `workspaces.rs`; `AgentView` carries the worktree it was spawned in; switching worktrees changes the view instead of tearing agents down. A new RPC method `worktree.start_session` resolves a branch and an agent name, spawns a session in that worktree, and sends the prompt without moving the caller's focus. Both agents get a system-prompt directive making that tool the only way into another worktree.

**Tech Stack:** Rust 2024 edition, `iced` desktop application, `serde_json`, workspace crates `agency-desktop`, `agency-mcp`, `agency-agents`, `agency-rpc`.

## Global Constraints

- Work happens in the worktree `/home/fugufish/Code/agency/.agency/worktrees/feat%2Fstart-worktree-session-tool` on branch `feat/start-worktree-session-tool`. Do not commit to the primary checkout.
- The spec is `docs/superpowers/specs/2026-08-04-start-worktree-session-design.md` in that worktree. It is authoritative; this plan implements it.
- Build and test with `cargo test -p <crate>` from the worktree root. `cargo fmt` and `cargo clippy --all-targets` must be clean before each commit.
- No new dependencies.
- Agent-name resolution must go through `Provider::from_name` (`crates/agency-agents/src/lib.rs:76`). Never match on a provider literal in resolution code.
- Every user-visible interaction stays a typed `AppEvent` published through the event bus; effects publish their outcome back as events.
- Error strings are sentences naming the thing that failed, matching the existing style in `crates/agency-desktop/src/worktrees.rs`.

---

## File Structure

- **Create `crates/agency-desktop/src/workspaces.rs`** — per-worktree state (`WorktreeState`, `Workspaces`), the pure resolution of a start-session request (`StartRequest`, `resolve_start_request`), and the two pure selection helpers (`active_after_switch`, `has_live_session`). This is the only new module; it exists so `main.rs` (8,089 lines) does not grow further.
- **Modify `crates/agency-desktop/src/main.rs`** — `Agency` swaps its `sessions` and `mcp_servers` fields for a `Workspaces`; `AgentView` gains `workspace`; `start_session_in` becomes the one spawn path; `select_worktree` stops tearing down; two RPC handlers change.
- **Modify `crates/agency-mcp/src/lib.rs`** — declare the `start_worktree_session` tool and map it to `worktree.start_session`.
- **Modify `crates/agency-agents/src/lib.rs`** — `Provider::ALL`, and the worktree directive appended to `agency_harness_context`.
- **Modify `CLAUDE.md`** — repository guidance matches the new directive.

---

### Task 1: Per-worktree state facet

**Files:**
- Create: `crates/agency-desktop/src/workspaces.rs`
- Modify: `crates/agency-desktop/src/main.rs` (add `mod workspaces;` beside the other `mod` lines at the top, around line 10)
- Test: `crates/agency-desktop/src/workspaces.rs` (`#[cfg(test)] mod tests` at the bottom, matching the crate's in-file test convention)

**Interfaces:**
- Consumes: `crate::sessions::SessionRegistry` (`load`, `empty`, `records`), `agency_agents::McpServer`.
- Produces: `WorktreeState { registry: SessionRegistry, mcp_servers: Vec<McpServer> }`, `Workspaces::new()`, `Workspaces::ensure(&mut self, workspace: &Path) -> Result<(), String>`, `Workspaces::state(&self, workspace: &Path) -> &WorktreeState`, `Workspaces::state_mut(&mut self, workspace: &Path) -> &mut WorktreeState`.

- [ ] **Step 1: Write the failing test**

Add to `crates/agency-desktop/src/workspaces.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use agency_agents::Provider;

    fn temp_workspace(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("agency-workspaces-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("could not create the test workspace");
        path
    }

    /// Each worktree keeps its own sessions on disk, so naming a delegated
    /// session must not land in the worktree the user happens to be looking at.
    #[test]
    fn recording_a_session_touches_only_its_own_worktree() {
        let first = temp_workspace("first");
        let second = temp_workspace("second");
        let mut workspaces = Workspaces::new();
        workspaces.ensure(&first).expect("could not load the first worktree");
        workspaces.ensure(&second).expect("could not load the second worktree");

        workspaces
            .state_mut(&first)
            .registry
            .record(Provider::Claude, "claude-1".to_owned(), Some("First".to_owned()))
            .expect("could not record the session");

        assert_eq!(workspaces.state(&first).registry.records().len(), 1);
        assert!(workspaces.state(&second).registry.records().is_empty());
    }

    /// MCP servers are added per worktree today (the list is cleared on every
    /// switch), and that has to survive the move into the facet.
    #[test]
    fn mcp_servers_are_scoped_to_their_worktree() {
        let first = temp_workspace("mcp-first");
        let second = temp_workspace("mcp-second");
        let mut workspaces = Workspaces::new();
        workspaces.ensure(&first).expect("could not load the first worktree");
        workspaces.ensure(&second).expect("could not load the second worktree");

        workspaces.state_mut(&first).mcp_servers.push(McpServer {
            name: "docs".to_owned(),
            enabled: true,
            transport: McpTransport::Stdio {
                command: "docs-mcp".to_owned(),
                args: Vec::new(),
                env: None,
                cwd: None,
            },
        });

        assert_eq!(workspaces.state(&first).mcp_servers.len(), 1);
        assert!(workspaces.state(&second).mcp_servers.is_empty());
    }

    /// `ensure` is what loads a worktree's registry from disk; asking for a
    /// worktree that was never ensured must not panic mid-render.
    #[test]
    fn an_unknown_worktree_reads_as_empty() {
        let workspaces = Workspaces::new();

        assert!(workspaces
            .state(Path::new("/nonexistent/worktree"))
            .registry
            .records()
            .is_empty());
    }
}
```

Check the exact shape of `McpServer` and `McpTransport` in `crates/agency-agents/src/lib.rs:30-58` before writing the second test and match its fields exactly.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agency-desktop workspaces`
Expected: FAIL — `workspaces.rs` has no `Workspaces` type yet (compile error).

- [ ] **Step 3: Write the implementation**

Put this above the test module in `crates/agency-desktop/src/workspaces.rs`:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agency_agents::McpServer;

use crate::sessions::SessionRegistry;

/// Everything Agency tracks for one worktree. Sessions are recorded under the
/// worktree that owns them, and MCP servers are added to the worktree they were
/// configured in, so both have to be keyed by worktree rather than held once on
/// the application.
pub struct WorktreeState {
    pub registry: SessionRegistry,
    pub mcp_servers: Vec<McpServer>,
}

pub struct Workspaces {
    states: HashMap<PathBuf, WorktreeState>,
    /// Read for a worktree that was never `ensure`d. Startup, worktree
    /// switching, and starting a session all `ensure` first, so this is a
    /// render-path guard rather than a state anyone should reach.
    fallback: WorktreeState,
}

impl Workspaces {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            fallback: WorktreeState {
                registry: SessionRegistry::empty(Path::new("")),
                mcp_servers: Vec::new(),
            },
        }
    }

    /// Loads the worktree's sessions from disk the first time it is asked for.
    /// Later calls are cheap, so callers can `ensure` freely.
    pub fn ensure(&mut self, workspace: &Path) -> Result<(), String> {
        if self.states.contains_key(workspace) {
            return Ok(());
        }
        let registry = SessionRegistry::load(workspace)?;
        self.states.insert(
            workspace.to_path_buf(),
            WorktreeState {
                registry,
                mcp_servers: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn state(&self, workspace: &Path) -> &WorktreeState {
        self.states.get(workspace).unwrap_or(&self.fallback)
    }

    pub fn state_mut(&mut self, workspace: &Path) -> &mut WorktreeState {
        self.states.entry(workspace.to_path_buf()).or_insert_with(|| WorktreeState {
            registry: SessionRegistry::empty(workspace),
            mcp_servers: Vec::new(),
        })
    }
}

impl Default for Workspaces {
    fn default() -> Self {
        Self::new()
    }
}
```

Add `mod workspaces;` to the module list at the top of `crates/agency-desktop/src/main.rs` (beside `mod worktrees;` at line 10).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agency-desktop workspaces`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/agency-desktop/src/workspaces.rs crates/agency-desktop/src/main.rs
git commit -m "feat(desktop): key session state by worktree"
```

---

### Task 2: Point the application at the facet

Behaviour must not change in this task. It replaces two `Agency` fields with the facet and routes every existing reader through the active worktree, so later tasks have somewhere to put a second worktree's state.

**Files:**
- Modify: `crates/agency-desktop/src/main.rs` (fields at lines 592 and 610; construction around line 1009; readers listed below)

**Interfaces:**
- Consumes: `Workspaces` from Task 1.
- Produces: `Agency::sessions(&self) -> &SessionRegistry`, `Agency::sessions_mut(&mut self) -> &mut SessionRegistry`, `Agency::mcp_servers(&self) -> &[McpServer]` — all scoped to `self.cwd`.

- [ ] **Step 1: Replace the fields**

In `struct Agency`, delete `sessions: SessionRegistry,` (line 592) and `mcp_servers: Vec<McpServer>,` (line 610). Add:

```rust
    workspaces: workspaces::Workspaces,
```

- [ ] **Step 2: Add the accessors**

In the `impl Agency` block that holds `active_terminal` (around line 3062), add:

```rust
    /// The session registry of the worktree the user is looking at. Sessions
    /// running in other worktrees live in their own registries and are reached
    /// through `self.workspaces`.
    fn sessions(&self) -> &SessionRegistry {
        &self.workspaces.state(&self.cwd).registry
    }

    fn sessions_mut(&mut self) -> &mut SessionRegistry {
        let cwd = self.cwd.clone();
        &mut self.workspaces.state_mut(&cwd).registry
    }

    fn mcp_servers(&self) -> &[McpServer] {
        &self.workspaces.state(&self.cwd).mcp_servers
    }
```

- [ ] **Step 3: Update every reader**

Replace `self.sessions` with `self.sessions()` at lines 1869, 1874, 2619, 2791, 3021, 3096, 3098, 3206, 4515, and with `self.sessions_mut()` at lines 2095, 2142, 3031, 3048, 3072, 3075. Replace `self.mcp_servers` with `self.mcp_servers()` at lines 2241, 2275, 2616, 2831, 2952, 2958, 3527, 3572; at line 2305 (`self.mcp_servers = servers;`) write:

```rust
        let cwd = self.cwd.clone();
        self.workspaces.state_mut(&cwd).mcp_servers = servers;
```

At line 2514 (`self.sessions = sessions;`) and line 2529 (`self.mcp_servers.clear();`) delete both lines — `select_worktree` now only needs `self.workspaces.ensure(&cwd)?`, which Task 4 finishes. For now, replace the registry load at lines 2501-2507 with:

```rust
        if let Err(error) = self.workspaces.ensure(&cwd) {
            self.notice = Some(error);
            return;
        }
```

In the constructor (around line 1009) replace the `sessions` and `mcp_servers` initialisers with a `Workspaces` that has the launch worktree ensured:

```rust
        let mut workspaces = workspaces::Workspaces::new();
        let session_notice = workspaces.ensure(&cwd).err();
```

and keep feeding `session_notice` into the existing `notice` chain at line 999. Delete the now-unused `SessionRegistry::load` call it replaces.

Where a borrow conflict appears (a `self.sessions_mut()` call in the same expression as another `self` field), bind the value first — for example at line 3072:

```rust
        let result = if let Some(conversation_id) = conversation_id {
            self.sessions_mut().record_binding(conversation_id, provider, id, name)
        } else {
            self.sessions_mut().record(provider, id, name)
        };
```

- [ ] **Step 4: Confirm the session list is already worktree-scoped**

The agent rail is the session list rendered from `self.sessions().records()` at line 3206, matched against running agents by conversation ID. Because `sessions()` now returns the active worktree's registry, the list shows only that worktree's sessions with no filtering code — this is what the spec calls filtering the rail by worktree. Read the fold at lines 3203-3240 and confirm nothing else there reaches across worktrees.

- [ ] **Step 5: Verify nothing changed**

Run: `cargo test -p agency-desktop && cargo clippy -p agency-desktop --all-targets`
Expected: PASS with no warnings. The existing 79 tests still pass; no test is added in this task because it is a pure move.

- [ ] **Step 6: Commit**

```bash
git add crates/agency-desktop/src/main.rs
git commit -m "refactor(desktop): read sessions and MCP servers per worktree"
```

---

### Task 3: Agents carry their worktree

**Files:**
- Modify: `crates/agency-desktop/src/main.rs` (`AgentView` at line 624; `start_agent` at line 2602; resume path at line 2843; `issue_rpc_capability` at line 2669; `record_session_updates` at line 3067)

**Interfaces:**
- Consumes: `Workspaces::ensure`, `Workspaces::state_mut` from Task 1.
- Produces: `AgentView.workspace: PathBuf`, `Agency::start_session_in(&mut self, provider: Provider, workspace: PathBuf, initial_prompt: Option<String>) -> Result<String, String>` returning the new conversation ID.

- [ ] **Step 1: Add the field and the spawn path**

Add `workspace: PathBuf,` to `struct AgentView` (line 624), and set it in both existing `AgentView { .. }` literals (lines 2621 and 2843) to the workspace the session was spawned in.

Change `issue_rpc_capability` (line 2669) to take the workspace instead of reading `self.cwd`:

```rust
    fn issue_rpc_capability(
        &self,
        conversation_id: &str,
        provider: Provider,
        workspace: &Path,
    ) -> Result<String, String> {
        if self.rpc_server.is_none() {
            return Err("Agency RPC is unavailable".to_owned());
        }
        self.rpc_capabilities.issue(SessionContext {
            conversation_id: conversation_id.to_owned(),
            workspace: workspace.to_path_buf(),
            provider: match provider {
                Provider::Codex => "codex",
                Provider::Claude => "claude",
            }
            .to_owned(),
            provider_session_id: None,
            generation: 1,
        })
    }
```

Update its three call sites to pass the spawning worktree.

- [ ] **Step 2: Extract the one spawn path**

Add to the `impl Agency` block that holds `start_agent`:

```rust
    /// Spawns a session in `workspace` and, when `initial_prompt` is set, sends
    /// it as the session's first message. Focus is left alone on purpose: a
    /// session started by a tool call belongs to the worktree it was started
    /// in, not to whatever the user is looking at.
    fn start_session_in(
        &mut self,
        provider: Provider,
        workspace: PathBuf,
        initial_prompt: Option<String>,
    ) -> Result<String, String> {
        self.workspaces.ensure(&workspace)?;
        let conversation_id = new_conversation_id();
        let rpc_token = self.issue_rpc_capability(&conversation_id, provider, &workspace)?;
        let environment = self.rpc_environment(&rpc_token, &conversation_id);
        let mcp_servers = self.workspaces.state(&workspace).mcp_servers.to_vec();
        let session = match AgentSession::spawn_with_env_and_mcps(
            provider,
            &workspace,
            &environment,
            &mcp_servers,
        ) {
            Ok(session) => session,
            Err(error) => {
                self.rpc_capabilities.revoke(&rpc_token);
                return Err(error);
            }
        };
        let session_directory = self
            .workspaces
            .state(&workspace)
            .registry
            .session_directory(&conversation_id);
        let diff_state = DiffSessionState::load(&session_directory).unwrap_or_default();
        self.agents.push(AgentView {
            workspace,
            conversation_id: conversation_id.clone(),
            rpc_token,
            session,
            transcript: Vec::new(),
            transcript_dirty: false,
            conversation: Conversation::default(),
            prompt: String::new(),
            prompt_selected: false,
            prompt_cursor: 0,
            prompt_selection_anchor: None,
            command_provider: None,
            images: Vec::new(),
            pending_question: None,
            status: "Initializing".to_owned(),
            session_id: None,
            pending_session_name: None,
            pending_conversation_id: Some(conversation_id.clone()),
            completed_turns: 0,
            activity: AgentActivity::Starting,
            queued_messages: VecDeque::new(),
            image_cache: HashMap::new(),
            image_cache_directory: session_directory.join("images"),
            diff_state,
            session_directory,
            last_changed_at_millis: unix_time_millis(),
            mcp_status: McpStatus::Waiting,
            plugin_installs: TranscriptInstalls::default(),
        });
        if let Some(prompt) = initial_prompt {
            if let Some(agent) = self.agents.last_mut() {
                agent.prompt = normalized_prompt(prompt);
                agent.prompt_cursor = agent.prompt.len();
                agent.prompt_selection_anchor = None;
                agent.submit();
            }
        }
        Ok(conversation_id)
    }
```

Rewrite `start_agent` (line 2602) to call it and keep its own focus and UI follow-ups:

```rust
    fn start_agent(&mut self, provider: Provider) {
        let workspace = self.cwd.clone();
        match self.start_session_in(provider, workspace, None) {
            Ok(_) => {
                self.active_agent = Some(self.agents.len() - 1);
                // The completion ranking depends on the focused agent's
                // provider, so a highlighted row that survived the switch
                // could silently point at a different command.
                self.overlays.slash.close();
                self.emit(AppEvent::TerminalVisibilityChanged(false));
                self.emit(AppEvent::EnterComposer);
                // ...the remaining follow-ups the current body performs.
            }
            Err(error) => self.notice = Some(error),
        }
    }
```

- [ ] **Step 3: Route session naming to the owning worktree**

`record_session_updates` (line 3067) currently writes into the active registry. Find the agent by its RPC token and write into that agent's worktree:

```rust
    fn record_session_updates(&mut self, updates: Vec<SessionUpdate>) {
        for (provider, id, name, conversation_id, rpc_token) in updates {
            self.rpc_capabilities
                .bind_provider_session(&rpc_token, id.clone());
            let workspace = self
                .agents
                .iter()
                .find(|agent| agent.rpc_token == rpc_token)
                .map_or_else(|| self.cwd.clone(), |agent| agent.workspace.clone());
            let registry = &mut self.workspaces.state_mut(&workspace).registry;
            let result = if let Some(conversation_id) = conversation_id {
                registry.record_binding(conversation_id, provider, id, name)
            } else {
                registry.record(provider, id, name)
            };
            if let Err(error) = result {
                self.notice = Some(error);
            }
        }
    }
```

Do the same for the two `name_if_missing` calls (lines 2095 and 2142): resolve the submitting agent's `workspace` and name the session in that worktree's registry rather than `self.sessions_mut()`.

- [ ] **Step 4: Keep an MCP-server change inside its own worktree**

`add_mcp_server` (around line 2264) reconnects every Claude agent when a server is added. Now that agents live in other worktrees, it must only reconnect the ones in the worktree the server was added to. In the `reconnect` collection at line 2277, add a workspace filter and resume in the agent's own worktree:

```rust
        let reconnect = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, agent)| {
                agent.session.provider() == Provider::Claude && agent.workspace == self.cwd
            })
            .map(|(index, agent)| {
                (
                    index,
                    agent.rpc_token.clone(),
                    agent.conversation_id.clone(),
                    agent.session_id.clone().unwrap_or_default(),
                    agent.workspace.clone(),
                )
            })
            .collect::<Vec<_>>();
        for (index, rpc_token, conversation_id, session_id, workspace) in reconnect {
            let environment = self.rpc_environment(&rpc_token, &conversation_id);
            let session = AgentSession::resume_with_env_and_mcps(
                Provider::Claude,
                &session_id,
                &workspace,
                &environment,
                &servers,
            )?;
```

Apply the same `agent.workspace == self.cwd` filter to the guard at line 2264 that refuses the change while a Claude agent is still connecting, so a session in another worktree cannot block it. Update the notice at line 2307 to count only the agents that were reconnected.

- [ ] **Step 5: Verify**

Run: `cargo test -p agency-desktop && cargo clippy -p agency-desktop --all-targets`
Expected: PASS with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/agency-desktop/src/main.rs
git commit -m "feat(desktop): bind each agent session to its worktree"
```

---

### Task 4: Switching worktrees stops killing sessions

This is the bugfix: today `select_worktree` revokes every RPC capability and clears `agents`, so no two worktrees can run sessions at once.

**Files:**
- Modify: `crates/agency-desktop/src/workspaces.rs` (add `active_after_switch`)
- Modify: `crates/agency-desktop/src/main.rs:2492-2531` (`select_worktree`)
- Test: `crates/agency-desktop/src/workspaces.rs`

**Interfaces:**
- Consumes: `AgentView.workspace` from Task 3.
- Produces: `pub fn active_after_switch(agent_workspaces: &[PathBuf], cwd: &Path, current: Option<usize>) -> Option<usize>`.

- [ ] **Step 1: Write the failing test**

Add to the test module in `workspaces.rs`:

```rust
    /// Switching worktrees must move focus to a session that lives in the new
    /// worktree, exactly as the terminal list already re-selects by directory.
    #[test]
    fn switching_focuses_a_session_in_the_new_worktree() {
        let workspaces = vec![PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/b")];

        assert_eq!(
            active_after_switch(&workspaces, Path::new("/b"), Some(0)),
            Some(1)
        );
    }

    /// A switch that lands back on the focused session's own worktree leaves it
    /// focused rather than jumping to the first one in the list.
    #[test]
    fn switching_keeps_a_session_that_already_belongs_to_the_worktree() {
        let workspaces = vec![PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/b")];

        assert_eq!(
            active_after_switch(&workspaces, Path::new("/b"), Some(2)),
            Some(2)
        );
    }

    /// A worktree with no sessions focuses nothing — and, critically, the
    /// roster it was given is unchanged, because sessions elsewhere keep running.
    #[test]
    fn switching_to_an_empty_worktree_focuses_nothing() {
        let workspaces = vec![PathBuf::from("/a"), PathBuf::from("/b")];

        assert_eq!(
            active_after_switch(&workspaces, Path::new("/c"), Some(1)),
            None
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agency-desktop workspaces`
Expected: FAIL — `active_after_switch` is not defined.

- [ ] **Step 3: Implement**

Add to `workspaces.rs`:

```rust
/// Which session should be focused after the user switches to `cwd`, given the
/// worktree each running session belongs to. Sessions outside `cwd` keep
/// running; they are simply not the ones on screen.
pub fn active_after_switch(
    agent_workspaces: &[PathBuf],
    cwd: &Path,
    current: Option<usize>,
) -> Option<usize> {
    if current.is_some_and(|index| agent_workspaces.get(index).is_some_and(|w| w == cwd)) {
        return current;
    }
    agent_workspaces
        .iter()
        .position(|workspace| workspace == cwd)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agency-desktop workspaces`
Expected: PASS.

- [ ] **Step 5: Use it in `select_worktree`**

Replace the teardown block at `main.rs:2515-2519`:

```rust
        for agent in &self.agents {
            self.rpc_capabilities.revoke(&agent.rpc_token);
        }
        self.agents.clear();
        self.active_agent = None;
```

with:

```rust
        let agent_workspaces = self
            .agents
            .iter()
            .map(|agent| agent.workspace.clone())
            .collect::<Vec<_>>();
        self.active_agent =
            workspaces::active_after_switch(&agent_workspaces, &self.cwd, self.active_agent);
```

Leave the rest of `select_worktree` as it is: it still sets `cwd`, refreshes the slash catalogue, closes the overlay, and re-selects the terminal.

- [ ] **Step 6: Verify**

Run: `cargo test -p agency-desktop && cargo clippy -p agency-desktop --all-targets`
Expected: PASS with no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/agency-desktop/src/workspaces.rs crates/agency-desktop/src/main.rs
git commit -m "fix(desktop): keep sessions running when the worktree changes"
```

---

### Task 5: Declare the MCP tool

**Files:**
- Modify: `crates/agency-mcp/src/lib.rs` (`tools()` at line 73, `call_tool` at line 121)
- Test: `crates/agency-mcp/src/lib.rs` (new `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `fn rpc_method(name: &str) -> Option<&'static str>`, and a `start_worktree_session` entry in `tools()`.

- [ ] **Step 1: Write the failing test**

Add at the bottom of `crates/agency-mcp/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_worktree_session_is_declared_with_its_three_inputs() {
        let tools = tools();
        let tool = tools
            .as_array()
            .expect("tools should be an array")
            .iter()
            .find(|tool| tool["name"] == "start_worktree_session")
            .expect("start_worktree_session should be declared");

        let required = tool["inputSchema"]["required"]
            .as_array()
            .expect("the schema should require its inputs");

        assert!(required.iter().any(|name| name == "worktree"));
        assert!(required.iter().any(|name| name == "prompt"));
        assert!(required.iter().any(|name| name == "agent"));
    }

    #[test]
    fn every_declared_tool_maps_to_an_rpc_method() {
        for tool in tools().as_array().expect("tools should be an array") {
            let name = tool["name"].as_str().expect("a tool needs a name");
            assert!(rpc_method(name).is_some(), "{name} has no RPC method");
        }
        assert_eq!(
            rpc_method("start_worktree_session"),
            Some("worktree.start_session")
        );
    }

    #[test]
    fn an_unknown_tool_has_no_rpc_method() {
        assert_eq!(rpc_method("fabricated_tool"), None);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agency-mcp`
Expected: FAIL — `rpc_method` is not defined and the tool is not declared.

- [ ] **Step 3: Implement**

Add the entry to the array in `tools()`:

```rust
        {
            "name": "start_worktree_session",
            "description": "Start an agent session in an existing worktree with a first prompt. The session runs in parallel with yours and is the only way to get work done in another worktree.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "worktree": {
                        "type": "string",
                        "description": "Branch of the worktree the session should run in, as reported by list_worktrees."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "First message sent to the new session."
                    },
                    "agent": {
                        "type": "string",
                        "description": "Agent to run, such as \"claude\" or \"codex\"."
                    }
                },
                "required": ["worktree", "prompt", "agent"],
                "additionalProperties": false
            }
        }
```

Extract the mapping out of `call_tool` so it can be tested, and have `call_tool` use it:

```rust
fn rpc_method(name: &str) -> Option<&'static str> {
    match name {
        "list_worktrees" => Some("worktree.list"),
        "create_worktree" => Some("worktree.create"),
        "remove_worktree" => Some("worktree.remove"),
        "start_worktree_session" => Some("worktree.start_session"),
        _ => None,
    }
}
```

```rust
    let Some(method) = rpc_method(name) else {
        return rpc_error(id, -32602, format!("Unknown Agency tool: {name}"));
    };
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agency-mcp`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/agency-mcp/src/lib.rs
git commit -m "feat(mcp): declare start_worktree_session"
```

---

### Task 6: Resolve a start-session request

**Files:**
- Modify: `crates/agency-agents/src/lib.rs` (add `Provider::ALL` in the `impl Provider` block at line 59)
- Modify: `crates/agency-desktop/src/workspaces.rs`
- Test: `crates/agency-desktop/src/workspaces.rs`

**Interfaces:**
- Consumes: `crate::worktrees::Worktree`, `agency_agents::Provider`.
- Produces: `pub struct StartRequest { pub worktree: Worktree, pub provider: Provider, pub prompt: String }` and `pub fn resolve_start_request(params: &serde_json::Value, worktrees: &[Worktree]) -> Result<StartRequest, String>`.

- [ ] **Step 1: Write the failing test**

Add to the test module in `workspaces.rs`:

```rust
    fn worktree(branch: &str, path: &str) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            label: branch.to_owned(),
            branch: Some(branch.to_owned()),
        }
    }

    #[test]
    fn a_request_resolves_its_worktree_agent_and_prompt() {
        let worktrees = vec![worktree("main", "/repo"), worktree("feature", "/repo/wt")];

        let request = resolve_start_request(
            &serde_json::json!({
                "worktree": "feature",
                "prompt": "Add the parser",
                "agent": "claude-code"
            }),
            &worktrees,
        )
        .expect("the request should resolve");

        assert_eq!(request.worktree.path, PathBuf::from("/repo/wt"));
        assert_eq!(request.provider, Provider::Claude);
        assert_eq!(request.prompt, "Add the parser");
    }

    #[test]
    fn an_unknown_worktree_names_the_branches_that_exist() {
        let worktrees = vec![worktree("main", "/repo")];

        let error = resolve_start_request(
            &serde_json::json!({"worktree": "typo", "prompt": "Go", "agent": "claude"}),
            &worktrees,
        )
        .expect_err("an unknown branch should be refused");

        assert!(error.contains("typo"));
        assert!(error.contains("main"));
    }

    /// Resolution reads the agent name through `Provider::from_name`, so an
    /// agent Agency does not ship must fail loudly instead of defaulting to
    /// whichever provider happens to be first.
    #[test]
    fn a_fabricated_agent_name_is_refused_with_the_accepted_names() {
        let worktrees = vec![worktree("main", "/repo")];

        let error = resolve_start_request(
            &serde_json::json!({"worktree": "main", "prompt": "Go", "agent": "fabricated-agent"}),
            &worktrees,
        )
        .expect_err("an unknown agent should be refused");

        assert!(error.contains("fabricated-agent"));
        assert!(error.contains("claude"));
        assert!(error.contains("codex"));
    }

    #[test]
    fn a_blank_prompt_is_refused_before_anything_spawns() {
        let worktrees = vec![worktree("main", "/repo")];

        let error = resolve_start_request(
            &serde_json::json!({"worktree": "main", "prompt": "   ", "agent": "claude"}),
            &worktrees,
        )
        .expect_err("a blank prompt should be refused");

        assert!(error.contains("prompt"));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agency-desktop workspaces`
Expected: FAIL — `resolve_start_request` is not defined.

- [ ] **Step 3: Implement**

In `crates/agency-agents/src/lib.rs`, inside `impl Provider`:

```rust
    /// Every agent Agency knows how to start. Used where a caller has to be
    /// told which names are accepted.
    pub const ALL: [Self; 2] = [Self::Codex, Self::Claude];
```

In `workspaces.rs`:

```rust
use agency_agents::Provider;
use serde_json::Value;

use crate::worktrees::Worktree;

pub struct StartRequest {
    pub worktree: Worktree,
    pub provider: Provider,
    pub prompt: String,
}

/// Turns `start_worktree_session` arguments into something startable, or into
/// the error the caller sees. Every refusal happens here, before a process is
/// spawned, so a mistyped branch or agent costs nothing.
pub fn resolve_start_request(
    params: &Value,
    worktrees: &[Worktree],
) -> Result<StartRequest, String> {
    let branch = params
        .get("worktree")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .ok_or_else(|| "start_worktree_session requires a worktree".to_owned())?;
    let worktree = worktrees
        .iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch))
        .cloned()
        .ok_or_else(|| {
            let known = worktrees
                .iter()
                .filter_map(|worktree| worktree.branch.as_deref())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "No worktree is checked out on {branch}. Create it first with create_worktree. \
Worktrees that exist: {known}"
            )
        })?;

    let agent = params
        .get("agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .ok_or_else(|| "start_worktree_session requires an agent".to_owned())?;
    let provider = Provider::from_name(agent).ok_or_else(|| {
        let known = Provider::ALL
            .iter()
            .map(|provider| provider.command())
            .collect::<Vec<_>>()
            .join(", ");
        format!("Unknown agent {agent}. Agents Agency can start: {known}")
    })?;

    let prompt = params
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .ok_or_else(|| "start_worktree_session requires a prompt".to_owned())?
        .to_owned();

    Ok(StartRequest {
        worktree,
        provider,
        prompt,
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agency-desktop workspaces && cargo test -p agency-agents`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/agency-agents/src/lib.rs crates/agency-desktop/src/workspaces.rs
git commit -m "feat(desktop): resolve start_worktree_session arguments"
```

---

### Task 7: Handle `worktree.start_session`

**Files:**
- Modify: `crates/agency-desktop/src/main.rs` (`AppEvent` enum around line 866; the reducer match around line 1300; `handle_rpc_calls` at line 2708)

**Interfaces:**
- Consumes: `resolve_start_request` (Task 6), `start_session_in` (Task 3).
- Produces: `AppEvent::WorktreeSessionStarted { conversation_id: String, worktree: Worktree, provider: Provider }`.

- [ ] **Step 1: Add the event**

In the `AppEvent` enum, beside `WorktreeCreated` and `WorktreeRemoved`:

```rust
    /// A tool call started a session in `worktree`. Focus does not move: the
    /// caller keeps its own worktree, and the notice is how the user learns
    /// where the new session is.
    WorktreeSessionStarted {
        conversation_id: String,
        worktree: Worktree,
        provider: Provider,
    },
```

Add the reducer arm beside the other worktree arms (around line 1303):

```rust
            AppEvent::WorktreeSessionStarted {
                conversation_id,
                worktree,
                provider,
            } => {
                self.notice = Some(format!(
                    "Started {} session {conversation_id} in {}",
                    provider.label(),
                    worktree.label
                ));
            }
```

- [ ] **Step 2: Add the handler**

In `handle_rpc_calls`, after the `"worktree.remove"` arm:

```rust
                "worktree.start_session" => worktrees::discover(&call.context.workspace)
                    .and_then(|worktrees| {
                        workspaces::resolve_start_request(&call.params, &worktrees)
                    })
                    .and_then(|request| {
                        let workspace = request.worktree.path.clone();
                        let conversation_id = self.start_session_in(
                            request.provider,
                            workspace,
                            Some(request.prompt),
                        )?;
                        let worktree = worktree_json(request.worktree.clone());
                        self.emit(AppEvent::WorktreeSessionStarted {
                            conversation_id: conversation_id.clone(),
                            worktree: request.worktree,
                            provider: request.provider,
                        });
                        Ok(serde_json::json!({
                            "caller": rpc_caller(&call.context),
                            "session": {
                                "conversation_id": conversation_id,
                                "worktree": worktree,
                                "agent": request.provider.command()
                            }
                        }))
                    }),
```

- [ ] **Step 3: Verify it compiles and the suite is green**

Run: `cargo test -p agency-desktop && cargo clippy -p agency-desktop --all-targets`
Expected: PASS with no warnings. The resolution behaviour this arm depends on is covered by Task 6's tests; the spawn effect itself is verified by hand in Task 10.

- [ ] **Step 4: Commit**

```bash
git add crates/agency-desktop/src/main.rs
git commit -m "feat(desktop): start a session in a worktree from a tool call"
```

---

### Task 8: Refuse to remove a worktree that is running a session

**Files:**
- Modify: `crates/agency-desktop/src/workspaces.rs` (add `has_live_session`)
- Modify: `crates/agency-desktop/src/main.rs` (`"worktree.remove"` arm at line 2740)
- Modify: `crates/agency-mcp/src/lib.rs` (the `remove_worktree` description at line 105)
- Test: `crates/agency-desktop/src/workspaces.rs`

**Interfaces:**
- Produces: `pub fn has_live_session(agent_workspaces: &[PathBuf], target: &Path) -> bool`.

- [ ] **Step 1: Write the failing test**

```rust
    /// Sessions outlive a worktree switch now, so removing a worktree could
    /// otherwise delete the directory a running agent is working in.
    #[test]
    fn a_worktree_running_a_session_is_detected() {
        let workspaces = vec![PathBuf::from("/repo"), PathBuf::from("/repo/wt")];

        assert!(has_live_session(&workspaces, Path::new("/repo/wt")));
        assert!(!has_live_session(&workspaces, Path::new("/repo/other")));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agency-desktop workspaces`
Expected: FAIL — `has_live_session` is not defined.

- [ ] **Step 3: Implement**

In `workspaces.rs`:

```rust
/// Whether any running session belongs to `target`.
pub fn has_live_session(agent_workspaces: &[PathBuf], target: &Path) -> bool {
    agent_workspaces.iter().any(|workspace| workspace == target)
}
```

In the `"worktree.remove"` arm, before calling `worktrees::remove`, resolve the target and refuse:

```rust
                    branch.and_then(|branch| {
                        let target = worktrees::discover(&call.context.workspace)?
                            .into_iter()
                            .find(|worktree| worktree.branch.as_deref() == Some(branch))
                            .ok_or_else(|| format!("No worktree is checked out on {branch}"))?;
                        let agent_workspaces = self
                            .agents
                            .iter()
                            .map(|agent| agent.workspace.clone())
                            .collect::<Vec<_>>();
                        if workspaces::has_live_session(&agent_workspaces, &target.path) {
                            return Err(format!(
                                "{branch} is running an Agency session and cannot be removed"
                            ));
                        }
                        worktrees::remove(&call.context.workspace, branch).map(|worktree| {
                            // ...unchanged body
                        })
                    })
```

Update the `remove_worktree` description in `crates/agency-mcp/src/lib.rs:105` to end with: `Refuses a worktree with uncommitted changes or a running session.`

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agency-desktop && cargo test -p agency-mcp && cargo clippy -p agency-desktop --all-targets`
Expected: PASS with no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/agency-desktop/src/workspaces.rs crates/agency-desktop/src/main.rs crates/agency-mcp/src/lib.rs
git commit -m "fix(desktop): refuse to remove a worktree with a running session"
```

---

### Task 9: Inject the worktree directive

**Files:**
- Modify: `crates/agency-agents/src/lib.rs:841-853` (`agency_harness_context`)
- Test: `crates/agency-agents/src/lib.rs:1112-1124` (extend `harness_context_identifies_the_agency_session`)

**Interfaces:**
- Consumes: nothing new. Both providers already read this string — Claude via `--append-system-prompt` (line 598), Codex via `developerInstructions` (lines 453 and 461).

- [ ] **Step 1: Write the failing test**

Add beside the existing harness-context test:

```rust
    /// The directive is the only thing stopping an agent from wandering into
    /// another worktree, and both providers read it from this one string.
    #[test]
    fn harness_context_sends_cross_worktree_work_through_the_tool() {
        let environment = vec![(
            ENV_CONVERSATION_ID.to_owned(),
            "conversation-123".to_owned(),
        )];

        let context = agency_harness_context(&environment).unwrap();

        assert!(context.contains("start_worktree_session"));
        assert!(context.contains("bound to the worktree"));
        assert!(context.contains("never `cd` into one"));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agency-agents harness_context`
Expected: FAIL — the context has no worktree directive.

- [ ] **Step 3: Implement**

Extend the formatted string in `agency_harness_context`:

```rust
    Some(format!(
        "You are running inside the Agency agentic coding harness. \
Your Agency session ID is `{conversation_id}`. Agency's MCP tools are \
session-scoped and automatically act on this session and its workspace; \
do not ask the user for a session ID when calling them. \
Your session is bound to the worktree it was started in, and that worktree is \
its working directory. Do not move to another worktree: never `cd` into one and \
never use a provider-native worktree-entry or worktree-switch tool. To get work \
done in a different worktree, call `start_worktree_session` with that \
worktree's branch, a prompt, and an agent — it starts a separate session there \
that runs in parallel with yours."
    ))
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agency-agents`
Expected: PASS, including the existing `harness_context_identifies_the_agency_session`.

- [ ] **Step 5: Commit**

```bash
git add crates/agency-agents/src/lib.rs
git commit -m "feat(agents): direct cross-worktree work through start_worktree_session"
```

---

### Task 10: Align repository guidance, then verify by hand

**Files:**
- Modify: `CLAUDE.md` ("Work in a worktree" section)

**Interfaces:**
- Consumes: the directive from Task 9. The wording here must not contradict it.

- [ ] **Step 1: Rewrite the two bullets that tell agents to enter a worktree**

Replace the second bullet ("Creating the worktree is a pre-condition of the `superpowers:brainstorming` skill…") with:

```markdown
- Creating the worktree is a pre-condition of the `superpowers:brainstorming`
  skill. When that skill is invoked, create the feature's worktree and its
  branch first, then hand the work to it with `start_worktree_session`, so the
  design conversation, any notes or specs it produces, and the implementation
  that follows all live on the same branch.
```

Replace the bullet beginning "Create and manage worktrees with Agency's worktree tools" with:

```markdown
- Create and manage worktrees with Agency's worktree tools, which are
  session-scoped; never ask the user for an Agency session ID and never fall
  back to raw `git worktree` commands when a tool covers the operation.
- A session is bound to the worktree it was started in. `start_worktree_session`
  is the only way into another worktree: never `cd` into one and never use a
  provider-native worktree-entry tool. The session it starts runs in parallel,
  and the user reaches it from that worktree's tab.
```

- [ ] **Step 2: Verify the whole suite**

Run: `cargo fmt --check && cargo clippy --all-targets && cargo test`
Expected: PASS with no warnings across every crate.

- [ ] **Step 3: Verify the spawn path by hand**

The spawn-and-send path cannot be unit tested — no test constructs a running `Agency`, because spawning needs real agent processes. Run the application from the worktree and confirm, in order:

1. `cargo run -p agency-desktop` from the worktree root.
2. In a session, call `create_worktree` for a throwaway branch, then `start_worktree_session` with that branch, a short prompt, and `"claude"`. The call returns a conversation ID; the calling session keeps its focus and its worktree.
3. A notice names the started session and its worktree.
4. Switch to that worktree's tab: the new session is listed and has the prompt in its transcript.
5. Switch back: the original session is still there, still running.
6. Call `remove_worktree` on the branch while its session runs: it is refused, naming the branch.
7. Call `start_worktree_session` with a branch that has no worktree, then with `"fabricated-agent"`: both are refused with the branch list and the agent list.

Record anything that deviates rather than fixing it silently.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: send cross-worktree work through start_worktree_session"
```
