use std::path::Path;

use agency_agents::Provider;
use agency_mux::CommandSpec;

use crate::terminal::{HeadlessOutcome, HeadlessTerminal};

/// What one `/plugin install <source>` asks an agent to do. Registering a
/// marketplace and installing a plugin from an already registered marketplace
/// are separate operations in both agent CLIs, so Agency picks between them
/// from the shape of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// A marketplace source: an HTTPS or SSH Git URL, a relative or absolute
    /// local path, or `owner/repo[@ref]`.
    Marketplace,
    /// A plugin selector: `plugin` or `plugin@marketplace`.
    Plugin,
}

impl InstallKind {
    /// How the operation reads in a notice, as a verb phrase.
    pub fn describe(self, source: &str) -> String {
        match self {
            Self::Marketplace => format!("marketplace source {source}"),
            Self::Plugin => format!("plugin {source}"),
        }
    }
}

/// Classifies what a user typed after `/plugin install`. Marketplace sources
/// are the only sources that carry a path separator, a scheme, or an SSH
/// prefix; a plugin selector is a bare name optionally qualified by the
/// marketplace it comes from. A local marketplace directory must therefore be
/// written as a path (`./marketplace`), matching how both agent CLIs document
/// their own marketplace sources.
pub fn install_kind(source: &str) -> InstallKind {
    let looks_like_marketplace = source.contains("://")
        || source.starts_with("git@")
        || source.starts_with('~')
        || source.contains('/')
        || source.ends_with(".git");
    if looks_like_marketplace {
        InstallKind::Marketplace
    } else {
        InstallKind::Plugin
    }
}

/// The command Agency runs for one agent. Both agents register marketplaces
/// with `plugin marketplace add`, but they spell the install step differently:
/// Claude Code uses `plugin install`, Codex uses `plugin add`. Running the
/// wrong verb fails, so the subcommand is resolved per provider rather than
/// shared.
pub fn install_command(provider: Provider, source: &str) -> CommandSpec {
    match install_kind(source) {
        InstallKind::Marketplace => {
            CommandSpec::new(provider.command(), ["plugin", "marketplace", "add", source])
        }
        InstallKind::Plugin => CommandSpec::new(
            provider.command(),
            ["plugin", install_verb(provider), source],
        ),
    }
}

/// The subcommand each agent uses to install a plugin from a registered
/// marketplace.
fn install_verb(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "install",
        Provider::Codex => "add",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStatus {
    Running,
    Installed,
    Failed,
}

impl InstallStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Installing",
            Self::Installed => "Installed",
            Self::Failed => "Failed",
        }
    }
}

/// Lifecycle of one headless plugin install, published to the application
/// event bus so every interested facet reduces the same typed events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginInstallEvent {
    Started {
        id: u64,
        conversation_id: String,
        provider: Provider,
        command: String,
    },
    Output {
        id: u64,
        conversation_id: String,
        provider: Provider,
        output: String,
    },
    Finished {
        id: u64,
        conversation_id: String,
        provider: Provider,
        kind: InstallKind,
        status: InstallStatus,
        detail: Option<String>,
    },
}

impl PluginInstallEvent {
    pub fn conversation_id(&self) -> &str {
        match self {
            Self::Started {
                conversation_id, ..
            }
            | Self::Output {
                conversation_id, ..
            }
            | Self::Finished {
                conversation_id, ..
            } => conversation_id,
        }
    }
}

/// Owns the headless terminals running plugin installs. Spawning a process and
/// reading its output are effects, so the facet only ever hands typed events
/// back to the caller to publish.
#[derive(Default)]
pub struct PluginInstalls {
    runs: Vec<Run>,
    next_id: u64,
}

struct Run {
    id: u64,
    conversation_id: String,
    provider: Provider,
    kind: InstallKind,
    terminal: HeadlessTerminal,
    output: String,
}

impl PluginInstalls {
    /// Starts one headless install per target agent.
    pub fn start(
        &mut self,
        conversation_id: &str,
        source: &str,
        targets: &[Provider],
        cwd: &Path,
    ) -> Vec<PluginInstallEvent> {
        let mut events = Vec::new();
        let kind = install_kind(source);

        for provider in targets.iter().copied() {
            self.spawn(
                conversation_id,
                provider,
                kind,
                install_command(provider, source),
                cwd,
                &mut events,
            );
        }

        events
    }

