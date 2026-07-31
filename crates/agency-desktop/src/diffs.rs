use std::fs;
use std::path::{Path, PathBuf};

use agency_translator_api::tools;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DIFF_ARTIFACTS_FILE: &str = "diffs.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffArtifact {
    pub event_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub diff: String,
    pub transcript_index: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffSessionState {
    #[serde(default)]
    pub artifacts: Vec<DiffArtifact>,
    #[serde(default)]
    pub selected: usize,
    #[serde(default)]
    pub activity_visible: bool,
    #[serde(default)]
    pub viewer_visible: bool,
    #[serde(default)]
    pub viewer_scroll: u32,
}

impl DiffSessionState {
    pub fn load(session_directory: &Path) -> Result<Self, String> {
        let path = path(session_directory);
        if !path.exists() {
            return Ok(Self::default());
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        let mut state: Self = serde_json::from_str(&source)
            .map_err(|error| format!("Could not parse {}: {error}", path.display()))?;
        state.clamp_selection();
        Ok(state)
    }

    pub fn save(&self, session_directory: &Path) -> Result<(), String> {
        fs::create_dir_all(session_directory).map_err(|error| {
            format!("Could not create {}: {error}", session_directory.display())
        })?;
        let path = path(session_directory);
        let source = serde_json::to_string_pretty(self)
            .map_err(|error| format!("Could not encode diff activity: {error}"))?;
        fs::write(&path, format!("{source}\n"))
            .map_err(|error| format!("Could not write {}: {error}", path.display()))
    }

    pub fn capture(&mut self, event_id: &str, input: &Value, transcript_index: usize) -> bool {
        // A change is only worth keeping once it has actually been applied.
        if tools::kind(input) == Some(tools::FILE_CHANGE)
            && tools::status(input) != tools::COMPLETED
        {
            return false;
        }
        let mut changes = file_changes(input);
        if changes.is_empty()
            && let Some(diff) = extract_diff(input)
        {
            changes.push(FileChange {
                path: diff_title(&diff).unwrap_or_else(|| "File changes".to_owned()),
                description: describe_diff("update", &diff),
                diff,
            });
        }
        let before = self.artifacts.len();
        for change in changes {
            if self
                .artifacts
                .iter()
                .any(|artifact| artifact.event_id == event_id && artifact.diff == change.diff)
            {
                continue;
            }
            self.artifacts.push(DiffArtifact {
                event_id: event_id.to_owned(),
                title: change.path,
                description: change.description,
                diff: change.diff,
                transcript_index,
            });
        }
        if self.artifacts.len() == before {
            return false;
        }
        self.selected = self.artifacts.len() - 1;
        true
    }

    pub fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.artifacts.len().saturating_sub(1));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub description: String,
    pub diff: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Metadata,
    Hunk,
    Context,
    Addition,
    Deletion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_number: Option<usize>,
    pub new_number: Option<usize>,
    pub content: String,
}

pub fn renderable_diff_lines(diff: &str) -> Vec<DiffLine> {
    let mut old_number = None;
    let mut new_number = None;

    diff.lines()
        .map(|line| {
            if line.starts_with("@@") {
                if let Some((old_start, new_start)) = hunk_starts(line) {
                    old_number = Some(old_start);
                    new_number = Some(new_start);
                }
                return DiffLine {
                    kind: DiffLineKind::Hunk,
                    old_number: None,
                    new_number: None,
                    content: line.to_owned(),
                };
            }

            let metadata = line.starts_with("diff --git ")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with("*** ");
            if metadata {
                return DiffLine {
                    kind: DiffLineKind::Metadata,
                    old_number: None,
                    new_number: None,
                    content: line.to_owned(),
                };
            }

            let (kind, shown_old, shown_new) = if line.starts_with('+') {
                let shown = new_number;
                new_number = new_number.map(|number| number + 1);
                (DiffLineKind::Addition, None, shown)
            } else if line.starts_with('-') {
                let shown = old_number;
                old_number = old_number.map(|number| number + 1);
                (DiffLineKind::Deletion, shown, None)
            } else {
                let shown_old = old_number;
                let shown_new = new_number;
                old_number = old_number.map(|number| number + 1);
                new_number = new_number.map(|number| number + 1);
                (DiffLineKind::Context, shown_old, shown_new)
            };

            DiffLine {
                kind,
                old_number: shown_old,
                new_number: shown_new,
                content: line
                    .strip_prefix(['+', '-', ' '])
                    .unwrap_or(line)
                    .to_owned(),
            }
        })
        .collect()
}

fn hunk_starts(line: &str) -> Option<(usize, usize)> {
    let range = line.strip_prefix("@@")?.split_once("@@")?.0.trim();
    let mut ranges = range.split_ascii_whitespace();
    let old = ranges
        .next()?
        .strip_prefix('-')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new = ranges
        .next()?
        .strip_prefix('+')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old, new))
}

