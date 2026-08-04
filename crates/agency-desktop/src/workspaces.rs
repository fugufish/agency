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

#[cfg(test)]
mod tests {
    use super::*;
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
}
