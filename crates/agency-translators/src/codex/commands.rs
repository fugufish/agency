//! Where Codex finds the skills and prompts it can run.
//!
//! Codex keeps skills outside its own config directory, in `.agents/skills`,
//! and mentions them with `$name` rather than a slash. Plugin skills are not
//! namespaced, so a plugin can shadow a personal skill by name — the same way
//! Codex itself resolves it.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use agency_translator_api::commands::{AgentCommand, CommandOrigin};
use agency_translator_api::discovery::{self, DiscoveredFile};

/// Every command Codex can run in `workspace`.
pub(super) fn catalog(home: &Path, workspace: &Path) -> Vec<AgentCommand> {
    let mut commands = Vec::new();

    // The documented locations first, then the layout Codex used to use. A
    // missing directory costs one failed `read_dir`.
    for (root, origin) in [
        (workspace.join(".agents/skills"), CommandOrigin::Project),
        (workspace.join(".codex/skills"), CommandOrigin::Project),
        (home.join(".agents/skills"), CommandOrigin::Personal),
        (home.join(".codex/skills"), CommandOrigin::Personal),
    ] {
        for file in discovery::skill_directories(&root) {
            push(&mut commands, skill(file, origin.clone()));
        }
    }

    // Custom prompts are deprecated in favour of skills but still load, and
    // they take a `prompts:` prefix rather than a plugin one.
    for file in discovery::command_files(&home.join(".codex/prompts")) {
        let contents = read(&file.path);
        push(
            &mut commands,
            AgentCommand {
                invocation: format!("/prompts:{} ", file.name),
                name: format!("prompts:{}", file.name),
                description: discovery::describe(&contents),
                argument_hint: discovery::frontmatter(&contents).argument_hint,
                origin: CommandOrigin::Personal,
            },
        );
    }

    for plugin in installed_plugins(home) {
        for root in plugin.skill_roots {
            for file in discovery::skill_directories(&root) {
                push(
                    &mut commands,
                    skill(
                        file,
                        CommandOrigin::Plugin {
                            plugin: plugin.name.clone(),
                            marketplace: plugin.marketplace.clone(),
                        },
                    ),
                );
            }
        }
    }

    commands
}

/// A plugin in the Codex cache, with its skill roots already resolved from its
/// manifest.
struct CodexPlugin {
    name: String,
    marketplace: String,
    skill_roots: Vec<PathBuf>,
}

/// Walks `~/.codex/plugins/cache/<marketplace>/<plugin>/<version>/`. The
/// manifest names the skills path, so it is read rather than assumed.
fn installed_plugins(home: &Path) -> Vec<CodexPlugin> {
    let mut found = Vec::new();
    let cache = home.join(".codex/plugins/cache");
    let Ok(marketplaces) = fs::read_dir(&cache) else {
        return found;
    };
    for marketplace in marketplaces.flatten() {
        let Ok(plugins) = fs::read_dir(marketplace.path()) else {
            continue;
        };
        for plugin in plugins.flatten() {
            let Ok(versions) = fs::read_dir(plugin.path()) else {
                continue;
            };
            // Codex ships no live-version pointer the way Claude's
            // installed_plugins.json does, so the highest-sorted version
            // directory stands in for one: without it, scanning every
            // version would leave stale skills from old versions in the
            // catalog, and a same-named skill across versions would resolve
            // by read_dir order, which is not deterministic. This is
            // lexicographic, not semver — a scheme where "0.9.0" sorts above
            // "0.10.0" would pick the wrong directory — but Codex gives us
            // nothing else to go on.
            let mut version_dirs: Vec<PathBuf> = versions
                .flatten()
                .map(|version| version.path())
                .filter(|install_path| install_path.join(".codex-plugin/plugin.json").is_file())
                .collect();
            version_dirs.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
            let Some(install_path) = version_dirs.pop() else {
                continue;
            };
            found.push(CodexPlugin {
                name: plugin.file_name().to_string_lossy().into_owned(),
                marketplace: marketplace.file_name().to_string_lossy().into_owned(),
                skill_roots: skill_roots(&install_path),
            });
        }
    }
    found.sort_by(|left, right| {
        (&left.marketplace, &left.name).cmp(&(&right.marketplace, &right.name))
    });
    found
}

