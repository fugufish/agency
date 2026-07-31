//! Where Claude Code finds the commands it can run, and which of them are
//! live.
//!
//! Three files decide this and none of them can be inferred from the cache
//! layout: `installed_plugins.json` names the install path for each plugin,
//! because several versions can sit in the cache at once; the settings chain
//! carries `enabledPlugins`; and each plugin's manifest can move its own
//! component directories and ship opted out by default.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use agency_translator_api::commands::{AgentCommand, CommandOrigin};
use agency_translator_api::discovery::{self, DiscoveredFile};
use serde_json::Value;

/// A plugin as `installed_plugins.json` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InstalledPlugin {
    pub(super) name: String,
    pub(super) marketplace: String,
    pub(super) install_path: PathBuf,
}

impl InstalledPlugin {
    /// The `<name>@<marketplace>` key that `enabledPlugins` also uses.
    fn key(&self) -> String {
        format!("{}@{}", self.name, self.marketplace)
    }
}

/// The manifest fields that change where components live or whether the plugin
/// starts on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Manifest {
    pub(super) default_enabled: Option<bool>,
    /// Extra skill roots. These add to the default `skills/` scan.
    pub(super) skills: Vec<PathBuf>,
    /// Command roots or files. When present these *replace* the default
    /// `commands/` scan, which is the documented asymmetry with `skills`.
    pub(super) commands: Option<Vec<PathBuf>>,
}

/// Every plugin the user has installed, sorted so the catalog is reproducible.
pub(super) fn installed_plugins(home: &Path) -> Vec<InstalledPlugin> {
    let path = home.join(".claude/plugins/installed_plugins.json");
    let Some(plugins) = read_json(&path)
        .and_then(|document| document.get("plugins").cloned())
        .and_then(|plugins| plugins.as_object().cloned())
    else {
        return Vec::new();
    };

    let mut found = plugins
        .into_iter()
        .filter_map(|(key, entries)| {
            let (name, marketplace) = key.split_once('@')?;
            // One key can carry several scopes. The first entry is the one the
            // file lists first, and a second scope for the same plugin points
            // at the same cache directory in practice.
            let install_path = entries
                .as_array()?
                .first()?
                .get("installPath")?
                .as_str()
                .map(PathBuf::from)?;
            Some(InstalledPlugin {
                name: name.to_owned(),
                marketplace: marketplace.to_owned(),
                install_path,
            })
        })
        .collect::<Vec<_>>();
    found.sort_by_key(|plugin| plugin.key());
    found
}

/// `enabledPlugins` merged across the settings scopes, in increasing
/// precedence. Managed enterprise settings outrank all of these and are out of
/// scope: Agency runs as a single user's desktop application.
pub(super) fn enabled_plugins(home: &Path, workspace: &Path) -> HashMap<String, bool> {
    let mut enabled = HashMap::new();
    for path in [
        home.join(".claude/settings.json"),
        workspace.join(".claude/settings.json"),
        workspace.join(".claude/settings.local.json"),
    ] {
        let Some(entries) = read_json(&path)
            .and_then(|document| document.get("enabledPlugins").cloned())
            .and_then(|entries| entries.as_object().cloned())
        else {
            continue;
        };
        for (key, value) in entries {
            if let Some(value) = value.as_bool() {
                enabled.insert(key, value);
            }
        }
    }
    enabled
}

/// A plugin's manifest, with its relative component paths resolved against the
/// install directory.
pub(super) fn manifest(install_path: &Path) -> Manifest {
    let Some(document) = read_json(&install_path.join(".claude-plugin/plugin.json")) else {
        return Manifest::default();
    };
    Manifest {
        default_enabled: document.get("defaultEnabled").and_then(Value::as_bool),
        skills: paths(install_path, document.get("skills")).unwrap_or_default(),
        commands: paths(install_path, document.get("commands")),
    }
}

/// Whether a plugin's components should be indexed. A settings entry at any
/// scope is a decision the user made and outranks whatever the plugin ships.
pub(super) fn is_enabled(
    plugin: &InstalledPlugin,
    enabled: &HashMap<String, bool>,
    manifest: &Manifest,
) -> bool {
    enabled
        .get(&plugin.key())
        .copied()
        .or(manifest.default_enabled)
        .unwrap_or(true)
}