    /// Starts one headless command, reporting the install through `events`
    /// whether or not the agent could be started.
    fn spawn(
        &mut self,
        conversation_id: &str,
        provider: Provider,
        kind: InstallKind,
        command: CommandSpec,
        cwd: &Path,
        events: &mut Vec<PluginInstallEvent>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        events.push(PluginInstallEvent::Started {
            id,
            conversation_id: conversation_id.to_owned(),
            provider,
            command: command.display(),
        });

        match HeadlessTerminal::spawn(command, cwd) {
            Ok(terminal) => self.runs.push(Run {
                id,
                conversation_id: conversation_id.to_owned(),
                provider,
                kind,
                terminal,
                output: String::new(),
            }),
            Err(error) => events.push(PluginInstallEvent::Finished {
                id,
                conversation_id: conversation_id.to_owned(),
                provider,
                kind,
                status: InstallStatus::Failed,
                detail: Some(error),
            }),
        }

        id
    }

    /// Drains streamed output and reports installs that have finished.
    pub fn poll(&mut self) -> Vec<PluginInstallEvent> {
        let mut events = Vec::new();

        for run in &mut self.runs {
            run.terminal.poll();
            if run.terminal.output() != run.output {
                run.output = run.terminal.output().to_owned();
                events.push(PluginInstallEvent::Output {
                    id: run.id,
                    conversation_id: run.conversation_id.clone(),
                    provider: run.provider,
                    output: run.output.clone(),
                });
            }
            let Some(outcome) = run.terminal.outcome() else {
                continue;
            };
            let (status, detail) = match outcome {
                HeadlessOutcome::Exited(exit) if exit.success => (InstallStatus::Installed, None),
                HeadlessOutcome::Exited(exit) => (
                    InstallStatus::Failed,
                    Some(match exit.code {
                        Some(code) => format!("{} exited with status {code}", run.provider.label()),
                        None => format!("{} exited abnormally", run.provider.label()),
                    }),
                ),
                HeadlessOutcome::Failed(error) => (InstallStatus::Failed, Some(error.clone())),
            };
            events.push(PluginInstallEvent::Finished {
                id: run.id,
                conversation_id: run.conversation_id.clone(),
                provider: run.provider,
                kind: run.kind,
                status,
                detail,
            });
        }

        self.runs.retain(|run| run.terminal.outcome().is_none());
        events
    }
}

/// One plugin install as it appears in a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstallEntry {
    pub id: u64,
    pub provider: Provider,
    pub command: String,
    pub output: String,
    pub status: InstallStatus,
    pub detail: Option<String>,
    /// Conversation events recorded before the install started, so the entry
    /// keeps its place when the transcript is rebuilt.
    pub after_events: usize,
}

/// The transcript-side facet of plugin installs. It reduces the same events
/// the runner publishes and owns nothing else.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TranscriptInstalls {
    entries: Vec<PluginInstallEntry>,
}

impl TranscriptInstalls {
    /// Reduces one install event, reporting whether the transcript changed.
    /// `recorded_events` is the number of conversation events already in the
    /// transcript, which fixes where a starting install is rendered.
    pub fn on_event(&mut self, event: &PluginInstallEvent, recorded_events: usize) -> bool {
        match event {
            PluginInstallEvent::Started {
                id,
                provider,
                command,
                ..
            } => {
                if self.entries.iter().any(|entry| entry.id == *id) {
                    return false;
                }
                self.entries.push(PluginInstallEntry {
                    id: *id,
                    provider: *provider,
                    command: command.clone(),
                    output: String::new(),
                    status: InstallStatus::Running,
                    detail: None,
                    after_events: recorded_events,
                });
                true
            }
            PluginInstallEvent::Output { id, output, .. } => {
                let Some(entry) = self.entry_mut(*id) else {
                    return false;
                };
                if entry.output == *output {
                    return false;
                }
                entry.output = output.clone();
                true
            }
            PluginInstallEvent::Finished {
                id, status, detail, ..
            } => {
                let Some(entry) = self.entry_mut(*id) else {
                    return false;
                };
                if entry.status == *status && entry.detail == *detail {
                    return false;
                }
                entry.status = *status;
                entry.detail = detail.clone();
                true
            }
        }
    }

    /// Every install recorded for this transcript, in the order they started.
    pub fn entries(&self) -> &[PluginInstallEntry] {
        &self.entries
    }

