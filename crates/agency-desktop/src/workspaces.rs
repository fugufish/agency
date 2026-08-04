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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::path_component;
    use agency_agents::{McpTransport, Provider};

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
