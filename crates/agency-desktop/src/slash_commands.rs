use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use agency_agents::{McpServer, Provider};
use agency_translator_api::ClientId;
use agency_translator_api::commands::AgentCommand;

use crate::config::{
    WORKSPACE_CONFIG_FILE, WORKSPACE_LOCAL_CONFIG_FILE, workspace_config_directory,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Init,
    McpAdd {
        name: String,
    },
    PluginInstall {
        source: String,
        /// The single agent named by `--agent`, or every configured agent.
        agent: Option<Provider>,
    },
}

pub const PLUGIN_INSTALL_USAGE: &str =
    "Usage: /plugin install [--agent <codex|claude>] <plugin[@marketplace] | marketplace source>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandCompletion {
    pub command: String,
    pub description: String,
    pub insertion: String,
    pub provider: Option<Provider>,
    pub built_in: bool,
}

pub const INIT_AGENT_PROMPT: &str = r#"/init

Initialize this repository for agents running inside the Agency harness. Create or update AGENTS.md, preserving and respecting every existing instruction. Add or merge a concise, provider-neutral "Agency harness" preamble that tells all agents:

- Agency is an agent orchestrator for agentic application development. It enables connected agents, such as Codex and Claude Code, to work cooperatively on the user's application.
- Instructions and work products must remain interoperable and not depend on one provider.
- Collaboration tools may be used to delegate independent work in parallel, exchange findings, and coordinate ownership. All agents share the same workspace, so avoid overlapping edits, preserve user and agent changes, and verify the combined result.
- Agency supplies session-scoped tools and identity automatically. Use available Agency tools for cross-agent coordination and worktree operations; never ask the user for an Agency session ID.
- Worktrees may isolate concurrent tasks, while repository instructions and the current worktree's state remain authoritative.
- Follow the closest AGENTS.md instructions, report blockers clearly, and do not overwrite unrelated work.

Keep the preamble compact to minimize recurring context cost. Do not duplicate an existing Agency section. Leave CLAUDE.md as the symlink to AGENTS.md created by Agency."#;

/// Agency's own commands, which the harness handles itself rather than passing
/// to an agent. These are always available, including before the first
/// translator catalog has loaded.
pub fn agency_commands() -> Vec<SlashCommandCompletion> {
    vec![
        SlashCommandCompletion {
            command: "/init".to_owned(),
            description: "Initialize Agency files in this workspace".to_owned(),
            insertion: "/init".to_owned(),
            provider: None,
            built_in: false,
        },
        SlashCommandCompletion {
            command: "/mcp add".to_owned(),
            description: "Add a configured MCP server".to_owned(),
            insertion: "/mcp add ".to_owned(),
            provider: None,
            built_in: false,
        },
        SlashCommandCompletion {
            command: "/plugin install".to_owned(),
            description: "Install a plugin, or add a marketplace source, for every configured agent"
                .to_owned(),
            insertion: "/plugin install ".to_owned(),
            provider: None,
            built_in: false,
        },
    ]
}

/// Asks each configured agent's translator what it can run here. This walks the
/// filesystem, so it belongs behind an effect rather than on the UI thread.
pub fn discover_agent_commands(
    providers: &[Provider],
    workspace: &Path,
) -> Vec<(Provider, AgentCommand)> {
    providers
        .iter()
        .filter_map(|provider| {
            let catalog = agency_translators::command_catalog(&client_id(*provider))?;
            Some(
                catalog
                    .commands(workspace)
                    .into_iter()
                    .map(move |command| (*provider, command)),
            )
        })
        .flatten()
        .collect()
}

/// Agency's commands followed by every agent's. Two agents offering the same
/// name stay as two rows, because their insertions differ and collapsing them
/// would send the wrong text to one of them. Shadowing *within* one agent was
/// already resolved by that agent's translator.
pub fn merge_catalog(agent: Vec<(Provider, AgentCommand)>) -> Vec<SlashCommandCompletion> {
    let mut catalog = agency_commands();
    catalog.extend(
        agent
            .into_iter()
            .map(|(provider, command)| SlashCommandCompletion {
                command: format!("/{}", command.name),
                description: command.description,
                insertion: command.invocation,
                provider: Some(provider),
                built_in: command.origin.is_built_in(),
            }),
    );
    catalog
}

fn client_id(provider: Provider) -> ClientId {
    match provider {
        Provider::Codex => ClientId::new("codex"),
        Provider::Claude => ClientId::new("claude-code"),
    }
}