/// A manifest path field, which is either one string or an array of them.
fn paths(install_path: &Path, value: Option<&Value>) -> Option<Vec<PathBuf>> {
    let value = value?;
    let relative = match value {
        Value::String(one) => vec![one.clone()],
        Value::Array(many) => many
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => return None,
    };
    Some(
        relative
            .into_iter()
            .map(|path| install_path.join(path.trim_start_matches("./").trim_end_matches('/')))
            .collect(),
    )
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// The commands Claude Code ships. Only the ones that make sense to send from
/// Agency's composer are listed: session controls such as `/exit` or `/login`
/// act on Claude Code's own terminal UI, which Agency does not present. Claude
/// Code's `/init` is omitted because Agency owns that command.
const BUILT_INS: [(&str, &str); 27] = [
    ("agents", "Create or manage subagents"),
    (
        "batch",
        "Orchestrate large-scale changes across a codebase in parallel",
    ),
    ("claude-api", "Load Claude API reference material"),
    (
        "code-review",
        "Review the current diff for bugs and cleanup opportunities",
    ),
    ("compact", "Free up context by summarizing the conversation"),
    ("context", "Visualize current context usage"),
    ("cost", "Show token usage and costs for the current session"),
    (
        "dataviz",
        "Design guidance for charts, graphs, and dashboards",
    ),
    ("debug", "Enable debug logging and troubleshoot issues"),
    (
        "deep-research",
        "Fan out web searches and synthesize a cited report",
    ),
    (
        "design-sync",
        "Convert a React design system and upload it to Claude Design",
    ),
    (
        "diff",
        "Open an interactive diff viewer for uncommitted changes",
    ),
    (
        "doctor",
        "Run a setup checkup that diagnoses and fixes issues",
    ),
    ("export", "Export the current conversation as plain text"),
    (
        "fewer-permission-prompts",
        "Add an allowlist to reduce permission prompts",
    ),
    (
        "goal",
        "Set a goal to keep working until a condition is met",
    ),
    ("hooks", "View hook configurations for tool events"),
    ("insights", "Generate a usage insights report"),
    (
        "loop",
        "Run a prompt repeatedly while the session stays open",
    ),
    ("mcp", "Manage MCP server connections"),
    ("memory", "Edit CLAUDE.md memory files"),
    ("model", "Switch the model for this session"),
    ("permissions", "View and manage permission rules"),
    ("rewind", "Roll code and conversation back to a checkpoint"),
    ("status", "Show the current session status and model"),
    (
        "usage",
        "Show token usage and costs for the current session",
    ),
    ("verify", "Verify code correctness and best practices"),
];

/// Every command Claude Code can run in `workspace`.
///
/// Sources are pushed in increasing precedence and a later entry replaces an
/// earlier one of the same name. Claude Code resolves a clash in favour of the
/// personal entry over the project one, and a skill over a command, so the
/// order below is built-ins, project commands, project skills, personal
/// commands, personal skills. Plugin entries are namespaced and cannot clash,
/// so they are appended last without participating in the shadowing.
pub(super) fn catalog(home: &Path, workspace: &Path) -> Vec<AgentCommand> {
    let mut commands = BUILT_INS
        .into_iter()
        .map(|(name, description)| {
            command(
                name.to_owned(),
                description.to_owned(),
                None,
                CommandOrigin::BuiltIn,
            )
        })
        .collect::<Vec<_>>();

    for (root, origin) in [
        (workspace.join(".claude"), CommandOrigin::Project),
        (home.join(".claude"), CommandOrigin::Personal),
    ] {
        for file in discovery::command_files(&root.join("commands")) {
            push(&mut commands, local(file, origin.clone()));
        }
        for file in discovery::skill_directories(&root.join("skills")) {
            push(&mut commands, local(file, origin.clone()));
        }
    }

    let enabled = enabled_plugins(home, workspace);
    for plugin in installed_plugins(home) {
        let manifest = manifest(&plugin.install_path);
        if !is_enabled(&plugin, &enabled, &manifest) {
            continue;
        }
        commands.extend(plugin_commands(&plugin, &manifest));
    }

    commands
}

/// One plugin's contribution, already namespaced.
fn plugin_commands(plugin: &InstalledPlugin, manifest: &Manifest) -> Vec<AgentCommand> {
    let origin = CommandOrigin::Plugin {
        plugin: plugin.name.clone(),
        marketplace: plugin.marketplace.clone(),
    };
    let mut commands = Vec::new();

    // `commands` in the manifest replaces the default directory; `skills` adds
    // to it. The asymmetry is Claude Code's, not ours.
    let command_roots = manifest
        .commands
        .clone()
        .unwrap_or_else(|| vec![plugin.install_path.join("commands")]);
    for root in command_roots {
        // A manifest entry may name a single file rather than a directory.
        let files = if root.is_file() {
            root.file_stem()
                .map(|stem| DiscoveredFile {
                    name: stem.to_string_lossy().into_owned(),
                    path: root.clone(),
                })
                .into_iter()
                .collect()
        } else {
            discovery::command_files(&root)
        };
        for file in files {
            let contents = read(&file.path);
            commands.push(command(
                format!("{}:{}", plugin.name, file.name),
                discovery::describe(&contents),
                discovery::frontmatter(&contents).argument_hint,
                origin.clone(),
            ));
        }
    }

    // A manifest that redundantly names the default `skills/` directory (or
    // repeats one of its own extra roots) would otherwise be scanned twice,
    // double-listing every skill inside it.
    let mut skill_roots = vec![plugin.install_path.join("skills")];
    for root in manifest.skills.iter().cloned() {
        if !skill_roots.contains(&root) {
            skill_roots.push(root);
        }
    }
    let mut found_any_skill = false;
    for root in skill_roots {
        for file in discovery::skill_directories(&root) {
            found_any_skill = true;
            let contents = read(&file.path);
            let parsed = discovery::frontmatter(&contents);
            // In a plugin skill the frontmatter name replaces the last segment
            // and the plugin prefix stays in place.
            let segment = command_token(parsed.name.clone(), || file.name.clone());
            commands.push(command(
                format!("{}:{segment}", plugin.name),
                discovery::describe(&contents),
                parsed.argument_hint,
                origin.clone(),
            ));
        }
    }

    // A plugin with a root SKILL.md, no skills directory and no skills
    // manifest key is a single-skill plugin.
    let root_skill = plugin.install_path.join("SKILL.md");
    if !found_any_skill && manifest.skills.is_empty() && root_skill.is_file() {
        let contents = read(&root_skill);
        let parsed = discovery::frontmatter(&contents);
        let segment = command_token(parsed.name.clone(), || plugin.name.clone());
        commands.push(command(
            format!("{}:{segment}", plugin.name),
            discovery::describe(&contents),
            parsed.argument_hint,
            origin,
        ));
    }

    commands
}

/// Claude Code's documented rule is that a plugin skill's frontmatter `name`
/// replaces the last segment of its command (`skills/review/SKILL.md` with
/// `name: fancy` becomes `/plugin:fancy`). That rule assumes the frontmatter
/// name is itself shaped like a command. Real plugins also use `name` for a
/// human-readable display label — Hookify's `writing-rules` skill sets
/// `name: Writing Hookify Rules` — and honoring that literally would emit
/// `/hookify:Writing Hookify Rules`, a command with a space in it that Claude
/// Code cannot resolve and nobody could type. So the rename only applies when
/// the frontmatter name could stand alone as a command segment; otherwise the
/// directory (or plugin) name is what Claude Code actually invokes, and that
/// is what gets indexed instead.
fn command_token(name: Option<String>, fallback: impl FnOnce() -> String) -> String {
    match name {
        Some(name) if !name.is_empty() && !name.chars().any(char::is_whitespace) => name,
        _ => fallback(),
    }
}

/// A personal or project entry. At these levels the frontmatter `name` is a
/// display label only — the command comes from the directory or file name.
fn local(file: DiscoveredFile, origin: CommandOrigin) -> AgentCommand {
    let contents = read(&file.path);
    command(
        file.name,
        discovery::describe(&contents),
        discovery::frontmatter(&contents).argument_hint,
        origin,
    )
}

fn command(
    name: String,
    description: String,
    argument_hint: Option<String>,
    origin: CommandOrigin,
) -> AgentCommand {
    AgentCommand {
        invocation: format!("/{name} "),
        name,
        description,
        argument_hint,
        origin,
    }
}

/// Replaces any entry already holding this name, so the caller's push order is
/// the precedence order.
fn push(commands: &mut Vec<AgentCommand>, command: AgentCommand) {
    commands.retain(|existing| existing.name != command.name);
    commands.push(command);
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agency-claude-{}-{label}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: PathBuf, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn install_paths_come_from_the_installed_plugins_file() {
        let home = scratch("installed");
        write(
            home.join(".claude/plugins/installed_plugins.json"),
            r#"{"version":2,"plugins":{
                "superpowers@superpowers-marketplace":[{"scope":"user","installPath":"/cache/superpowers/6.2.0","version":"6.2.0"}],
                "hookify@claude-code-plugins":[{"scope":"user","installPath":"/cache/hookify/0.1.0","version":"0.1.0"}]
            }}"#,
        );

        let found = installed_plugins(&home);

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "hookify");
        assert_eq!(found[0].marketplace, "claude-code-plugins");
        assert_eq!(found[1].name, "superpowers");
        assert_eq!(
            found[1].install_path,
            PathBuf::from("/cache/superpowers/6.2.0")
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_missing_or_malformed_installed_plugins_file_yields_nothing() {
        let home = scratch("malformed");
        assert!(installed_plugins(&home).is_empty());
        write(
            home.join(".claude/plugins/installed_plugins.json"),
            "{not json",
        );
        assert!(installed_plugins(&home).is_empty());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn later_settings_scopes_override_earlier_ones() {
        let home = scratch("settings-home");
        let workspace = scratch("settings-workspace");
        write(
            home.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"a@m":true,"b@m":true,"c@m":true}}"#,
        );
        write(
            workspace.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"b@m":false}}"#,
        );
        write(
            workspace.join(".claude/settings.local.json"),
            r#"{"enabledPlugins":{"c@m":false}}"#,
        );

        let enabled = enabled_plugins(&home, &workspace);

        assert_eq!(enabled.get("a@m"), Some(&true));
        assert_eq!(enabled.get("b@m"), Some(&false));
        assert_eq!(enabled.get("c@m"), Some(&false));
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn manifest_paths_replace_commands_and_extend_skills() {
        let install = scratch("manifest");
        write(
            install.join(".claude-plugin/plugin.json"),
            r#"{"name":"demo","defaultEnabled":false,"skills":"./extra/skills/","commands":["./cmd/one.md","./cmd/two.md"]}"#,
        );

        let parsed = manifest(&install);

        assert_eq!(parsed.default_enabled, Some(false));
        assert_eq!(parsed.skills, vec![install.join("extra/skills")]);
        assert_eq!(
            parsed.commands,
            Some(vec![install.join("cmd/one.md"), install.join("cmd/two.md")])
        );
        fs::remove_dir_all(install).unwrap();
    }

    #[test]
    fn a_plugin_without_a_manifest_keeps_the_default_scan() {
        let install = scratch("no-manifest");
        let parsed = manifest(&install);
        assert_eq!(parsed.default_enabled, None);
        assert!(parsed.skills.is_empty());
        assert_eq!(parsed.commands, None);
        fs::remove_dir_all(install).unwrap();
    }

    fn plugin(name: &str) -> InstalledPlugin {
        InstalledPlugin {
            name: name.to_owned(),
            marketplace: "m".to_owned(),
            install_path: PathBuf::from("/cache").join(name),
        }
    }

    #[test]
    fn a_settings_entry_outranks_the_manifest_default() {
        let opted_out = Manifest {
            default_enabled: Some(false),
            ..Manifest::default()
        };
        let mut enabled = HashMap::new();
        enabled.insert("a@m".to_owned(), true);

        assert!(is_enabled(&plugin("a"), &enabled, &opted_out));
    }

    #[test]
    fn the_manifest_default_decides_when_no_scope_names_the_plugin() {
        let enabled = HashMap::new();
        assert!(!is_enabled(
            &plugin("a"),
            &enabled,
            &Manifest {
                default_enabled: Some(false),
                ..Manifest::default()
            }
        ));
        assert!(is_enabled(&plugin("a"), &enabled, &Manifest::default()));
    }

    #[test]
    fn an_explicit_false_disables_the_plugin() {
        let mut enabled = HashMap::new();
        enabled.insert("a@m".to_owned(), false);
        assert!(!is_enabled(&plugin("a"), &enabled, &Manifest::default()));
    }

    fn skill(root: PathBuf, name: &str, description: &str) {
        write(
            root.join(name).join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: {description}\n---\nBody\n"),
        );
    }

    fn named(commands: &[AgentCommand], name: &str) -> Option<AgentCommand> {
        commands
            .iter()
            .find(|command| command.name == name)
            .cloned()
    }

    #[test]
    fn personal_and_project_entries_are_indexed_without_a_namespace() {
        let home = scratch("levels-home");
        let workspace = scratch("levels-workspace");
        skill(home.join(".claude/skills"), "deploy", "Ship it");
        write(
            workspace.join(".claude/commands/audit.md"),
            "---\ndescription: Audit the repo\nargument-hint: [path]\n---\n",
        );

        let commands = catalog(&home, &workspace);

        let deploy = named(&commands, "deploy").unwrap();
        assert_eq!(deploy.description, "Ship it");
        assert_eq!(deploy.invocation, "/deploy ");
        assert_eq!(deploy.origin, CommandOrigin::Personal);

        let audit = named(&commands, "audit").unwrap();
        assert_eq!(audit.origin, CommandOrigin::Project);
        assert_eq!(audit.argument_hint.as_deref(), Some("[path]"));

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// Claude Code resolves a name clash in favour of the personal skill, which
    /// is the opposite of the usual project-wins intuition.
    #[test]
    fn a_personal_entry_shadows_a_project_entry_of_the_same_name() {
        let home = scratch("shadow-home");
        let workspace = scratch("shadow-workspace");
        skill(home.join(".claude/skills"), "deploy", "Personal");
        skill(workspace.join(".claude/skills"), "deploy", "Project");

        let commands = catalog(&home, &workspace);

        let deploy = commands
            .iter()
            .filter(|command| command.name == "deploy")
            .collect::<Vec<_>>();
        assert_eq!(deploy.len(), 1);
        assert_eq!(deploy[0].description, "Personal");
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_skill_beats_a_command_of_the_same_name() {
        let home = scratch("skill-beats-home");
        let workspace = scratch("skill-beats-workspace");
        skill(home.join(".claude/skills"), "deploy", "From the skill");
        write(
            home.join(".claude/commands/deploy.md"),
            "---\ndescription: From the command\n---\n",
        );

        let commands = catalog(&home, &workspace);

        assert_eq!(
            named(&commands, "deploy").unwrap().description,
            "From the skill"
        );
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// Sets up a plugin in the cache and registers it in
    /// `installed_plugins.json` under `<name>@marketplace`.
    fn install(home: &Path, name: &str, manifest_json: Option<&str>) -> PathBuf {
        let install_path = home.join("cache").join(name);
        fs::create_dir_all(&install_path).unwrap();
        if let Some(manifest_json) = manifest_json {
            write(
                install_path.join(".claude-plugin/plugin.json"),
                manifest_json,
            );
        }
        write(
            home.join(".claude/plugins/installed_plugins.json"),
            &format!(
                r#"{{"version":2,"plugins":{{"{name}@marketplace":[{{"scope":"user","installPath":"{}"}}]}}}}"#,
                install_path.display()
            ),
        );
        install_path
    }

    #[test]
    fn plugin_entries_are_namespaced_and_carry_their_origin() {
        let home = scratch("plugin-home");
        let workspace = scratch("plugin-workspace");
        let install = install(&home, "superpowers", None);
        skill(
            install.join("skills"),
            "brainstorming",
            "Turn ideas into designs",
        );
        write(
            install.join("commands/status.md"),
            "---\ndescription: Show status\n---\n",
        );

        let commands = catalog(&home, &workspace);

        let brainstorming = named(&commands, "superpowers:brainstorming").unwrap();
        assert_eq!(brainstorming.invocation, "/superpowers:brainstorming ");
        assert_eq!(
            brainstorming.origin,
            CommandOrigin::Plugin {
                plugin: "superpowers".to_owned(),
                marketplace: "marketplace".to_owned(),
            }
        );
        assert!(named(&commands, "superpowers:status").is_some());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// In a plugin skill the frontmatter name replaces only the last segment.
    #[test]
    fn a_plugin_skill_frontmatter_name_replaces_the_last_segment_only() {
        let home = scratch("rename-home");
        let workspace = scratch("rename-workspace");
        let install = install(&home, "demo", None);
        write(
            install.join("skills/review/SKILL.md"),
            "---\nname: fancy\ndescription: Review\n---\n",
        );

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "demo:fancy").is_some());
        assert!(named(&commands, "demo:review").is_none());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// Regression for the real Hookify plugin: its `writing-rules` skill sets
    /// `name: Writing Hookify Rules`, which is not a token Claude Code can
    /// invoke. The indexer must fall back to the directory name rather than
    /// emitting a command nobody can type.
    #[test]
    fn a_plugin_skill_frontmatter_name_with_whitespace_falls_back_to_the_directory_name() {
        let home = scratch("rename-invalid-home");
        let workspace = scratch("rename-invalid-workspace");
        let install = install(&home, "hookify", None);
        write(
            install.join("skills/writing-rules/SKILL.md"),
            "---\nname: Writing Hookify Rules\ndescription: Write hookify rules\n---\n",
        );

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "hookify:writing-rules").is_some());
        assert!(named(&commands, "hookify:Writing Hookify Rules").is_none());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_plugin_root_skill_file_becomes_a_single_skill_plugin() {
        let home = scratch("root-skill-home");
        let workspace = scratch("root-skill-workspace");
        let install = install(&home, "solo", None);
        write(
            install.join("SKILL.md"),
            "---\ndescription: The only one\n---\n",
        );

        let commands = catalog(&home, &workspace);

        assert_eq!(
            named(&commands, "solo:solo").unwrap().description,
            "The only one"
        );
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_disabled_plugin_contributes_nothing() {
        let home = scratch("disabled-home");
        let workspace = scratch("disabled-workspace");
        let install = install(&home, "off", None);
        skill(install.join("skills"), "hidden", "Should not appear");
        // Positive control: without this, the assertion below would also pass
        // if `catalog` returned nothing at all, regardless of `is_enabled`.
        skill(home.join(".claude/skills"), "elsewhere", "Still works");
        write(
            home.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"off@marketplace":false}}"#,
        );

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "off:hidden").is_none());
        assert!(named(&commands, "elsewhere").is_some());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_plugin_opted_out_by_default_is_restored_by_a_settings_entry() {
        let home = scratch("opt-in-home");
        let workspace = scratch("opt-in-workspace");
        let install = install(
            &home,
            "optional",
            Some(r#"{"name":"optional","defaultEnabled":false}"#),
        );
        skill(install.join("skills"), "extra", "Opt in");

        assert!(named(&catalog(&home, &workspace), "optional:extra").is_none());

        write(
            home.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"optional@marketplace":true}}"#,
        );
        assert!(named(&catalog(&home, &workspace), "optional:extra").is_some());

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn manifest_command_paths_replace_the_default_directory() {
        let home = scratch("replace-home");
        let workspace = scratch("replace-workspace");
        let install = install(
            &home,
            "custom",
            Some(r#"{"name":"custom","commands":["./cmd/"]}"#),
        );
        write(
            install.join("commands/ignored.md"),
            "---\ndescription: No\n---\n",
        );
        write(install.join("cmd/used.md"), "---\ndescription: Yes\n---\n");

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "custom:used").is_some());
        assert!(named(&commands, "custom:ignored").is_none());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// A manifest declaring `"skills":"./skills/"` names the same directory
    /// `catalog` already scans by default. Without a dedup, every skill in
    /// it would be indexed twice.
    #[test]
    fn a_manifest_that_redundantly_names_the_default_skills_directory_lists_once() {
        let home = scratch("redundant-skills-home");
        let workspace = scratch("redundant-skills-workspace");
        let install = install(&home, "dup", Some(r#"{"name":"dup","skills":"./skills/"}"#));
        skill(install.join("skills"), "once", "Should appear once");

        let commands = catalog(&home, &workspace);

        assert_eq!(
            commands
                .iter()
                .filter(|command| command.name == "dup:once")
                .count(),
            1
        );
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn manifest_skill_paths_add_to_the_default_directory() {
        let home = scratch("extend-home");
        let workspace = scratch("extend-workspace");
        let install = install(
            &home,
            "both",
            Some(r#"{"name":"both","skills":["./extra/"]}"#),
        );
        skill(install.join("skills"), "standard", "Default root");
        skill(install.join("extra"), "additional", "Extra root");

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "both:standard").is_some());
        assert!(named(&commands, "both:additional").is_some());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn built_ins_are_present_and_a_personal_entry_shadows_one() {
        let home = scratch("builtin-home");
        let workspace = scratch("builtin-workspace");
        let commands = catalog(&home, &workspace);
        assert!(
            named(&commands, "code-review")
                .unwrap()
                .origin
                .is_built_in()
        );

        skill(home.join(".claude/skills"), "code-review", "Mine");
        let commands = catalog(&home, &workspace);
        let review = named(&commands, "code-review").unwrap();
        assert_eq!(review.origin, CommandOrigin::Personal);
        assert_eq!(
            commands.iter().filter(|c| c.name == "code-review").count(),
            1
        );

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_broken_plugin_does_not_empty_the_catalog() {
        let home = scratch("broken-home");
        let workspace = scratch("broken-workspace");
        let install = install(&home, "broken", Some("{not json"));
        skill(install.join("skills"), "still-here", "Survives");
        skill(home.join(".claude/skills"), "personal", "Survives too");

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "personal").is_some());
        assert!(named(&commands, "broken:still-here").is_some());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }
}