/// The skill roots a plugin declares, falling back to the conventional
/// `skills/` directory when its manifest is silent or unreadable.
fn skill_roots(install_path: &Path) -> Vec<PathBuf> {
    let declared = read_json(&install_path.join(".codex-plugin/plugin.json"))
        .and_then(|document| document.get("skills").cloned());
    let relative = match declared {
        Some(Value::String(one)) => vec![one],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => return vec![install_path.join("skills")],
    };
    relative
        .into_iter()
        .map(|path| install_path.join(path.trim_start_matches("./").trim_end_matches('/')))
        .collect()
}

fn skill(file: DiscoveredFile, origin: CommandOrigin) -> AgentCommand {
    let contents = read(&file.path);
    AgentCommand {
        invocation: format!("${} ", file.name),
        name: file.name,
        description: discovery::describe(&contents),
        argument_hint: discovery::frontmatter(&contents).argument_hint,
        origin,
    }
}

fn push(commands: &mut Vec<AgentCommand>, command: AgentCommand) {
    commands.retain(|existing| existing.name != command.name);
    commands.push(command);
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agency-codex-{}-{label}-{}",
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

    fn skill(root: PathBuf, name: &str, description: &str) {
        write(
            root.join(name).join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: {description}\n---\nBody\n"),
        );
    }

    fn named(commands: &[AgentCommand], name: &str) -> Option<AgentCommand> {
        commands.iter().find(|command| command.name == name).cloned()
    }

    #[test]
    fn skills_are_mentioned_with_a_dollar_sign() {
        let home = scratch("skills-home");
        let workspace = scratch("skills-workspace");
        skill(home.join(".agents/skills"), "deploy", "Ship it");
        skill(workspace.join(".agents/skills"), "audit", "Check it");

        let commands = catalog(&home, &workspace);

        let deploy = named(&commands, "deploy").unwrap();
        assert_eq!(deploy.invocation, "$deploy ");
        assert_eq!(deploy.description, "Ship it");
        assert_eq!(deploy.origin, CommandOrigin::Personal);
        assert_eq!(named(&commands, "audit").unwrap().origin, CommandOrigin::Project);

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// Pins the precedence `catalog` and `push` actually implement: root
    /// entries are scanned workspace-first, then home, and `push` lets the
    /// later write win, so a personal skill shadows a project skill of the
    /// same name. This is a regression net, not a new rule — if the roots
    /// array in `catalog` is ever reordered by accident, this test should
    /// catch it.
    #[test]
    fn a_personal_skill_shadows_a_project_skill_of_the_same_name() {
        let home = scratch("shadow-personal-home");
        let workspace = scratch("shadow-personal-workspace");
        skill(workspace.join(".agents/skills"), "shared", "Project version");
        skill(home.join(".agents/skills"), "shared", "Personal version");

        let commands = catalog(&home, &workspace);

        assert_eq!(
            commands.iter().filter(|command| command.name == "shared").count(),
            1
        );
        let shared = named(&commands, "shared").unwrap();
        assert_eq!(shared.description, "Personal version");
        assert_eq!(shared.origin, CommandOrigin::Personal);

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// The `.codex/skills` layout is not documented any more, but files left
    /// there should keep working.
    #[test]
    fn the_older_codex_skills_layout_still_resolves() {
        let home = scratch("legacy-home");
        let workspace = scratch("legacy-workspace");
        skill(home.join(".codex/skills"), "legacy", "Still here");

        assert!(named(&catalog(&home, &workspace), "legacy").is_some());

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn custom_prompts_are_namespaced_under_prompts() {
        let home = scratch("prompts-home");
        let workspace = scratch("prompts-workspace");
        write(
            home.join(".codex/prompts/draftpr.md"),
            "---\ndescription: Draft a pull request\n---\n",
        );

        let commands = catalog(&home, &workspace);

        let draft = named(&commands, "prompts:draftpr").unwrap();
        assert_eq!(draft.invocation, "/prompts:draftpr ");
        assert_eq!(draft.description, "Draft a pull request");

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// Codex does not namespace plugin skills, so a plugin skill is indexed
    /// under its bare name.
    #[test]
    fn plugin_skills_are_indexed_without_a_namespace() {
        let home = scratch("plugin-home");
        let workspace = scratch("plugin-workspace");
        let install = home.join(".codex/plugins/cache/openai/templates/0.1.0");
        write(
            install.join(".codex-plugin/plugin.json"),
            r#"{"name":"templates","skills":"./skills/"}"#,
        );
        skill(install.join("skills"), "letterhead", "Minimal letterhead");

        let commands = catalog(&home, &workspace);

        let letterhead = named(&commands, "letterhead").unwrap();
        assert_eq!(letterhead.invocation, "$letterhead ");
        assert_eq!(
            letterhead.origin,
            CommandOrigin::Plugin {
                plugin: "templates".to_owned(),
                marketplace: "openai".to_owned(),
            }
        );

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// Pins the other half of the precedence `catalog` implements: plugins
    /// are scanned last, so a plugin skill shadows a personal skill of the
    /// same bare name, even though Codex does not namespace plugin skills.
    /// This is a regression net, not a new rule.
    #[test]
    fn a_plugin_skill_shadows_a_personal_skill_of_the_same_name() {
        let home = scratch("shadow-plugin-home");
        let workspace = scratch("shadow-plugin-workspace");
        skill(home.join(".agents/skills"), "shared", "Personal version");
        let install = home.join(".codex/plugins/cache/openai/templates/0.1.0");
        write(
            install.join(".codex-plugin/plugin.json"),
            r#"{"name":"templates","skills":"./skills/"}"#,
        );
        skill(install.join("skills"), "shared", "Plugin version");

        let commands = catalog(&home, &workspace);

        assert_eq!(
            commands.iter().filter(|command| command.name == "shared").count(),
            1
        );
        let shared = named(&commands, "shared").unwrap();
        assert_eq!(shared.description, "Plugin version");
        assert_eq!(
            shared.origin,
            CommandOrigin::Plugin {
                plugin: "templates".to_owned(),
                marketplace: "openai".to_owned(),
            }
        );

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// Codex ships no live-version pointer, so `installed_plugins` has to
    /// pick one version directory itself. Only the highest-sorted version
    /// should be scanned: an older version's skill must not survive an
    /// update, and a same-named skill in both versions must resolve to the
    /// newer one rather than to `read_dir` order.
    #[test]
    fn only_the_highest_version_of_a_plugin_is_scanned() {
        let home = scratch("versions-home");
        let workspace = scratch("versions-workspace");
        let cache = home.join(".codex/plugins/cache/openai/templates");
        write(
            cache.join("0.1.0/.codex-plugin/plugin.json"),
            r#"{"name":"templates","skills":"./skills/"}"#,
        );
        skill(cache.join("0.1.0/skills"), "old-only", "From the old version");
        write(
            cache.join("0.2.0/.codex-plugin/plugin.json"),
            r#"{"name":"templates","skills":"./skills/"}"#,
        );
        skill(cache.join("0.2.0/skills"), "new-only", "From the new version");

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "new-only").is_some());
        assert!(named(&commands, "old-only").is_none());

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_plugin_without_a_skills_key_falls_back_to_the_default_directory() {
        let home = scratch("default-home");
        let workspace = scratch("default-workspace");
        let install = home.join(".codex/plugins/cache/market/plain/1.0.0");
        write(install.join(".codex-plugin/plugin.json"), r#"{"name":"plain"}"#);
        skill(install.join("skills"), "included", "Found anyway");

        assert!(named(&catalog(&home, &workspace), "included").is_some());

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_broken_plugin_manifest_does_not_empty_the_catalog() {
        let home = scratch("broken-home");
        let workspace = scratch("broken-workspace");
        let install = home.join(".codex/plugins/cache/market/broken/1.0.0");
        write(install.join(".codex-plugin/plugin.json"), "{not json");
        skill(home.join(".agents/skills"), "personal", "Survives");

        assert!(named(&catalog(&home, &workspace), "personal").is_some());

        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn an_empty_home_yields_an_empty_catalog_rather_than_failing() {
        let home = scratch("empty-home");
        let workspace = scratch("empty-workspace");
        assert!(catalog(&home, &workspace).is_empty());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }
}