    fn entry_mut(&mut self, id: u64) -> Option<&mut PluginInstallEntry> {
        self.entries.iter_mut().find(|entry| entry.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(id: u64, provider: Provider) -> PluginInstallEvent {
        PluginInstallEvent::Started {
            id,
            conversation_id: "conversation".to_owned(),
            provider,
            command: install_command(provider, "owner/repo").display(),
        }
    }

    #[test]
    fn marketplace_sources_are_registered_the_same_way_by_every_agent() {
        for source in [
            "https://example.com/plugins",
            "git@github.com:owner/repo.git",
            "owner/repo@main",
            "./marketplace",
            "/srv/marketplace",
            "~/marketplace",
        ] {
            assert_eq!(install_kind(source), InstallKind::Marketplace, "{source}");
            assert_eq!(
                install_command(Provider::Claude, source).display(),
                format!("claude plugin marketplace add {source}")
            );
            assert_eq!(
                install_command(Provider::Codex, source).display(),
                format!("codex plugin marketplace add {source}")
            );
        }
    }

    /// Claude Code installs a plugin with `plugin install`, Codex with
    /// `plugin add`. Sharing one verb makes the install fail for one of them.
    #[test]
    fn plugin_selectors_use_each_agents_own_install_subcommand() {
        for source in ["superpowers", "superpowers@superpowers-marketplace"] {
            assert_eq!(install_kind(source), InstallKind::Plugin, "{source}");
            assert_eq!(
                install_command(Provider::Claude, source).display(),
                format!("claude plugin install {source}")
            );
            assert_eq!(
                install_command(Provider::Codex, source).display(),
                format!("codex plugin add {source}")
            );
        }
    }

    #[test]
    fn starting_an_install_records_its_place_in_the_transcript() {
        let mut installs = TranscriptInstalls::default();

        assert!(installs.on_event(&started(0, Provider::Codex), 4));
        assert!(!installs.on_event(&started(0, Provider::Codex), 9));

        let entry = &installs.entries()[0];
        assert_eq!(entry.after_events, 4);
        assert_eq!(entry.status, InstallStatus::Running);
        assert_eq!(entry.command, "codex plugin marketplace add owner/repo");
    }

    #[test]
    fn streamed_output_replaces_the_rendered_screen_once_per_change() {
        let mut installs = TranscriptInstalls::default();
        installs.on_event(&started(1, Provider::Claude), 0);
        let output = |output: &str| PluginInstallEvent::Output {
            id: 1,
            conversation_id: "conversation".to_owned(),
            provider: Provider::Claude,
            output: output.to_owned(),
        };

        assert!(installs.on_event(&output("cloning"), 0));
        assert!(!installs.on_event(&output("cloning"), 0));
        assert!(installs.on_event(&output("cloning\ndone"), 0));
        assert_eq!(installs.entries()[0].output, "cloning\ndone");
    }

    #[test]
    fn finishing_an_install_records_its_status_and_detail() {
        let mut installs = TranscriptInstalls::default();
        installs.on_event(&started(2, Provider::Codex), 0);

        assert!(installs.on_event(
            &PluginInstallEvent::Finished {
                id: 2,
                conversation_id: "conversation".to_owned(),
                provider: Provider::Codex,
                kind: InstallKind::Marketplace,
                status: InstallStatus::Failed,
                detail: Some("Codex exited with status 1".to_owned()),
            },
            0
        ));

        let entry = &installs.entries()[0];
        assert_eq!(entry.status, InstallStatus::Failed);
        assert_eq!(entry.detail.as_deref(), Some("Codex exited with status 1"));
    }

    #[test]
    fn events_for_other_transcripts_are_ignored() {
        let mut installs = TranscriptInstalls::default();

        assert!(!installs.on_event(
            &PluginInstallEvent::Output {
                id: 7,
                conversation_id: "conversation".to_owned(),
                provider: Provider::Codex,
                output: "output".to_owned(),
            },
            0
        ));
        assert!(installs.entries().is_empty());
    }

    /// Installs mutate the caller's agent configuration, so the runner is
    /// exercised through an agent that cannot exist instead of a real one.
    #[test]
    fn an_agent_that_cannot_start_fails_its_install_without_leaving_a_run_behind() {
        let mut installs = PluginInstalls {
            next_id: 5,
            ..PluginInstalls::default()
        };
        let mut events = Vec::new();
        let id = installs.spawn(
            "conversation",
            Provider::Codex,
            InstallKind::Marketplace,
            CommandSpec::new("agency-agent-that-does-not-exist", ["plugin"]),
            Path::new("."),
            &mut events,
        );

        assert_eq!(id, 5);
        assert_eq!(installs.next_id, 6);
        assert!(installs.runs.is_empty());
        assert!(matches!(
            events.as_slice(),
            [
                PluginInstallEvent::Started { id: 5, .. },
                PluginInstallEvent::Finished {
                    id: 5,
                    status: InstallStatus::Failed,
                    detail: Some(_),
                    ..
                }
            ]
        ));
    }
}