pub fn slash_command_completions<'a>(
    catalog: &'a [SlashCommandCompletion],
    input: &'a str,
) -> impl Iterator<Item = &'a SlashCommandCompletion> {
    let input = input.trim_start();
    catalog
        .iter()
        .filter(move |completion| matches(&completion.command, input))
}

/// Whether `input` finds `command`.
///
/// Plugin entries are namespaced — `/superpowers:brainstorming` — so matching
/// on the whole command alone would force the user to remember which plugin
/// owns a command before they could find it. Each `:`-delimited segment is
/// also offered as a starting point, which keeps the match predictable: a
/// query always prefixes *something*, never an arbitrary subsequence.
pub fn matches(command: &str, input: &str) -> bool {
    let Some(typed) = input.strip_prefix('/') else {
        return false;
    };
    let Some(command) = command.strip_prefix('/') else {
        return false;
    };
    command
        .split(':')
        .scan(0, |offset, segment| {
            let start = *offset;
            *offset += segment.len() + 1;
            Some(&command[start..])
        })
        .any(|segment| segment.starts_with(typed))
}

/// What the composer looks like to the completion list. The overlay borrows
/// focus from the composer, so "focused" stays true while the list itself is
/// the focused element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposerState {
    /// The composer keymap owns the keyboard.
    pub focused: bool,
    /// The composer is in a mode that types into the prompt.
    pub accepting_text: bool,
}

/// Self-contained state for the inline slash command list. It reduces the
/// composer's prompt into a visibility and a highlighted row, so no view or key
/// handler has to keep the two in step by hand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlashCompletionState {
    open: bool,
    selected: usize,
}

impl SlashCompletionState {
    pub fn is_open(self) -> bool {
        self.open
    }

    pub fn selected(self) -> usize {
        self.selected
    }

    /// Closes the list without forgetting anything else. Typing reopens it,
    /// because the next prompt change refreshes from scratch.
    pub fn close(&mut self) {
        self.open = false;
        self.selected = 0;
    }

    /// Re-derives visibility from the prompt as it stands *now*. Every caller
    /// has to run this after the prompt has already been mutated, otherwise the
    /// list describes the previous keystroke.
    pub fn refresh(
        &mut self,
        catalog: &[SlashCommandCompletion],
        prompt: &str,
        composer: ComposerState,
    ) {
        let matches = slash_command_completions(catalog, prompt).count();
        if matches == 0 || !composer.focused {
            self.close();
            return;
        }
        // Typing opens the list; NORMAL mode keeps an already-open list around
        // so `j`/`k` can walk it without INSERT.
        if composer.accepting_text {
            self.open = true;
        }
        self.selected = self.selected.min(matches - 1);
    }

    pub fn select_previous(&mut self, matches: usize) {
        if matches > 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(matches - 1);
        }
    }

    pub fn select_next(&mut self, matches: usize) {
        if matches > 0 {
            self.selected = (self.selected + 1) % matches;
        }
    }
}

