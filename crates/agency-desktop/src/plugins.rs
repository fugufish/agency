use std::path::Path;

use agency_agents::Provider;
use agency_mux::CommandSpec;

use crate::terminal::{HeadlessOutcome, HeadlessTerminal};

/// The command Agency runs to install a plugin source for an agent. Both
/// providers register plugin sources the same way, so the command stays
/// provider-neutral apart from the executable.
pub fn install_command(provider: Provider, source: &str) -> CommandSpec {
    CommandSpec::new(provider.command(), ["plugin", "marketplace", "add", source])
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

        for provider in targets.iter().copied() {
            self.spawn(
                conversation_id,
                provider,
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
                terminal,
                output: String::new(),
            }),
            Err(error) => events.push(PluginInstallEvent::Finished {
                id,
                conversation_id: conversation_id.to_owned(),
                provider,
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
    fn installs_run_the_same_command_for_every_agent() {
        assert_eq!(
            install_command(Provider::Claude, "https://example.com/plugins").display(),
            "claude plugin marketplace add https://example.com/plugins"
        );
        assert_eq!(
            install_command(Provider::Codex, "owner/repo").display(),
            "codex plugin marketplace add owner/repo"
        );
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
