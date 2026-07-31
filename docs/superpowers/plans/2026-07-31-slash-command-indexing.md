# Slash Command Indexing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Index every slash command an agent can run — including plugin-provided skills and commands — by moving discovery out of the desktop crate and into the agent-level translators.

**Architecture:** `agency-translator-api` gains a `CommandCatalog` trait, an `AgentCommand` type, and mechanical discovery helpers. `agency-translators` implements the trait once per provider, owning that provider's directory layout, plugin cache, enablement rules, and invocation sigil. `agency-desktop` keeps Agency's own commands, the completion state machine, matching, and rendering, and refreshes the agent half of the catalog through typed events backed by a blocking effect.

**Tech Stack:** Rust 2024 edition, `serde`/`serde_json`, `iced` 0.14 for the desktop event loop, `tokio` (`rt` feature) for `spawn_blocking`.

## Global Constraints

- Rust edition 2024, `rust-version = "1.95"`, workspace-level dependency versions.
- No new dependency beyond `tokio = { version = "1", features = ["rt"] }` in `agency-desktop`. Frontmatter is parsed by hand; do not add a YAML crate.
- `agency-translator-api` must stay provider-neutral. No string literal naming Claude or Codex belongs in it.
- Every discovery function that touches `$HOME` takes `home: &Path` as a parameter so tests can point it at a temporary directory. Only the trait implementation reads the real `HOME`.
- A malformed or unreadable file, directory, or JSON document drops that one entry or that one root. It never aborts the catalog.
- Run `cargo test --workspace` before each commit. Run `cargo clippy --workspace --all-targets` before the final commit of each task.
- Follow the surrounding comment style: explain *why* a rule exists, not what the code does.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/agency-translator-api/src/commands.rs` (new) | `AgentCommand`, `CommandOrigin`, `CommandCatalog` trait |
| `crates/agency-translator-api/src/discovery.rs` (new) | Frontmatter parsing, skill-directory and command-file walking |
| `crates/agency-translator-api/src/lib.rs` (modify) | Declare and re-export the two new modules |
| `crates/agency-translators/src/claude/commands.rs` (new) | Claude discovery: settings chain, installed plugins, manifests, built-ins |
| `crates/agency-translators/src/claude.rs` (modify) | Declare `mod commands;`, implement `CommandCatalog` |
| `crates/agency-translators/src/codex/commands.rs` (new) | Codex discovery: `.agents/skills`, prompts, plugin manifests |
| `crates/agency-translators/src/codex.rs` (modify) | Declare `mod commands;`, implement `CommandCatalog` |
| `crates/agency-translators/src/lib.rs` (modify) | `command_catalog(&ClientId)` registry |
| `crates/agency-desktop/src/slash_commands.rs` (modify) | Agency commands, merge, segment matching; discovery removed |
| `crates/agency-desktop/src/main.rs` (modify) | Catalog events, blocking effect, refresh triggers |

`claude.rs` keeps its name and gains a sibling `claude/` directory holding `commands.rs`. Rust's module system resolves `mod commands;` inside `claude.rs` to `claude/commands.rs`, so no file needs renaming.

---

### Task 1: Frontmatter parsing

**Files:**
- Create: `crates/agency-translator-api/src/discovery.rs`
- Modify: `crates/agency-translator-api/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `discovery.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub struct Frontmatter { pub name: Option<String>, pub description: Option<String>, pub argument_hint: Option<String> }`, `pub fn frontmatter(contents: &str) -> Frontmatter`, `pub fn describe(contents: &str) -> String`, `pub const DEFAULT_DESCRIPTION: &str`.

**Background:** Every skill and command file opens with a YAML frontmatter block fenced by `---` lines. The existing desktop code takes the first line that is neither empty nor `---`, which yields `name: brainstorming` instead of the description. We need the `description` and `argument-hint` keys specifically.

- [ ] **Step 1: Write the failing tests**

Create `crates/agency-translator-api/src/discovery.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\nname: brainstorming\ndescription: Turn ideas into designs\nargument-hint: [topic]\n---\n\n# Brainstorming\n\nBody text.\n";

    #[test]
    fn frontmatter_keys_are_read_by_name() {
        let parsed = frontmatter(SKILL);
        assert_eq!(parsed.name.as_deref(), Some("brainstorming"));
        assert_eq!(parsed.description.as_deref(), Some("Turn ideas into designs"));
        assert_eq!(parsed.argument_hint.as_deref(), Some("[topic]"));
    }

    /// Regression: the description used to be taken as the first line that was
    /// neither blank nor a fence, which is the `name` key in every real file.
    #[test]
    fn the_description_is_not_the_name_key() {
        assert_eq!(describe(SKILL), "Turn ideas into designs");
    }

    #[test]
    fn values_keep_colons_and_lose_surrounding_quotes() {
        let parsed = frontmatter("---\ndescription: \"Use when: you are stuck\"\n---\n");
        assert_eq!(
            parsed.description.as_deref(),
            Some("Use when: you are stuck")
        );
        let single = frontmatter("---\ndescription: 'Quoted'\n---\n");
        assert_eq!(single.description.as_deref(), Some("Quoted"));
    }

    #[test]
    fn a_file_without_frontmatter_falls_back_to_its_first_prose_line() {
        assert_eq!(describe("# Deploy\n\nShips the app.\n"), "Deploy");
        assert_eq!(describe("Ships the app.\n"), "Ships the app.");
    }

    #[test]
    fn an_unterminated_block_is_not_treated_as_frontmatter() {
        let parsed = frontmatter("---\ndescription: never closed\n");
        assert_eq!(parsed.description, None);
        assert_eq!(describe("---\ndescription: never closed\n"), "description: never closed");
    }

    #[test]
    fn an_empty_or_bodyless_file_falls_back_to_the_default() {
        assert_eq!(describe(""), DEFAULT_DESCRIPTION);
        assert_eq!(describe("---\nname: bare\n---\n"), DEFAULT_DESCRIPTION);
    }

    #[test]
    fn blank_values_and_unknown_keys_are_ignored() {
        let parsed = frontmatter("---\ndescription:\nmodel: opus\n---\nBody\n");
        assert_eq!(parsed.description, None);
        assert_eq!(describe("---\ndescription:\nmodel: opus\n---\nBody\n"), "Body");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agency-translator-api discovery`
