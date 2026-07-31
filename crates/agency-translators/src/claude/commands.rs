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
        assert_eq!(found[1].install_path, PathBuf::from("/cache/superpowers/6.2.0"));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_missing_or_malformed_installed_plugins_file_yields_nothing() {
        let home = scratch("malformed");
        assert!(installed_plugins(&home).is_empty());
        write(home.join(".claude/plugins/installed_plugins.json"), "{not json");
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
}