/// How many catalog entries `prompt` currently matches.
pub fn completion_count(catalog: &[SlashCommandCompletion], prompt: &str) -> usize {
    slash_command_completions(catalog, prompt).count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabCompletion {
    /// Fill the shared prefix and leave the list open on the narrowed matches.
    Fill(String),
    /// Nothing left to fill, so commit the highlighted command.
    Accept(SlashCommandCompletion),
}

/// What a Tab press should do for `input` with `selected` highlighted, or
/// `None` when nothing matches and Tab has no completion to offer.
pub fn tab_completion(
    catalog: &[SlashCommandCompletion],
    input: &str,
    selected: usize,
) -> Option<TabCompletion> {
    match shared_completion_prefix(catalog, input) {
        Some(prefix) => Some(TabCompletion::Fill(prefix)),
        None => slash_command_completions(catalog, input)
            .nth(selected)
            .cloned()
            .map(TabCompletion::Accept),
    }
}

/// The longest prefix every match shares, when it reaches past what was typed.
/// Tab fills this in the way a shell does, so a unique command completes in one
/// press and an ambiguous one narrows to the point where the choices differ.
pub fn shared_completion_prefix(catalog: &[SlashCommandCompletion], input: &str) -> Option<String> {
    let input = input.trim_start();
    let mut matches =
        slash_command_completions(catalog, input).map(|completion| completion.command.as_str());
    let mut prefix = matches.next()?.to_owned();
    for command in matches {
        let shared = prefix
            .char_indices()
            .zip(command.chars())
            .take_while(|((_, mine), theirs)| mine == theirs)
            .map(|((index, mine), _)| index + mine.len_utf8())
            .last()
            .unwrap_or(0);
        prefix.truncate(shared);
    }
    (prefix.len() > input.len()).then_some(prefix)
}

pub fn parse_slash_command(input: &str) -> Result<Option<SlashCommand>, String> {
    let input = input.trim();
    if !input.starts_with('/') {
        return Ok(None);
    }

    let parts = input.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        ["/init"] => Ok(Some(SlashCommand::Init)),
        ["/init", ..] => Err("Usage: /init".to_owned()),
        ["/mcp", "add", name] => Ok(Some(SlashCommand::McpAdd {
            name: (*name).to_owned(),
        })),
        ["/mcp", "add"] => Err("Usage: /mcp add <name>".to_owned()),
        ["/mcp", ..] => Err("Usage: /mcp add <name>".to_owned()),
        ["/plugin", "install", arguments @ ..] => parse_plugin_install(arguments).map(Some),
        ["/plugin", ..] => Err(PLUGIN_INSTALL_USAGE.to_owned()),
        [command, ..] => Err(format!("Unknown Agency command: {command}")),
        [] => Ok(None),
    }
}

fn parse_plugin_install(arguments: &[&str]) -> Result<SlashCommand, String> {
    let mut agent = None;
    let mut source = None;
    let mut arguments = arguments.iter().copied();

    while let Some(argument) = arguments.next() {
        let named_agent = if argument == "--agent" {
            Some(
                arguments
                    .next()
                    .ok_or_else(|| PLUGIN_INSTALL_USAGE.to_owned())?,
            )
        } else {
            argument.strip_prefix("--agent=")
        };

        match named_agent {
            Some(name) => {
                if agent.is_some() {
                    return Err("Pass --agent at most once".to_owned());
                }
                agent = Some(Provider::from_name(name).ok_or_else(|| {
                    format!("Unknown agent {name:?}. Use \"codex\" or \"claude\".")
                })?);
            }
            None if argument.starts_with('-') => {
                return Err(format!(
                    "Unknown option {argument:?}. {PLUGIN_INSTALL_USAGE}"
                ));
            }
            None if source.is_some() => return Err(PLUGIN_INSTALL_USAGE.to_owned()),
            None => source = Some(argument.to_owned()),
        }
    }

    Ok(SlashCommand::PluginInstall {
        source: source.ok_or_else(|| PLUGIN_INSTALL_USAGE.to_owned())?,
        agent,
    })
}

pub fn initialize_workspace(workspace: &Path) -> Result<Vec<PathBuf>, String> {
    let config_directory = workspace_config_directory(workspace);
    fs::create_dir_all(&config_directory).map_err(|error| {
        format!(
            "Could not create workspace config directory {}: {error}",
            config_directory.display()
        )
    })?;

    let mut created = Vec::new();
    for path in [
        config_directory.join(WORKSPACE_CONFIG_FILE),
        config_directory.join(WORKSPACE_LOCAL_CONFIG_FILE),
        workspace.join("AGENTS.md"),
    ] {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => created.push(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!("Could not create {}: {error}", path.display()));
            }
        }
    }

    let claude_path = workspace.join("CLAUDE.md");
    match fs::symlink_metadata(&claude_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_agents_symlink(&claude_path)?;
            created.push(claude_path);
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect {}: {error}",
                claude_path.display()
            ));
        }
    }

    Ok(created)
}

#[cfg(unix)]
fn create_agents_symlink(path: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink("AGENTS.md", path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))
}

#[cfg(windows)]
fn create_agents_symlink(path: &Path) -> Result<(), String> {
    std::os::windows::fs::symlink_file("AGENTS.md", path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))
}