Expected: FAIL — `discovery.rs` is not yet declared as a module, so the crate does not compile.

- [ ] **Step 3: Declare the module**

Add to the top of `crates/agency-translator-api/src/lib.rs`, above the existing `use` statements:

```rust
pub mod discovery;
```

- [ ] **Step 4: Write the implementation**

Add above the test module in `crates/agency-translator-api/src/discovery.rs`:

```rust
//! Mechanical helpers every provider needs to read skills and commands off
//! disk. The shapes are the same everywhere — a fenced frontmatter block
//! followed by markdown — so only the directory layouts and naming rules stay
//! provider specific.

/// What a completion row shows when a file offers no description at all.
pub const DEFAULT_DESCRIPTION: &str = "Agent skill or command";

/// The frontmatter keys that affect how a command is listed. Everything else
/// in the block governs the agent's own behavior and is none of Agency's
/// business.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

/// Reads the fenced frontmatter block, if the file opens with one.
///
/// Values are single-line scalars. A folded or block scalar (`>` or `|`)
/// yields nothing for that key rather than the marker character, so a file
/// using one falls back to its prose line.
pub fn frontmatter(contents: &str) -> Frontmatter {
    let mut parsed = Frontmatter::default();
    for line in frontmatter_lines(contents) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => parsed.name = Some(value.to_owned()),
            "description" => parsed.description = Some(value.to_owned()),
            "argument-hint" => parsed.argument_hint = Some(value.to_owned()),
            _ => {}
        }
    }
    parsed
}

/// The description to list a command under: its frontmatter `description`, the
/// first prose line of its body, or a generic label.
pub fn describe(contents: &str) -> String {
    if let Some(description) = frontmatter(contents).description {
        return description;
    }
    body(contents)
        .lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .map_or_else(|| DEFAULT_DESCRIPTION.to_owned(), str::to_owned)
}

/// The lines inside the frontmatter fence, or none when the file does not open
/// with one. An unterminated fence is not frontmatter: treating it as such
/// would swallow a whole file that merely starts with a horizontal rule.
fn frontmatter_lines(contents: &str) -> impl Iterator<Item = &str> {
    let inside = contents
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---").map(|(block, _)| block))
        .unwrap_or_default();
    inside.lines()
}

/// Everything after the frontmatter fence, or the whole file without one.
fn body(contents: &str) -> &str {
    contents
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---").map(|(_, body)| body))
        .unwrap_or(contents)
}

/// Strips one matching pair of surrounding quotes. A value like
/// `"Use when: you are stuck"` has to survive intact, colon and all.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|value| value.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p agency-translator-api discovery`
Expected: PASS, 7 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/agency-translator-api/src/discovery.rs crates/agency-translator-api/src/lib.rs
git commit -m "feat(translator-api): parse skill and command frontmatter"
```

---

### Task 2: Command vocabulary and directory walkers

**Files:**
- Create: `crates/agency-translator-api/src/commands.rs`
- Modify: `crates/agency-translator-api/src/discovery.rs`, `crates/agency-translator-api/src/lib.rs`
- Test: inline test modules in both files

**Interfaces:**
- Consumes: `describe`, `frontmatter` from Task 1.
- Produces:
  - `pub struct AgentCommand { pub name: String, pub description: String, pub invocation: String, pub argument_hint: Option<String>, pub origin: CommandOrigin }`
  - `pub enum CommandOrigin { BuiltIn, Personal, Project, Plugin { plugin: String, marketplace: String } }`
  - `pub trait CommandCatalog: Send + Sync { fn commands(&self, workspace: &Path) -> Vec<AgentCommand>; }`
  - `pub struct DiscoveredFile { pub name: String, pub path: PathBuf }`
  - `pub fn skill_directories(root: &Path) -> Vec<DiscoveredFile>`
  - `pub fn command_files(root: &Path) -> Vec<DiscoveredFile>`

**Background:** Skills are directories containing `SKILL.md`, named by the directory. Commands are `.md` files, named by the file stem; Claude Code's documented naming takes the stem regardless of nesting, so the walker recurses but does not build a path-based name. Results are sorted so a catalog is reproducible.

- [ ] **Step 1: Write the failing walker tests**

Append to `crates/agency-translator-api/src/discovery.rs`, inside the existing `mod tests`:

```rust
    use std::fs;
    use std::path::PathBuf;

    /// A throwaway directory named for the test that asked for it. The api
    /// crate has no dev-dependency on a tempdir crate and does not need one.
    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agency-discovery-{}-{label}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn skills_are_directories_holding_a_skill_file() {
        let root = scratch("skills");
        fs::create_dir_all(root.join("deploy")).unwrap();
        fs::write(root.join("deploy/SKILL.md"), "---\ndescription: Ship\n---\n").unwrap();
        fs::create_dir_all(root.join("audit")).unwrap();
        fs::write(root.join("audit/SKILL.md"), "body").unwrap();
        // A directory with no SKILL.md is not a skill.
        fs::create_dir_all(root.join("assets")).unwrap();

        let found = skill_directories(&root);

        assert_eq!(
            found.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            ["audit", "deploy"]
        );
        assert_eq!(found[1].path, root.join("deploy/SKILL.md"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commands_are_markdown_files_named_by_stem_at_any_depth() {
        let root = scratch("commands");
        fs::write(root.join("deploy.md"), "body").unwrap();
        fs::create_dir_all(root.join("git")).unwrap();
        fs::write(root.join("git/commit.md"), "body").unwrap();
        // Non-markdown files are not commands.
        fs::write(root.join("README.txt"), "body").unwrap();

        let found = command_files(&root);

        assert_eq!(
            found.iter().map(|file| file.name.as_str()).collect::<Vec<_>>(),
            ["commit", "deploy"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_root_yields_nothing_rather_than_failing() {
        assert!(skill_directories(Path::new("/does/not/exist")).is_empty());
        assert!(command_files(Path::new("/does/not/exist")).is_empty());
    }
```

Add `use std::path::Path;` to the top of the test module if not already in scope through `super::*`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agency-translator-api discovery`
Expected: FAIL — `cannot find function 'skill_directories' in this scope`.

- [ ] **Step 3: Write the walkers**

Add to `crates/agency-translator-api/src/discovery.rs`, above the test module:

```rust
use std::fs;
use std::path::{Path, PathBuf};

/// One markdown file that defines a command, and the name it is defined under
/// before any provider-specific namespacing is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub name: String,
    pub path: PathBuf,
}

/// How deep `command_files` will recurse. A symlink loop inside a plugin must
/// not be able to hang the indexer.
const MAX_DEPTH: usize = 8;

/// Skills laid out as `<root>/<name>/SKILL.md`. A directory without a
/// `SKILL.md` is supporting material, not a skill.
pub fn skill_directories(root: &Path) -> Vec<DiscoveredFile> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().join("SKILL.md");
            path.is_file().then(|| DiscoveredFile {
                name: entry.file_name().to_string_lossy().into_owned(),
                path,
            })
        })
        .collect::<Vec<_>>();
    sort(&mut found);
    found
}

/// Commands laid out as markdown files under `root`. Claude Code names a
/// command after its file stem however deeply it is nested, so the walk
/// recurses without building a path-based name.
pub fn command_files(root: &Path) -> Vec<DiscoveredFile> {
    let mut found = Vec::new();
    collect_command_files(root, 0, &mut found);
    sort(&mut found);
    found
}

fn collect_command_files(root: &Path, depth: usize, found: &mut Vec<DiscoveredFile>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_command_files(&path, depth + 1, found);
        } else if path.extension().is_some_and(|extension| extension == "md")
            && let Some(stem) = path.file_stem()
        {
            found.push(DiscoveredFile {
                name: stem.to_string_lossy().into_owned(),
                path,
            });
        }
    }
}

/// Directory order is not stable across filesystems, and an unstable catalog
/// would reorder the completion list between runs.
fn sort(found: &mut [DiscoveredFile]) {
    found.sort_by(|left, right| left.name.cmp(&right.name).then(left.path.cmp(&right.path)));
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agency-translator-api discovery`
Expected: PASS, 10 tests.

- [ ] **Step 5: Write the failing vocabulary test**

Create `crates/agency-translator-api/src/commands.rs`:

```rust
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
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo test -p agency-translator-api commands`
Expected: FAIL — the module is not declared, so the crate does not compile.

- [ ] **Step 7: Write the vocabulary**

Add `pub mod commands;` to `crates/agency-translator-api/src/lib.rs` beside `pub mod discovery;`, then add above the test module in `commands.rs`:

```rust
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
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p agency-translator-api`
Expected: PASS, all tests including the pre-existing `tools` tests.

- [ ] **Step 9: Commit**

```bash
git add crates/agency-translator-api/src/
git commit -m "feat(translator-api): add the neutral command catalog vocabulary"
```

---

### Task 3: Claude plugin enablement and install paths

**Files:**
- Create: `crates/agency-translators/src/claude/commands.rs`
- Modify: `crates/agency-translators/src/claude.rs`
- Test: inline `#[cfg(test)] mod tests` in `claude/commands.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (crate-private):
  - `struct InstalledPlugin { name: String, marketplace: String, install_path: PathBuf }`
  - `fn installed_plugins(home: &Path) -> Vec<InstalledPlugin>`
  - `fn enabled_plugins(home: &Path, workspace: &Path) -> HashMap<String, bool>`
  - `struct Manifest { default_enabled: Option<bool>, skills: Vec<PathBuf>, commands: Option<Vec<PathBuf>> }`
  - `fn manifest(install_path: &Path) -> Manifest`
  - `fn is_enabled(plugin: &InstalledPlugin, enabled: &HashMap<String, bool>, manifest: &Manifest) -> bool`

**Background, all confirmed against Claude Code's documentation:**

- `~/.claude/plugins/installed_plugins.json` has shape `{"version": 2, "plugins": {"<name>@<marketplace>": [{"scope": ..., "installPath": ..., "version": ...}]}}`. The `installPath` is authoritative; several versions of one plugin can sit in the cache at once.
- `enabledPlugins` is keyed by the same `<name>@<marketplace>` string. Settings scopes in increasing precedence: `~/.claude/settings.json`, `<workspace>/.claude/settings.json`, `<workspace>/.claude/settings.local.json`.
- When no scope names the plugin, `defaultEnabled` in `<installPath>/.claude-plugin/plugin.json` decides. Absent that, the plugin is enabled.
- Manifest `commands` **replaces** the default `commands/` scan. Manifest `skills` **adds to** the default `skills/` scan. Both accept a string or an array of strings, relative to the plugin root.

- [ ] **Step 1: Write the failing tests**

Create `crates/agency-translators/src/claude/commands.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agency-translators claude::commands`
Expected: FAIL — the module is not declared, so the crate does not compile.

- [ ] **Step 3: Declare the module**

Add to the top of `crates/agency-translators/src/claude.rs`, immediately after the existing `use` block:

```rust
mod commands;
```

- [ ] **Step 4: Write the implementation**

Add above the test module in `crates/agency-translators/src/claude/commands.rs`:

```rust
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
    found.sort_by(|left, right| left.key().cmp(&right.key()));
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p agency-translators claude::commands`
Expected: PASS, 8 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/agency-translators/src/claude.rs crates/agency-translators/src/claude/commands.rs
git commit -m "feat(translators): resolve Claude plugin install paths and enablement"
```

---

### Task 4: Claude catalog assembly

**Files:**
- Modify: `crates/agency-translators/src/claude/commands.rs`, `crates/agency-translators/src/claude.rs`, `crates/agency-translators/Cargo.toml`
- Test: inline test module in `claude/commands.rs`

**Interfaces:**
- Consumes: Task 2's `AgentCommand`, `CommandOrigin`, `CommandCatalog`, `describe`, `frontmatter`, `skill_directories`, `command_files`; Task 3's `installed_plugins`, `enabled_plugins`, `manifest`, `is_enabled`.
- Produces: `pub(super) fn catalog(home: &Path, workspace: &Path) -> Vec<AgentCommand>` and `impl CommandCatalog for ClaudeTranslator`.

**Background, all confirmed against the documentation:**

- Personal and project entries take their command name from the directory or file name. Frontmatter `name` is a display label only at these levels.
- A plugin skill's frontmatter `name` replaces the last segment; the `<plugin>:` prefix stays. A plugin with a root `SKILL.md`, no `skills/` directory and no `skills` manifest key is a single-skill plugin named by frontmatter `name`, falling back to the plugin's own name.
- Personal overrides project; either overrides a built-in of the same name. A skill beats a command of the same name. Plugin entries are namespaced and never collide with the other levels.

- [ ] **Step 1: Add the api dependency**

`crates/agency-translators/Cargo.toml` already depends on `agency-translator-api`. Confirm with:

Run: `grep agency-translator-api crates/agency-translators/Cargo.toml`
Expected: `agency-translator-api = { path = "../agency-translator-api" }`

- [ ] **Step 2: Write the failing tests**

Append to the `mod tests` block in `crates/agency-translators/src/claude/commands.rs`:

```rust
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

        assert_eq!(named(&commands, "deploy").unwrap().description, "From the skill");
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    /// Sets up a plugin in the cache and registers it in
    /// `installed_plugins.json` under `<name>@marketplace`.
    fn install(home: &Path, name: &str, manifest_json: Option<&str>) -> PathBuf {
        let install_path = home.join("cache").join(name);
        fs::create_dir_all(&install_path).unwrap();
        if let Some(manifest_json) = manifest_json {
            write(install_path.join(".claude-plugin/plugin.json"), manifest_json);
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
        skill(install.join("skills"), "brainstorming", "Turn ideas into designs");
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

    #[test]
    fn a_plugin_root_skill_file_becomes_a_single_skill_plugin() {
        let home = scratch("root-skill-home");
        let workspace = scratch("root-skill-workspace");
        let install = install(&home, "solo", None);
        write(install.join("SKILL.md"), "---\ndescription: The only one\n---\n");

        let commands = catalog(&home, &workspace);

        assert_eq!(named(&commands, "solo:solo").unwrap().description, "The only one");
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_disabled_plugin_contributes_nothing() {
        let home = scratch("disabled-home");
        let workspace = scratch("disabled-workspace");
        let install = install(&home, "off", None);
        skill(install.join("skills"), "hidden", "Should not appear");
        write(
            home.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"off@marketplace":false}}"#,
        );

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "off:hidden").is_none());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn a_plugin_opted_out_by_default_is_restored_by_a_settings_entry() {
        let home = scratch("opt-in-home");
        let workspace = scratch("opt-in-workspace");
        let install = install(&home, "optional", Some(r#"{"name":"optional","defaultEnabled":false}"#));
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
        let install = install(&home, "custom", Some(r#"{"name":"custom","commands":["./cmd/"]}"#));
        write(install.join("commands/ignored.md"), "---\ndescription: No\n---\n");
        write(install.join("cmd/used.md"), "---\ndescription: Yes\n---\n");

        let commands = catalog(&home, &workspace);

        assert!(named(&commands, "custom:used").is_some());
        assert!(named(&commands, "custom:ignored").is_none());
        fs::remove_dir_all(home).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn manifest_skill_paths_add_to_the_default_directory() {
        let home = scratch("extend-home");
        let workspace = scratch("extend-workspace");
        let install = install(&home, "both", Some(r#"{"name":"both","skills":["./extra/"]}"#));
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
        assert!(named(&commands, "code-review").unwrap().origin.is_built_in());

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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p agency-translators claude::commands`
Expected: FAIL — `cannot find function 'catalog' in this scope`.

- [ ] **Step 4: Write the catalog assembly**

Add to the `use` block at the top of `crates/agency-translators/src/claude/commands.rs`:

```rust
use agency_translator_api::commands::{AgentCommand, CommandOrigin};
use agency_translator_api::discovery::{self, DiscoveredFile};
```

Then add above the test module:

```rust
/// The commands Claude Code ships. Only the ones that make sense to send from
/// Agency's composer are listed: session controls such as `/exit` or `/login`
/// act on Claude Code's own terminal UI, which Agency does not present. Claude
/// Code's `/init` is omitted because Agency owns that command.
const BUILT_INS: [(&str, &str); 27] = [
    ("agents", "Create or manage subagents"),
    ("batch", "Orchestrate large-scale changes across a codebase in parallel"),
    ("claude-api", "Load Claude API reference material"),
    ("code-review", "Review the current diff for bugs and cleanup opportunities"),
    ("compact", "Free up context by summarizing the conversation"),
    ("context", "Visualize current context usage"),
    ("cost", "Show token usage and costs for the current session"),
    ("dataviz", "Design guidance for charts, graphs, and dashboards"),
    ("debug", "Enable debug logging and troubleshoot issues"),
    ("deep-research", "Fan out web searches and synthesize a cited report"),
    ("design-sync", "Convert a React design system and upload it to Claude Design"),
    ("diff", "Open an interactive diff viewer for uncommitted changes"),
    ("doctor", "Run a setup checkup that diagnoses and fixes issues"),
    ("export", "Export the current conversation as plain text"),
    ("fewer-permission-prompts", "Add an allowlist to reduce permission prompts"),
    ("goal", "Set a goal to keep working until a condition is met"),
    ("hooks", "View hook configurations for tool events"),
    ("insights", "Generate a usage insights report"),
    ("loop", "Run a prompt repeatedly while the session stays open"),
    ("mcp", "Manage MCP server connections"),
    ("memory", "Edit CLAUDE.md memory files"),
    ("model", "Switch the model for this session"),
    ("permissions", "View and manage permission rules"),
    ("rewind", "Roll code and conversation back to a checkpoint"),
    ("status", "Show the current session status and model"),
    ("usage", "Show token usage and costs for the current session"),
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
        .map(|(name, description)| command(name.to_owned(), description.to_owned(), None, CommandOrigin::BuiltIn))
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

    let mut skill_roots = vec![plugin.install_path.join("skills")];
    skill_roots.extend(manifest.skills.iter().cloned());
    let mut found_any_skill = false;
    for root in skill_roots {
        for file in discovery::skill_directories(&root) {
            found_any_skill = true;
            let contents = read(&file.path);
            let parsed = discovery::frontmatter(&contents);
            // In a plugin skill the frontmatter name replaces the last segment
            // and the plugin prefix stays in place.
            let segment = parsed.name.clone().unwrap_or(file.name);
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
        let segment = parsed.name.clone().unwrap_or_else(|| plugin.name.clone());
        commands.push(command(
            format!("{}:{segment}", plugin.name),
            discovery::describe(&contents),
            parsed.argument_hint,
            origin,
        ));
    }

    commands
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p agency-translators claude::commands`
Expected: PASS, 20 tests.

- [ ] **Step 6: Implement the trait**

Add to `crates/agency-translators/src/claude.rs`, after the existing `impl LiveEventTranslator for ClaudeTranslator` block:

```rust
impl CommandCatalog for ClaudeTranslator {
    fn commands(&self, workspace: &Path) -> Vec<AgentCommand> {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return Vec::new();
        };
        commands::catalog(&home, workspace)
    }
}
```

Extend the file's `use` block with:

```rust
use std::path::{Path, PathBuf};

use agency_translator_api::commands::{AgentCommand, CommandCatalog};
```

- [ ] **Step 7: Run the whole crate's tests**

Run: `cargo test -p agency-translators`
Expected: PASS, including the pre-existing translator tests.

- [ ] **Step 8: Commit**

```bash
git add crates/agency-translators/src/claude.rs crates/agency-translators/src/claude/commands.rs
git commit -m "feat(translators): index Claude skills, commands, and plugin entries"
```

---

### Task 5: Codex catalog

**Files:**
- Create: `crates/agency-translators/src/codex/commands.rs`
- Modify: `crates/agency-translators/src/codex.rs`
- Test: inline test module in `codex/commands.rs`

**Interfaces:**
- Consumes: Task 2's `AgentCommand`, `CommandOrigin`, `CommandCatalog`, `discovery`.
- Produces: `pub(super) fn catalog(home: &Path, workspace: &Path) -> Vec<AgentCommand>` and `impl CommandCatalog for CodexTranslator`.

**Background, confirmed against Codex's documentation:**

- Skills live at `$HOME/.agents/skills` (personal) and `<repo>/.agents/skills` (repository). The `.codex/skills` paths Agency scans today are not the documented locations; they are kept as a fallback because an older layout may still have files there and scanning a missing directory costs nothing.
- Skills are mentioned with `$name`. This matches Agency's existing insertion form.
- Codex does **not** namespace plugin-provided skills. They are indexed under their bare names.
- Custom prompts at `~/.codex/prompts/*.md` are invoked as `/prompts:<name>`. They are deprecated in favour of skills but still load.
- Plugins install to `~/.codex/plugins/cache/<marketplace>/<plugin>/<version>/` and declare their skills path in `.codex-plugin/plugin.json` under a `skills` key.
- `/etc/codex/skills` is a documented machine-wide location and is out of scope.

- [ ] **Step 1: Write the failing tests**

Create `crates/agency-translators/src/codex/commands.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agency-translators codex::commands`
Expected: FAIL — the module is not declared, so the crate does not compile.

- [ ] **Step 3: Declare the module**

Add to `crates/agency-translators/src/codex.rs`, immediately after the existing `use` block:

```rust
mod commands;
```

- [ ] **Step 4: Write the implementation**

Add above the test module in `crates/agency-translators/src/codex/commands.rs`:

```rust
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
            for version in versions.flatten() {
                let install_path = version.path();
                if !install_path.join(".codex-plugin/plugin.json").is_file() {
                    continue;
                }
                found.push(CodexPlugin {
                    name: plugin.file_name().to_string_lossy().into_owned(),
                    marketplace: marketplace.file_name().to_string_lossy().into_owned(),
                    skill_roots: skill_roots(&install_path),
                });
            }
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p agency-translators codex::commands`
Expected: PASS, 7 tests.

- [ ] **Step 6: Implement the trait**

Add to `crates/agency-translators/src/codex.rs`, after the existing `impl LiveEventTranslator for CodexTranslator` block:

```rust
impl CommandCatalog for CodexTranslator {
    fn commands(&self, workspace: &Path) -> Vec<AgentCommand> {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return Vec::new();
        };
        commands::catalog(&home, workspace)
    }
}
```

Extend the file's `use` block with:

```rust
use std::path::{Path, PathBuf};

use agency_translator_api::commands::{AgentCommand, CommandCatalog};
```

- [ ] **Step 7: Register both catalogs**

Add to `crates/agency-translators/src/lib.rs`, after the existing `built_in` function:

```rust
/// The command catalog for a client, when one is registered. This is the same
/// registry as [`built_in`], kept separate because a translator may know how
/// to read a transcript without knowing how to enumerate commands.
pub fn command_catalog(client: &ClientId) -> Option<Box<dyn CommandCatalog>> {
    match client.0.as_str() {
        "claude-code" => Some(Box::new(claude::ClaudeTranslator::default())),
        "codex" => Some(Box::new(codex::CodexTranslator)),
        _ => None,
    }
}
```

Extend that file's `use` block with `commands::CommandCatalog`:

```rust
use agency_translator_api::commands::CommandCatalog;
```

- [ ] **Step 8: Add a registry test**

Append to the `mod tests` block in `crates/agency-translators/src/lib.rs`:

```rust
    #[test]
    fn both_built_in_clients_expose_a_command_catalog() {
        assert!(command_catalog(&ClientId::new("claude-code")).is_some());
        assert!(command_catalog(&ClientId::new("codex")).is_some());
        assert!(command_catalog(&ClientId::new("nobody")).is_none());
    }
```

- [ ] **Step 9: Run the whole crate's tests**

Run: `cargo test -p agency-translators`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/agency-translators/src/
git commit -m "feat(translators): index Codex skills, prompts, and plugin skills"
```

---

### Task 6: Segment matching in the composer

**Files:**
- Modify: `crates/agency-desktop/src/slash_commands.rs`
- Test: the existing `#[cfg(test)] mod tests` in that file

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub fn matches(command: &str, input: &str) -> bool`, used by the existing `slash_command_completions`.

**Background:** `slash_command_completions` filters with `completion.command.starts_with(input)`. Once plugin entries are namespaced, `/brain` finds nothing because the command is `/superpowers:brainstorming`. Matching must also try each `:`-delimited segment. `shared_completion_prefix` needs no change: it returns `None` when the common prefix is not longer than the input, which is already the fallback that makes Tab accept the highlighted row.

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block in `crates/agency-desktop/src/slash_commands.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agency-desktop slash_commands`
Expected: FAIL — `cannot find function 'matches' in this scope`, and the segment-count assertions fail.

- [ ] **Step 3: Write the implementation**

Replace `slash_command_completions` in `crates/agency-desktop/src/slash_commands.rs` with:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agency-desktop slash_commands`
Expected: PASS, including the pre-existing completion and Tab tests.

- [ ] **Step 5: Commit**

```bash
git add crates/agency-desktop/src/slash_commands.rs
git commit -m "feat(desktop): find namespaced commands by segment"
```

---

### Task 7: Merge translator catalogs into the composer

**Files:**
- Modify: `crates/agency-desktop/src/slash_commands.rs`
- Test: the existing `#[cfg(test)] mod tests` in that file

**Interfaces:**
- Consumes: Task 2's `AgentCommand`, `CommandOrigin`; Task 5's `agency_translators::command_catalog`.
- Produces:
  - `pub fn agency_commands() -> Vec<SlashCommandCompletion>`
  - `pub fn discover_agent_commands(providers: &[Provider], workspace: &Path) -> Vec<(Provider, AgentCommand)>`
  - `pub fn merge_catalog(agent: Vec<(Provider, AgentCommand)>) -> Vec<SlashCommandCompletion>`
- Removes: `slash_command_catalog`, `discover_skills`, `discover_claude_commands`, `push_agent_completion`, `CLAUDE_BUILT_INS`.

**Background:** Two providers can each offer a command of the same bare name. They stay as separate rows because their insertions differ — `$review` versus `/review`. Within one provider, shadowing was already resolved by that provider's translator, so the merge does no deduplication of its own.

- [ ] **Step 1: Write the failing tests**

Replace the `plugin_install_is_offered_as_an_agency_command`, `duplicate_names_are_kept_between_agents_and_replaced_within_one_agent`, and `claude_built_ins_are_available_and_can_be_overridden` tests in `crates/agency-desktop/src/slash_commands.rs` with:

```rust
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
```

Add to the test module's imports:

```rust
    use agency_translator_api::commands::CommandOrigin;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agency-desktop slash_commands`
Expected: FAIL — `cannot find function 'merge_catalog' in this scope`.

- [ ] **Step 3: Delete the old discovery code**

Remove from `crates/agency-desktop/src/slash_commands.rs`: `slash_command_catalog`, `discover_skills`, `discover_claude_commands`, `push_agent_completion`, and the `CLAUDE_BUILT_INS` constant. Remove the now-unused `use std::fs;` if nothing else in the file uses it — `initialize_workspace` and `load_codex_mcp` still do, so keep it.

- [ ] **Step 4: Write the replacement**

Add to `crates/agency-desktop/src/slash_commands.rs`, where `slash_command_catalog` used to be:

```rust
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
```

Extend the file's `use` block with:

```rust
use agency_translator_api::ClientId;
use agency_translator_api::commands::AgentCommand;
```

- [ ] **Step 5: Keep `main.rs` compiling**

Deleting `slash_command_catalog` breaks its two call sites. Wire them to the new
functions synchronously for now; Task 8 moves the work behind an effect.

In the `use slash_commands::{...}` list near line 58, replace
`slash_command_catalog` with `agency_commands, discover_agent_commands, merge_catalog`.

In `impl Default for Agency` (around line 933), replace:

```rust
        let slash_command_catalog = slash_command_catalog(&cwd);
```

with:

```rust
        let slash_command_catalog =
            merge_catalog(discover_agent_commands(&configured_agents, &cwd));
```

In the worktree-switch handler (around line 2294), replace:

```rust
        self.slash_command_catalog = slash_command_catalog(&self.cwd);
```

with:

```rust
        self.slash_command_catalog =
            merge_catalog(discover_agent_commands(&self.configured_agents, &self.cwd));
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p agency-desktop`
Expected: PASS. The crate compiles and the composer already lists plugin commands, synchronously.

- [ ] **Step 7: Commit**

```bash
git add crates/agency-desktop/src/slash_commands.rs crates/agency-desktop/src/main.rs
git commit -m "feat(desktop): build the command catalog from translator catalogs"
```

---

### Task 8: Refresh the catalog through typed events

**Files:**
- Modify: `crates/agency-desktop/src/main.rs`, `crates/agency-desktop/Cargo.toml`
- Test: the existing `#[cfg(test)] mod tests` in `main.rs`

**Interfaces:**
- Consumes: Task 7's `agency_commands`, `discover_agent_commands`, `merge_catalog`.
- Produces: `AppEvent::SlashCatalogRequested`, `AppEvent::SlashCatalogLoaded(Vec<(Provider, AgentCommand)>)`, `AppEvent::SlashCatalogFailed(String)`, and `Agency::boot`.

**Background:** `Agency::update` drains the event bus and batches the `Task`s that `reduce_event` returns, so a `Task::perform` is the idiomatic effect here. The catalog is built once at construction from Agency's own commands only, and the agent half arrives asynchronously. A failed load leaves the previous catalog alone: an empty command list is worse than a stale one.

- [ ] **Step 1: Add the tokio dependency**

Add to `crates/agency-desktop/Cargo.toml` under `[dependencies]`, in alphabetical position after `serde_json`:

```toml
tokio = { version = "1", features = ["rt"] }
```

- [ ] **Step 2: Write the failing tests**

Append to the `mod tests` block in `crates/agency-desktop/src/main.rs`:

```rust
    fn agent_command(name: &str) -> AgentCommand {
        AgentCommand {
            name: name.to_owned(),
            description: "Does a thing".to_owned(),
            invocation: format!("/{name} "),
            argument_hint: None,
            origin: agency_translator_api::commands::CommandOrigin::Personal,
        }
    }

    #[test]
    fn a_loaded_catalog_replaces_the_agent_half_and_keeps_agency_commands() {
        let mut catalog = slash_commands::agency_commands();
        let agency_count = catalog.len();

        catalog = slash_commands::merge_catalog(vec![(Provider::Claude, agent_command("deploy"))]);

        assert_eq!(catalog.len(), agency_count + 1);
        assert!(catalog.iter().any(|completion| completion.command == "/init"));
        assert!(catalog.iter().any(|completion| completion.command == "/deploy"));

        // A second load replaces the first rather than accumulating.
        catalog = slash_commands::merge_catalog(vec![(Provider::Claude, agent_command("audit"))]);
        assert_eq!(catalog.len(), agency_count + 1);
        assert!(!catalog.iter().any(|completion| completion.command == "/deploy"));
    }

    #[test]
    fn a_failed_load_leaves_the_previous_catalog_in_place() {
        let mut agency = Agency::default();
        agency.slash_command_catalog =
            slash_commands::merge_catalog(vec![(Provider::Claude, agent_command("deploy"))]);
        let before = agency.slash_command_catalog.clone();

        let _ = agency.reduce_event(AppEvent::SlashCatalogFailed("disk on fire".to_owned()));

        assert_eq!(agency.slash_command_catalog, before);
        assert_eq!(agency.notice.as_deref(), Some("disk on fire"));
    }

    #[test]
    fn a_loaded_catalog_is_stored_and_clears_no_notice() {
        let mut agency = Agency::default();

        let _ = agency.reduce_event(AppEvent::SlashCatalogLoaded(vec![(
            Provider::Claude,
            agent_command("deploy"),
        )]));

        assert!(
            agency
                .slash_command_catalog
                .iter()
                .any(|completion| completion.command == "/deploy")
        );
    }
```

Add `use agency_translator_api::commands::AgentCommand;` to the test module's imports.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p agency-desktop slash_catalog`
Expected: FAIL — `no variant named 'SlashCatalogFailed'`.

- [ ] **Step 4: Add the events**

Add to the `AppEvent` enum in `crates/agency-desktop/src/main.rs`, beside the other slash-command variants near line 866:

```rust
    /// Ask the configured agents what they can run here. Published at startup,
    /// on worktree switch, and after an install changes what is on disk.
    SlashCatalogRequested,
    SlashCatalogLoaded(Vec<(Provider, AgentCommand)>),
    SlashCatalogFailed(String),
```

Add `use agency_translator_api::commands::AgentCommand;` to the file's imports. The `use slash_commands::{...}` list already carries `agency_commands`, `discover_agent_commands`, and `merge_catalog` from Task 7.

- [ ] **Step 5: Handle the events**

Add to `reduce_event` in `crates/agency-desktop/src/main.rs`, beside the other slash-command arms:

```rust
            AppEvent::SlashCatalogRequested => {
                let workspace = self.cwd.clone();
                let providers = self.configured_agents.clone();
                return Task::perform(
                    async move {
                        // Discovery reads hundreds of directories under the
                        // plugin caches. Running it on the async runtime would
                        // stall every other effect for as long as it takes.
                        tokio::task::spawn_blocking(move || {
                            discover_agent_commands(&providers, &workspace)
                        })
                        .await
                        .map_err(|error| format!("Could not index agent commands: {error}"))
                    },
                    |result| match result {
                        Ok(commands) => AppEvent::SlashCatalogLoaded(commands),
                        Err(error) => AppEvent::SlashCatalogFailed(error),
                    },
                );
            }
            AppEvent::SlashCatalogLoaded(commands) => {
                self.slash_command_catalog = merge_catalog(commands);
            }
            AppEvent::SlashCatalogFailed(error) => {
                // A stale catalog is more useful than an empty one, so the
                // previous entries stay.
                self.notice = Some(error);
            }
```

- [ ] **Step 6: Seed the catalog and request the first load**

In `impl Default for Agency` (around line 933), replace what Task 7 left there:

```rust
        let slash_command_catalog =
            merge_catalog(discover_agent_commands(&configured_agents, &cwd));
```

with:

```rust
        let slash_command_catalog = agency_commands();
```

Then add a boot function to the `impl Agency` block that starts around line 984:

```rust
    /// iced's boot hook. The catalog starts with Agency's own commands and the
    /// agent half is requested immediately, so the composer is usable while
    /// the first index runs.
    fn boot() -> (Self, Task<AppEvent>) {
        (Self::default(), Task::done(AppEvent::SlashCatalogRequested))
    }
```

And in `run_desktop`, change:

```rust
    iced::application(Agency::default, Agency::update, Agency::view)
```

to:

```rust
    iced::application(Agency::boot, Agency::update, Agency::view)
```

- [ ] **Step 7: Wire the remaining triggers**

In the worktree-switch handler (around line 2294), replace what Task 7 left there:

```rust
        self.slash_command_catalog =
            merge_catalog(discover_agent_commands(&self.configured_agents, &self.cwd));
```

with:

```rust
        self.slash_command_catalog = agency_commands();
        self.emit(AppEvent::SlashCatalogRequested);
```

In the `AppEvent::PluginInstall(event)` arm, inside the existing `if let PluginInstallEvent::Finished { .. }` block, add after the existing body:

```rust
                    // An install changes what is on disk, so the catalog it
                    // was built from is now stale.
                    self.emit(AppEvent::SlashCatalogRequested);
```

In the `SlashCommand::McpAdd` handling path, after the server is added, add the same `self.emit(AppEvent::SlashCatalogRequested);` line.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p agency-desktop`
Expected: PASS.

- [ ] **Step 9: Verify the whole workspace**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets`
Expected: no warnings.

- [ ] **Step 10: Verify against the real machine**

Run: `cargo run -p agency-desktop --bin agency`

Type `/` in the composer and confirm:
- `/init`, `/mcp add`, and `/plugin install` appear with an AGENCY badge
- Plugin entries such as `/superpowers:brainstorming` and `/hookify:hookify` appear
- Typing `/brain` narrows to the brainstorming entry
- Descriptions read as prose, not `name: brainstorming`
- No entry appears for a plugin disabled in `~/.claude/settings.json` — on this machine, `commit-commands`, `feature-dev`, `security-guidance`, `learning-output-style`, and `claude-opus-4-5-migration`

- [ ] **Step 11: Commit**

```bash
git add crates/agency-desktop/
git commit -m "feat(desktop): refresh the command catalog through typed events"
```

---

## Self-Review

**Spec coverage:**

| Spec requirement | Task |
|---|---|
| `CommandCatalog` trait and `AgentCommand` in the api crate | 2 |
| `discovery` helpers shared by both translators | 1, 2 |
| Frontmatter description fixing the `name:` bug | 1 |
| `argument-hint` captured | 1, 4, 5 |
| Claude personal/project/plugin/built-in sources | 4 |
| `installPath` from `installed_plugins.json` | 3 |
| `enabledPlugins` settings chain, `defaultEnabled` fallback | 3 |
| Manifest `commands` replaces, `skills` adds | 3, 4 |
| Plugin skill frontmatter `name` replaces last segment | 4 |
| Plugin root `SKILL.md` single-skill plugin | 4 |
| Personal overrides project; skill beats command | 4 |
| Codex `.agents/skills`, `.codex/skills` fallback, prompts, plugin skills | 5 |
| Codex `$name` invocation, no plugin namespacing | 5 |
| Per-source failure containment | 1, 3, 4, 5 |
| Segment matching | 6 |
| Tab unchanged | 6 |
| Merge with Agency commands; cross-provider duplicates kept | 7 |
| `SlashCatalogRequested`/`Loaded`/`Failed` | 8 |
| Refresh triggers: startup, worktree switch, plugin install, mcp add | 8 |
| Failed load keeps the previous catalog | 8 |

**Deliberate omissions**, all named as out of scope in the spec: managed enterprise settings, marketplace-level `defaultEnabled`, `/etc/codex/skills`, a filesystem watcher, fuzzy matching, and surfacing disabled plugins.

**Known limitation not in the spec:** when `installed_plugins.json` lists several scope entries for one plugin, Task 3 takes the first. On the current machine every plugin has exactly one entry.