pub fn file_changes(value: &Value) -> Vec<FileChange> {
    value
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            let path = change.get("path")?.as_str()?.to_owned();
            let kind = tools::change_kind(change);
            let hunk = change.get("diff")?.as_str()?;
            let old = if kind.eq_ignore_ascii_case("add") {
                "/dev/null".to_owned()
            } else {
                format!("a/{path}")
            };
            let new = if kind.eq_ignore_ascii_case("delete") {
                "/dev/null".to_owned()
            } else {
                format!("b/{path}")
            };
            let diff = format!("--- {old}\n+++ {new}\n{hunk}");
            Some(FileChange {
                path,
                description: describe_diff(kind, hunk),
                diff,
            })
        })
        .collect()
}

fn describe_diff(kind: &str, diff: &str) -> String {
    let additions = diff
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    let deletions = diff
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();
    let action = match kind.to_ascii_lowercase().as_str() {
        "add" | "create" => "Created",
        "delete" | "remove" => "Deleted",
        "move" | "rename" => "Moved",
        _ => "Updated",
    };
    let context = diff.lines().find_map(|line| {
        let rest = line.strip_prefix("@@")?;
        let (_, context) = rest.split_once("@@")?;
        let context = context.trim();
        (!context.is_empty()).then_some(context)
    });
    match context {
        Some(context) => format!("{action} {context} · +{additions} −{deletions}"),
        None => format!("{action} · +{additions} −{deletions}"),
    }
}

fn path(session_directory: &Path) -> PathBuf {
    session_directory.join(DIFF_ARTIFACTS_FILE)
}

fn extract_diff(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let looks_like_diff = value.contains("*** Begin Patch")
                || value.contains("\ndiff --git ")
                || value.starts_with("diff --git ")
                || (value.contains("\n--- ") && value.contains("\n+++ "));
            looks_like_diff.then(|| value.to_owned())
        }
        Value::Array(values) => values.iter().find_map(extract_diff),
        Value::Object(values) => {
            for key in ["diff", "patch", "changes", "input", "arguments"] {
                if let Some(diff) = values.get(key).and_then(extract_diff) {
                    return Some(diff);
                }
            }
            values.values().find_map(extract_diff)
        }
        _ => None,
    }
}

fn diff_title(diff: &str) -> Option<String> {
    diff.lines().find_map(|line| {
        line.strip_prefix("*** Update File: ")
            .or_else(|| line.strip_prefix("*** Add File: "))
            .or_else(|| line.strip_prefix("*** Delete File: "))
            .or_else(|| line.strip_prefix("+++ b/"))
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn extracts_apply_patch_artifacts() {
        let mut state = DiffSessionState::default();
        assert!(state.capture(
            "call-1",
            &json!({"patch":"*** Begin Patch\n*** Update File: src/main.rs\n@@\n-old\n+new\n*** End Patch"}),
            4,
        ));
        assert_eq!(state.artifacts[0].title, "src/main.rs");
        assert_eq!(state.artifacts[0].transcript_index, 4);
    }

    #[test]
    fn parses_unified_diff_lines_with_line_numbers() {
        let lines = renderable_diff_lines(
            "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -10,2 +10,2 @@ fn main()\n old\n-removed\n+added",
        );

        assert_eq!(lines[2].kind, DiffLineKind::Hunk);
        assert_eq!(
            (lines[3].old_number, lines[3].new_number),
            (Some(10), Some(10))
        );
        assert_eq!(
            (lines[4].kind, lines[4].old_number, lines[4].new_number),
            (DiffLineKind::Deletion, Some(11), None)
        );
        assert_eq!(
            (lines[5].kind, lines[5].old_number, lines[5].new_number),
            (DiffLineKind::Addition, None, Some(11))
        );
    }

    #[test]
    fn captures_codex_hunk_only_file_changes() {
        let mut state = DiffSessionState::default();
        assert!(state.capture(
            "change-1",
            &json!({
                "type": "fileChange",
                "status": "completed",
                "changes": [{
                    "path": "src/lib.rs",
                    "kind": "update",
                    "diff": "@@ -1 +1 @@\n-old\n+new\n"
                }]
            }),
            3,
        ));
        assert_eq!(state.artifacts[0].title, "src/lib.rs");
        assert_eq!(state.artifacts[0].description, "Updated · +1 −1");
        assert!(state.artifacts[0].diff.starts_with("--- a/src/lib.rs"));
    }

    #[test]
    fn ignores_duplicate_tool_events() {
        let input = json!({"diff":"diff --git a/a b/a\n--- a/a\n+++ b/a\n-old\n+new"});
        let mut state = DiffSessionState::default();
        assert!(state.capture("one", &input, 1));
        assert!(!state.capture("one", &input, 1));
    }

    #[test]
    fn session_view_state_round_trips_with_artifacts() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("agency-diffs-{unique}"));
        let mut state = DiffSessionState {
            activity_visible: true,
            viewer_visible: true,
            viewer_scroll: 160,
            ..DiffSessionState::default()
        };
        state.capture(
            "call-2",
            &json!({"diff":"diff --git a/a b/a\n--- a/a\n+++ b/a\n-old\n+new"}),
            7,
        );
        state.save(&directory).unwrap();

        assert_eq!(DiffSessionState::load(&directory).unwrap(), state);
        fs::remove_dir_all(directory).unwrap();
    }
}
