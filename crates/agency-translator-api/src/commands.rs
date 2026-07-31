//! The provider-neutral vocabulary for the commands an agent can run.
//!
//! Agents disagree on almost everything here: where commands live, whether
//! plugins namespace them, and what sigil invokes one. A translator resolves
//! all of that and reports [`AgentCommand`]s, so the composer can list every
//! agent's commands without knowing anything about either.

use std::path::Path;

/// Where a command came from, which is what decides how it is labelled and
/// which of two same-named commands wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOrigin {
    /// Shipped with the agent itself.
    BuiltIn,
    /// The user's own, available in every workspace.
    Personal,
    /// Checked into the workspace.
    Project,
    /// Supplied by an installed plugin, which namespaces it.
    Plugin { plugin: String, marketplace: String },
}

impl CommandOrigin {
    pub fn is_built_in(&self) -> bool {
        matches!(self, Self::BuiltIn)
    }

    pub fn is_plugin(&self) -> bool {
        matches!(self, Self::Plugin { .. })
    }
}

/// One command an agent can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    /// Fully qualified, without a leading sigil: `superpowers:brainstorming`.
    pub name: String,
    pub description: String,
    /// Exactly what gets typed at the agent, sigil included, with a trailing
    /// space when the command takes arguments.
    pub invocation: String,
    pub argument_hint: Option<String>,
    pub origin: CommandOrigin,
}

/// An agent's answer to "what can I run here?".
pub trait CommandCatalog: Send + Sync {
    fn commands(&self, workspace: &Path) -> Vec<AgentCommand>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct Fixed;

    impl CommandCatalog for Fixed {
        fn commands(&self, _workspace: &Path) -> Vec<AgentCommand> {
            vec![AgentCommand {
                name: "superpowers:brainstorming".to_owned(),
                description: "Turn ideas into designs".to_owned(),
                invocation: "/superpowers:brainstorming ".to_owned(),
                argument_hint: Some("[topic]".to_owned()),
                origin: CommandOrigin::Plugin {
                    plugin: "superpowers".to_owned(),
                    marketplace: "superpowers-marketplace".to_owned(),
                },
            }]
        }
    }

    #[test]
    fn a_catalog_is_object_safe_and_reports_its_commands() {
        let catalog: Box<dyn CommandCatalog> = Box::new(Fixed);
        let commands = catalog.commands(Path::new("/workspace"));
        assert_eq!(commands[0].name, "superpowers:brainstorming");
        assert!(commands[0].origin.is_plugin());
    }

    #[test]
    fn only_a_built_in_reports_itself_as_one() {
        assert!(CommandOrigin::BuiltIn.is_built_in());
        assert!(!CommandOrigin::Personal.is_built_in());
        assert!(!CommandOrigin::Personal.is_plugin());
    }
}
