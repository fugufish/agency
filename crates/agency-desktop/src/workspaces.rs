use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agency_agents::{McpServer, Provider};
use serde_json::Value;

use crate::sessions::SessionRegistry;
use crate::worktrees::Worktree;

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
    ///
    /// A load failure still inserts a state — scoped to `workspace` with an
    /// empty registry — rather than leaving the worktree unensured. Callers
    /// that only capture the error for a notice (startup, worktree switching)
    /// must not leave `self.sessions()` / `self.mcp_servers()` resolving
    /// against the shared fallback, which is scoped to nothing on disk.
    pub fn ensure(&mut self, workspace: &Path) -> Result<(), String> {
        if self.states.contains_key(workspace) {
            return Ok(());
        }
        let (registry, result) = match SessionRegistry::load(workspace) {
            Ok(registry) => (registry, Ok(())),
            Err(error) => (SessionRegistry::empty(workspace), Err(error)),
        };
        self.states.insert(
            workspace.to_path_buf(),
            WorktreeState {
                registry,
                mcp_servers: Vec::new(),
            },
        );
        result
    }

    pub fn state(&self, workspace: &Path) -> &WorktreeState {
        self.states.get(workspace).unwrap_or(&self.fallback)
    }

    pub fn state_mut(&mut self, workspace: &Path) -> &mut WorktreeState {
        self.states
            .entry(workspace.to_path_buf())
            .or_insert_with(|| WorktreeState {
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

#[derive(Debug)]
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

/// Which sessions in the roster belong to `target`.
///
/// `self.agents` holds every worktree's sessions at once now, so every scan
/// that means "the sessions in this worktree" — the busy gate and MCP badges
/// for the worktree on screen, the eviction a removal performs — has to go
/// through this one rule rather than scanning the whole vector. Indices come
/// back ascending, so a caller removing them can walk the result in reverse.
pub fn sessions_in_worktree(agent_workspaces: &[PathBuf], target: &Path) -> Vec<usize> {
    agent_workspaces
        .iter()
        .enumerate()
        .filter(|(_, workspace)| workspace.as_path() == target)
        .map(|(index, _)| index)
        .collect()
}

/// The focused session after the sessions at `removed` leave the roster.
///
/// `agent_workspaces` is the roster *before* removal and the answer indexes the
/// roster *after* it, because every surviving index at or past a removal
/// shifts. Shifting alone is not enough: the roster spans every worktree, so an
/// index that merely stays in range can land on a session in a worktree the
/// user is not looking at, while the rest of the UI still names `cwd`. Focus is
/// therefore re-decided with `active_after_switch` over the survivors, which
/// keeps the previously focused session when it survived and belongs to `cwd`.
pub fn active_after_removal(
    agent_workspaces: &[PathBuf],
    removed: &[usize],
    cwd: &Path,
    current: Option<usize>,
) -> Option<usize> {
    let survivors = agent_workspaces
        .iter()
        .enumerate()
        .filter(|(index, _)| !removed.contains(index))
        .collect::<Vec<_>>();
    let shifted =
        current.and_then(|current| survivors.iter().position(|(index, _)| *index == current));
    let surviving_workspaces = survivors
        .into_iter()
        .map(|(_, workspace)| workspace.clone())
        .collect::<Vec<_>>();
    active_after_switch(&surviving_workspaces, cwd, shifted)
}

/// One running session, reduced to what removing a worktree has to decide
/// about: where it runs, and whether it is still doing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningSession {
    pub workspace: PathBuf,
    /// The session is mid-turn, waiting on the user, or holding messages it has
    /// not sent yet.
    pub working: bool,
}

/// Whether a session in `target` is still working.
///
/// Deliberately not "a session exists here". Nothing removes an `AgentView`
/// when a session finishes — agents carry no process-exit event — so a session
/// that merely *ran* here once would block removal of its worktree forever,
/// and the agent that delegated the work would have no way to clear it: it
/// cannot trash another worktree's session, and it must not enter that
/// worktree. Only work actually in flight is worth refusing for, because that
/// refusal clears itself.
pub fn has_working_session(sessions: &[RunningSession], target: &Path) -> bool {
    sessions
        .iter()
        .any(|session| session.working && session.workspace.as_path() == target)
}

/// Whether `target` may be removed, given which worktree is primary and
/// what its sessions are doing.
///
/// The primary worktree is refused unconditionally by `worktrees::remove`
/// itself, with a message naming that as the reason. A working session running
/// in the primary worktree too must not shadow that permanent refusal with
/// the transient, fixable-sounding "session is working" one — so this
/// returns `true` for the primary regardless of what is running there,
/// deferring the decision to `worktrees::remove`.
pub fn may_remove(target: &Path, primary: &Path, sessions: &[RunningSession]) -> bool {
    target == primary || !has_working_session(sessions, target)
}

/// The refusal a caller sees when work is still in flight in the worktree it
/// asked to remove. It has to name the remedy, because the condition is
/// temporary: the caller waits for the session to go idle and asks again.
pub fn working_session_refusal(branch: &str) -> String {
    format!(
        "An Agency session is still working in {branch}. Wait for it to go idle, then remove the \
worktree again."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::path_component;
    use agency_agents::McpTransport;

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
        workspaces
            .ensure(&first)
            .expect("could not load the first worktree");
        workspaces
            .ensure(&second)
            .expect("could not load the second worktree");

        workspaces
            .state_mut(&first)
            .registry
            .record(
                Provider::Claude,
                "claude-1".to_owned(),
                Some("First".to_owned()),
            )
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
        workspaces
            .ensure(&first)
            .expect("could not load the first worktree");
        workspaces
            .ensure(&second)
            .expect("could not load the second worktree");

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

        assert!(
            workspaces
                .state(Path::new("/nonexistent/worktree"))
                .registry
                .records()
                .is_empty()
        );
    }

    /// Switching worktrees must move focus to a session that lives in the new
    /// worktree, exactly as the terminal list already re-selects by directory.
    #[test]
    fn switching_focuses_a_session_in_the_new_worktree() {
        let workspaces = vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/b"),
        ];

        assert_eq!(
            active_after_switch(&workspaces, Path::new("/b"), Some(0)),
            Some(1)
        );
    }

    /// A switch that lands back on the focused session's own worktree leaves it
    /// focused rather than jumping to the first one in the list.
    #[test]
    fn switching_keeps_a_session_that_already_belongs_to_the_worktree() {
        let workspaces = vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/b"),
        ];

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

    /// The filtering rule every "sessions in this worktree" scan shares.
    /// `self.agents` spans every worktree now, so a scan that forgets to filter
    /// reports another worktree's sessions as if they were on screen.
    #[test]
    fn only_the_sessions_of_one_worktree_are_selected() {
        let workspaces = vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/a"),
        ];

        assert_eq!(
            sessions_in_worktree(&workspaces, Path::new("/a")),
            vec![0, 2]
        );
        assert_eq!(sessions_in_worktree(&workspaces, Path::new("/b")), vec![1]);
        assert!(sessions_in_worktree(&workspaces, Path::new("/c")).is_empty());
    }

    /// A prefix is not a match: a worktree nested under another must not drag
    /// the parent's sessions into its own scans.
    #[test]
    fn a_nested_worktree_does_not_claim_its_parents_sessions() {
        let workspaces = vec![PathBuf::from("/repo"), PathBuf::from("/repo/wt")];

        assert_eq!(
            sessions_in_worktree(&workspaces, Path::new("/repo")),
            vec![0]
        );
    }

    /// The regression test for focus after trashing a session: plain index
    /// arithmetic over a roster that spans worktrees could focus a session in
    /// another worktree while every other element still named `cwd`, so typing
    /// would go to an agent the user cannot see.
    #[test]
    fn trashing_the_focused_session_keeps_focus_inside_the_worktree() {
        let workspaces = vec![PathBuf::from("/a"), PathBuf::from("/b")];

        // The user is in /b, focused on the only session there, and trashes it.
        assert_eq!(
            active_after_removal(&workspaces, &[1], Path::new("/b"), Some(1)),
            None,
            "no session is left in the worktree on screen, so nothing is focused"
        );
    }

    /// Trashing one of several sessions in the worktree on screen falls back to
    /// a sibling in that same worktree, not to whichever index survives.
    #[test]
    fn trashing_a_session_falls_back_to_a_sibling_in_the_same_worktree() {
        let workspaces = vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/b"),
        ];

        assert_eq!(
            active_after_removal(&workspaces, &[2], Path::new("/b"), Some(2)),
            Some(1)
        );
    }

    /// Removing a session ahead of the focused one shifts its index down; the
    /// same session must stay focused rather than the roster sliding under it.
    #[test]
    fn removing_an_earlier_session_shifts_the_focused_index_down() {
        let workspaces = vec![
            PathBuf::from("/b"),
            PathBuf::from("/a"),
            PathBuf::from("/b"),
        ];

        assert_eq!(
            active_after_removal(&workspaces, &[0], Path::new("/b"), Some(2)),
            Some(1)
        );
    }

    /// Several sessions can leave the roster at once — a worktree removal
    /// evicts every session that belonged to it. Focus has to survive that as a
    /// group, not one removal at a time.
    #[test]
    fn removing_a_group_of_sessions_leaves_focus_on_the_worktree_on_screen() {
        let workspaces = vec![
            PathBuf::from("/a"),
            PathBuf::from("/wt"),
            PathBuf::from("/wt"),
            PathBuf::from("/a"),
        ];
        let removed = sessions_in_worktree(&workspaces, Path::new("/wt"));

        assert_eq!(
            active_after_removal(&workspaces, &removed, Path::new("/a"), Some(3)),
            Some(1),
            "the focused session survives, re-indexed after the removals ahead of it"
        );
    }

    fn session(workspace: &str, working: bool) -> RunningSession {
        RunningSession {
            workspace: PathBuf::from(workspace),
            working,
        }
    }

    /// Sessions outlive a worktree switch now, so removing a worktree could
    /// otherwise delete the directory a running agent is working in.
    #[test]
    fn a_worktree_running_a_working_session_is_detected() {
        let sessions = vec![session("/repo", true), session("/repo/wt", true)];

        assert!(has_working_session(&sessions, Path::new("/repo/wt")));
        assert!(!has_working_session(&sessions, Path::new("/repo/other")));
    }

    /// The bug that replaced the old "a session lives here" guard: nothing
    /// drops an `AgentView` when its session finishes, so a worktree that had
    /// ever hosted a session was un-removable forever — breaking the
    /// delegate-then-clean-up workflow `start_worktree_session` exists for.
    #[test]
    fn a_session_that_has_stopped_working_does_not_count_as_working() {
        let sessions = vec![session("/repo/wt", false)];

        assert!(!has_working_session(&sessions, Path::new("/repo/wt")));
    }

    /// A working session outside the primary worktree blocks removal — this is
    /// the ordinary case the guard exists for.
    #[test]
    fn a_non_primary_worktree_with_a_working_session_may_not_be_removed() {
        let sessions = vec![session("/repo", true), session("/repo/wt", true)];

        assert!(!may_remove(
            Path::new("/repo/wt"),
            Path::new("/repo"),
            &sessions
        ));
    }

    /// An idle session is not a reason to keep a worktree. Removal is
    /// deliberate and `worktrees::remove` already refuses uncommitted changes,
    /// so the session ends with the directory it was working in.
    #[test]
    fn a_worktree_whose_session_went_idle_may_be_removed() {
        let sessions = vec![session("/repo/wt", false)];

        assert!(may_remove(
            Path::new("/repo/wt"),
            Path::new("/repo"),
            &sessions
        ));
    }

    /// A worktree nobody is running a session in may be removed.
    #[test]
    fn a_worktree_with_no_session_may_be_removed() {
        let sessions = vec![session("/repo", true)];

        assert!(may_remove(
            Path::new("/repo/wt"),
            Path::new("/repo"),
            &sessions
        ));
    }

    /// The motivating case: a session happens to be working in the primary
    /// worktree too. `worktrees::remove` refuses the primary unconditionally
    /// and permanently, so `may_remove` must not claim the working session is
    /// the reason removal fails — it defers to that refusal instead.
    #[test]
    fn a_primary_worktree_with_a_working_session_defers_to_the_primary_refusal() {
        let sessions = vec![session("/repo", true)];

        assert!(may_remove(
            Path::new("/repo"),
            Path::new("/repo"),
            &sessions
        ));
    }

    /// The refusal is temporary, so it has to tell the caller how to get past
    /// it. A message that only says "cannot be removed" reads as permanent and
    /// leaves a delegating agent with nothing to try.
    #[test]
    fn the_refusal_names_the_branch_and_the_remedy() {
        let refusal = working_session_refusal("feature");

        assert!(refusal.contains("feature"));
        assert!(refusal.contains("idle"));
    }

    /// Removing the worktree the user is looking at leaves nothing focused
    /// here; `SelectWorktree` re-picks once `cwd` has moved.
    #[test]
    fn evicting_the_worktree_on_screen_focuses_nothing() {
        let workspaces = vec![PathBuf::from("/wt"), PathBuf::from("/a")];
        let evicted = sessions_in_worktree(&workspaces, Path::new("/wt"));

        assert_eq!(
            active_after_removal(&workspaces, &evicted, Path::new("/wt"), Some(0)),
            None
        );
    }

    /// A load failure (I/O error, corrupt session file) must not leave the
    /// worktree unensured: callers that only capture the error for a notice
    /// (startup, worktree switching) still need every later read to resolve
    /// against a registry scoped to this workspace, not the shared fallback
    /// (whose `session_directory` resolves against the process cwd instead
    /// of the worktree).
    #[test]
    fn a_failed_load_still_scopes_the_registry_to_its_workspace() {
        let workspace = temp_workspace("load-failure");
        // `SessionRegistry::load` reads the sessions directory with
        // `fs::read_dir`; putting a plain file where that directory is
        // expected makes the read fail reliably and cheaply.
        let sessions_directory = crate::sessions::worktree_sessions_directory(&workspace);
        std::fs::create_dir_all(
            sessions_directory
                .parent()
                .expect("the sessions directory always has a parent"),
        )
        .expect("could not create the workspace config directory");
        std::fs::write(&sessions_directory, "not a directory")
            .expect("could not occupy the sessions directory with a file");

        let mut workspaces = Workspaces::new();
        let result = workspaces.ensure(&workspace);

        assert!(result.is_err(), "a failed load must still report its error");
        assert_eq!(
            workspaces
                .state(&workspace)
                .registry
                .session_directory("conversation"),
            sessions_directory.join(path_component("conversation")),
            "the registry inserted for a failed load must stay scoped to its workspace"
        );
    }
}
