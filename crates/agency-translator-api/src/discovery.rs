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
        .map(|rest| {
            rest.split_once("\n---")
                .map(|(_, body)| body)
                .unwrap_or(rest)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\nname: brainstorming\ndescription: Turn ideas into designs\nargument-hint: [topic]\n---\n\n# Brainstorming\n\nBody text.\n";

    #[test]
    fn frontmatter_keys_are_read_by_name() {
        let parsed = frontmatter(SKILL);
        assert_eq!(parsed.name.as_deref(), Some("brainstorming"));
        assert_eq!(
            parsed.description.as_deref(),
            Some("Turn ideas into designs")
        );
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
        assert_eq!(
            describe("---\ndescription: never closed\n"),
            "description: never closed"
        );
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
        assert_eq!(
            describe("---\ndescription:\nmodel: opus\n---\nBody\n"),
            "Body"
        );
    }

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
        fs::write(
            root.join("deploy/SKILL.md"),
            "---\ndescription: Ship\n---\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("audit")).unwrap();
        fs::write(root.join("audit/SKILL.md"), "body").unwrap();
        // A directory with no SKILL.md is not a skill.
        fs::create_dir_all(root.join("assets")).unwrap();

        let found = skill_directories(&root);

        assert_eq!(
            found
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
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
            found
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            ["commit", "deploy"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_root_yields_nothing_rather_than_failing() {
        assert!(skill_directories(Path::new("/does/not/exist")).is_empty());
        assert!(command_files(Path::new("/does/not/exist")).is_empty());
    }
}