pub fn load_codex_mcp(name: &str) -> Result<McpServer, String> {
    let output = Command::new("codex")
        .args(["mcp", "get", name, "--json"])
        .output()
        .map_err(|error| format!("Could not inspect MCP server {name}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .filter(|line| !line.starts_with("WARNING:"))
            .collect::<Vec<_>>()
            .join(" ");
        return Err(if detail.trim().is_empty() {
            format!("MCP server {name:?} is not configured in Codex")
        } else {
            detail
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Codex returned invalid MCP configuration for {name}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agency_translator_api::commands::CommandOrigin;

    #[test]
    fn regular_prompts_are_not_commands() {
        assert_eq!(parse_slash_command("please fix it").unwrap(), None);
    }

    #[test]
    fn init_accepts_no_arguments() {
        assert_eq!(
            parse_slash_command(" /init ").unwrap(),
            Some(SlashCommand::Init)
        );
        assert_eq!(
            parse_slash_command("/init now").unwrap_err(),
            "Usage: /init"
        );
    }

    #[test]
    fn init_prompt_covers_agency_interoperability_without_provider_specific_instructions() {
        assert!(INIT_AGENT_PROMPT.starts_with("/init"));
        assert!(INIT_AGENT_PROMPT.contains("preserving and respecting every existing instruction"));
        assert!(INIT_AGENT_PROMPT.contains("Agency is an agent orchestrator"));
        assert!(INIT_AGENT_PROMPT.contains("Codex and Claude Code"));
        assert!(INIT_AGENT_PROMPT.contains("agentic application development"));
        assert!(INIT_AGENT_PROMPT.contains("All agents share the same workspace"));
        assert!(INIT_AGENT_PROMPT.contains("never ask the user for an Agency session ID"));
        assert!(INIT_AGENT_PROMPT.contains("Do not duplicate an existing Agency section"));
    }

    #[cfg(unix)]
    #[test]
    fn init_creates_workspace_files_and_preserves_existing_contents() {
        let workspace = std::env::temp_dir().join(format!(
            "agency-init-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("AGENTS.md"), "existing instructions").unwrap();

        let created = initialize_workspace(&workspace).unwrap();
        assert_eq!(created.len(), 3);
        assert!(workspace.join(".agency/config.toml").is_file());
        assert!(workspace.join(".agency/config.local.toml").is_file());
        assert_eq!(
            fs::read_to_string(workspace.join("AGENTS.md")).unwrap(),
            "existing instructions"
        );
        assert_eq!(
            fs::read_link(workspace.join("CLAUDE.md")).unwrap(),
            Path::new("AGENTS.md")
        );

        assert!(initialize_workspace(&workspace).unwrap().is_empty());
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn mcp_add_requires_exactly_one_name() {
        assert_eq!(
            parse_slash_command(" /mcp add context7 ").unwrap(),
            Some(SlashCommand::McpAdd {
                name: "context7".to_owned()
            })
        );
        assert_eq!(
            parse_slash_command("/mcp add").unwrap_err(),
            "Usage: /mcp add <name>"
        );
    }

    #[test]
    fn plugin_install_defaults_to_every_configured_agent() {
        assert_eq!(
            parse_slash_command("  /plugin install https://example.com/plugins  ").unwrap(),
            Some(SlashCommand::PluginInstall {
                source: "https://example.com/plugins".to_owned(),
                agent: None,
            })
        );
    }

    #[test]
    fn plugin_install_accepts_an_agent_before_or_after_the_source() {
        let claude = Some(SlashCommand::PluginInstall {
            source: "owner/repo".to_owned(),
            agent: Some(Provider::Claude),
        });
        assert_eq!(
            parse_slash_command("/plugin install --agent claude owner/repo").unwrap(),
            claude
        );
        assert_eq!(
            parse_slash_command("/plugin install owner/repo --agent=claude-code").unwrap(),
            claude
        );
        assert_eq!(
            parse_slash_command("/plugin install --agent Codex owner/repo").unwrap(),
            Some(SlashCommand::PluginInstall {
                source: "owner/repo".to_owned(),
                agent: Some(Provider::Codex),
            })
        );
    }

    #[test]
    fn plugin_install_reports_usage_for_malformed_input() {
        assert_eq!(
            parse_slash_command("/plugin install").unwrap_err(),
            PLUGIN_INSTALL_USAGE
        );
        assert_eq!(
            parse_slash_command("/plugin").unwrap_err(),
            PLUGIN_INSTALL_USAGE
        );
        assert_eq!(
            parse_slash_command("/plugin remove thing").unwrap_err(),
            PLUGIN_INSTALL_USAGE
        );
        assert_eq!(
            parse_slash_command("/plugin install one two").unwrap_err(),
            PLUGIN_INSTALL_USAGE
        );
        assert_eq!(
            parse_slash_command("/plugin install --agent gemini url").unwrap_err(),
            "Unknown agent \"gemini\". Use \"codex\" or \"claude\"."
        );
        assert_eq!(
            parse_slash_command("/plugin install url --agent").unwrap_err(),
            PLUGIN_INSTALL_USAGE
        );
        assert_eq!(
            parse_slash_command("/plugin install --scope user url").unwrap_err(),
            format!("Unknown option \"--scope\". {PLUGIN_INSTALL_USAGE}")
        );
    }

    fn agent_command(name: &str, invocation: &str, origin: CommandOrigin) -> AgentCommand {
        AgentCommand {
            name: name.to_owned(),
            description: "Does a thing".to_owned(),
            invocation: invocation.to_owned(),
            argument_hint: None,
            origin,
        }
    }

    #[test]
    fn agency_commands_are_always_offered() {
        let catalog = merge_catalog(Vec::new());
        let plugin = catalog
            .iter()
            .find(|completion| completion.command == "/plugin install")
            .unwrap();

        assert_eq!(plugin.insertion, "/plugin install ");
        assert_eq!(plugin.provider, None);
        assert!(!plugin.built_in);
        assert!(catalog.iter().any(|completion| completion.command == "/init"));
        assert!(catalog.iter().any(|completion| completion.command == "/mcp add"));
    }

    #[test]
    fn a_translator_command_keeps_its_invocation_and_origin() {
        let catalog = merge_catalog(vec![
            (
                Provider::Claude,
                agent_command(
                    "superpowers:brainstorming",
                    "/superpowers:brainstorming ",
                    CommandOrigin::Plugin {
                        plugin: "superpowers".to_owned(),
                        marketplace: "superpowers-marketplace".to_owned(),
                    },
                ),
            ),
            (
                Provider::Claude,
                agent_command("code-review", "/code-review ", CommandOrigin::BuiltIn),
            ),
        ]);

        let brainstorming = catalog
            .iter()
            .find(|completion| completion.command == "/superpowers:brainstorming")
            .unwrap();
        assert_eq!(brainstorming.insertion, "/superpowers:brainstorming ");
        assert_eq!(brainstorming.provider, Some(Provider::Claude));
        assert!(!brainstorming.built_in);

        let review = catalog
            .iter()
            .find(|completion| completion.command == "/code-review")
            .unwrap();
        assert!(review.built_in);
    }

    /// Two agents offering the same name stay as two rows: the insertions
    /// differ, so collapsing them would send the wrong text to one of them.
    #[test]
    fn duplicate_names_are_kept_between_agents() {
        let catalog = merge_catalog(vec![
            (
                Provider::Codex,
                agent_command("review", "$review ", CommandOrigin::Personal),
            ),
            (
                Provider::Claude,
                agent_command("review", "/review ", CommandOrigin::Personal),
            ),
        ]);

        let review = catalog
            .iter()
            .filter(|completion| completion.command == "/review")
            .collect::<Vec<_>>();
        assert_eq!(review.len(), 2);
        assert_eq!(review[0].insertion, "$review ");
        assert_eq!(review[1].insertion, "/review ");
    }

    #[test]
    fn discovery_of_a_workspace_with_no_agents_yields_nothing() {
        assert!(discover_agent_commands(&[], Path::new("/a/workspace")).is_empty());
    }

    #[test]
    fn unknown_commands_are_rejected_locally() {
        assert_eq!(
            parse_slash_command("/wat").unwrap_err(),
            "Unknown Agency command: /wat"
        );
    }

    #[test]
    fn slash_prefixes_offer_matching_completions() {
        let completions = vec![SlashCommandCompletion {
            command: "/mcp add".to_owned(),
            description: "Add a configured MCP server".to_owned(),
            insertion: "/mcp add ".to_owned(),
            provider: None,
            built_in: false,
        }];
        assert_eq!(
            slash_command_completions(&completions, "/").collect::<Vec<_>>(),
            completions.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            slash_command_completions(&completions, "/mcp a").collect::<Vec<_>>(),
            completions.iter().collect::<Vec<_>>()
        );
        assert!(
            slash_command_completions(&completions, "hello")
                .next()
                .is_none()
        );
        assert!(
            slash_command_completions(&completions, "/wat")
                .next()
                .is_none()
        );
    }

    fn completion(command: &str) -> SlashCommandCompletion {
        SlashCommandCompletion {
            command: command.to_owned(),
            description: String::new(),
            insertion: format!("{command} "),
            provider: None,
            built_in: false,
        }
    }

    #[test]
    fn tab_extends_an_ambiguous_prefix_to_where_the_matches_diverge() {
        let catalog = vec![
            completion("/plugin install"),
            completion("/plugin remove"),
            completion("/mcp add"),
        ];
        assert_eq!(
            shared_completion_prefix(&catalog, "/p").as_deref(),
            Some("/plugin ")
        );
        // Leading whitespace is trimmed, and a prefix that is already filled
        // leaves Tab nothing to add.
        assert_eq!(
            shared_completion_prefix(&catalog, "  /pl").as_deref(),
            Some("/plugin ")
        );
        assert_eq!(shared_completion_prefix(&catalog, "/plugin "), None);
    }

    #[test]
    fn tab_completes_a_unique_match_in_full() {
        let catalog = vec![completion("/mcp add"), completion("/init")];
        assert_eq!(
            shared_completion_prefix(&catalog, "/m").as_deref(),
            Some("/mcp add")
        );
    }

    #[test]
    fn tab_reports_nothing_to_fill_once_the_prefix_is_complete() {
        let catalog = vec![completion("/init"), completion("/insights")];
        assert_eq!(shared_completion_prefix(&catalog, "/in"), None);
        assert_eq!(shared_completion_prefix(&catalog, "/init"), None);
        assert_eq!(shared_completion_prefix(&catalog, "/wat"), None);
        assert_eq!(shared_completion_prefix(&catalog, "hello"), None);
    }

    #[test]
    fn tab_does_not_split_multi_byte_characters() {
        let catalog = vec![completion("/résumé"), completion("/rétro")];
        assert_eq!(
            shared_completion_prefix(&catalog, "/r").as_deref(),
            Some("/ré")
        );
    }

    #[test]
    fn tab_fills_before_it_accepts_and_then_commits_the_highlighted_command() {
        let catalog = vec![completion("/plugin install"), completion("/plugin remove")];

        assert_eq!(
            tab_completion(&catalog, "/p", 1),
            Some(TabCompletion::Fill("/plugin ".to_owned()))
        );
        // A second press has no prefix left to fill, so it takes the selection.
        assert_eq!(
            tab_completion(&catalog, "/plugin ", 1),
            Some(TabCompletion::Accept(completion("/plugin remove")))
        );
        assert_eq!(tab_completion(&catalog, "/wat", 0), None);
    }

    #[test]
    fn tab_accepts_the_insertion_rather_than_the_displayed_command() {
        let catalog = vec![SlashCommandCompletion {
            command: "/review".to_owned(),
            description: String::new(),
            insertion: "$review ".to_owned(),
            provider: Some(Provider::Codex),
            built_in: false,
        }];

        let Some(TabCompletion::Accept(accepted)) = tab_completion(&catalog, "/review", 0) else {
            panic!("a fully typed command should be accepted");
        };
        assert_eq!(accepted.insertion, "$review ");
        assert_eq!(accepted.provider, Some(Provider::Codex));
    }

    #[test]
    fn tab_ignores_a_selection_past_the_narrowed_matches() {
        let catalog = vec![completion("/init"), completion("/mcp add")];

        assert_eq!(tab_completion(&catalog, "/init", 7), None);
    }

    const TYPING: ComposerState = ComposerState {
        focused: true,
        accepting_text: true,
    };
    const NORMAL: ComposerState = ComposerState {
        focused: true,
        accepting_text: false,
    };

    /// Regression: the list used to be refreshed from the prompt as it stood
    /// *before* the keystroke was applied, so the leading `/` never opened it
    /// and every later row described the previous character.
    #[test]
    fn the_leading_slash_opens_the_list() {
        let catalog = vec![completion("/init"), completion("/mcp add")];
        let mut state = SlashCompletionState::default();

        state.refresh(&catalog, "/", TYPING);

        assert!(state.is_open());
        assert_eq!(state.selected(), 0);
    }

    #[test]
    fn a_prompt_without_matches_closes_the_list() {
        let catalog = vec![completion("/init")];
        let mut state = SlashCompletionState::default();

        state.refresh(&catalog, "/i", TYPING);
        assert!(state.is_open());

        state.refresh(&catalog, "/ix", TYPING);
        assert!(!state.is_open());

        state.refresh(&catalog, "", TYPING);
        assert!(!state.is_open());
    }

    #[test]
    fn narrowing_the_matches_pulls_the_selection_back_into_range() {
        let catalog = vec![completion("/init"), completion("/insights")];
        let mut state = SlashCompletionState::default();
        state.refresh(&catalog, "/in", TYPING);
        state.select_next(2);
        assert_eq!(state.selected(), 1);

        state.refresh(&catalog, "/init", TYPING);

        assert!(state.is_open());
        assert_eq!(state.selected(), 0);
    }

    /// The list binds `j`/`k` in NORMAL, so leaving INSERT must not close it.
    /// It still must not *open* without the composer typing into the prompt.
    #[test]
    fn normal_mode_keeps_an_open_list_but_does_not_open_a_closed_one() {
        let catalog = vec![completion("/init")];
        let mut state = SlashCompletionState::default();

        state.refresh(&catalog, "/i", NORMAL);
        assert!(!state.is_open());

        state.refresh(&catalog, "/i", TYPING);
        state.refresh(&catalog, "/i", NORMAL);
        assert!(state.is_open());
    }

    /// Focus moving off the composer takes the composer's overlay with it.
    #[test]
    fn losing_composer_focus_closes_the_list() {
        let catalog = vec![completion("/init")];
        let mut state = SlashCompletionState::default();
        state.refresh(&catalog, "/i", TYPING);

        state.refresh(
            &catalog,
            "/i",
            ComposerState {
                focused: false,
                accepting_text: true,
            },
        );

        assert!(!state.is_open());
    }

    #[test]
    fn selection_wraps_in_both_directions_and_ignores_an_empty_list() {
        let mut state = SlashCompletionState::default();

        state.select_previous(3);
        assert_eq!(state.selected(), 2);
        state.select_next(3);
        assert_eq!(state.selected(), 0);
        state.select_previous(0);
        assert_eq!(state.selected(), 0);
        state.select_next(0);
        assert_eq!(state.selected(), 0);
    }

    /// Accepting a completion closes the list even though the accepted prompt
    /// still matches its own catalog entry, so the next Enter submits.
    #[test]
    fn closing_survives_a_prompt_that_still_matches() {
        let catalog = vec![completion("/init")];
        let mut state = SlashCompletionState::default();
        state.refresh(&catalog, "/i", TYPING);

        state.close();

        assert!(!state.is_open());
        assert_eq!(completion_count(&catalog, "/init"), 1);
    }

    #[test]
    fn a_segment_of_a_namespaced_command_matches() {
        let catalog = vec![
            completion("/superpowers:brainstorming"),
            completion("/hookify:configure"),
        ];

        // The whole command still matches by prefix.
        assert_eq!(completion_count(&catalog, "/super"), 1);
        assert_eq!(completion_count(&catalog, "/superpowers:b"), 1);
        // And so does the part after the namespace.
        assert_eq!(completion_count(&catalog, "/brain"), 1);
        assert_eq!(completion_count(&catalog, "/configure"), 1);
        // A bare slash still matches everything.
        assert_eq!(completion_count(&catalog, "/"), 2);
        // Nonsense still matches nothing.
        assert_eq!(completion_count(&catalog, "/zzz"), 0);
    }

    /// A segment match is not a prefix, so there is nothing for Tab to fill in
    /// common across divergent matches — it falls through to accepting the
    /// highlighted row, which is the existing behaviour.
    #[test]
    fn tab_fills_a_unique_segment_match_and_accepts_an_ambiguous_one() {
        let catalog = vec![
            completion("/superpowers:brainstorming"),
            completion("/hookify:brainstorming-lite"),
        ];

        assert_eq!(
            tab_completion(&catalog, "/superpowers:b", 0),
            Some(TabCompletion::Fill("/superpowers:brainstorming".to_owned()))
        );
        assert_eq!(
            tab_completion(&catalog, "/brain", 1),
            Some(TabCompletion::Accept(completion("/hookify:brainstorming-lite")))
        );
    }

    #[test]
    fn matching_requires_a_leading_slash_and_a_segment_boundary() {
        assert!(matches("/superpowers:brainstorming", "/brain"));
        assert!(matches("/superpowers:brainstorming", "/superpowers"));
        // "storming" starts mid-segment, so it does not match.
        assert!(!matches("/superpowers:brainstorming", "/storming"));
        assert!(!matches("/superpowers:brainstorming", "brain"));
    }
}
