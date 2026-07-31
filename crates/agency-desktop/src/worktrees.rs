use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub label: String,
    pub branch: Option<String>,
}

pub fn discover(workspace: &Path) -> Result<Vec<Worktree>, String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("Could not discover Git worktrees: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not discover Git worktrees: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let worktrees = parse_porcelain(&String::from_utf8_lossy(&output.stdout));
    if worktrees.is_empty() {
        Err("Git did not report any worktrees".to_owned())
    } else {
        Ok(worktrees)
    }
}

pub fn create(
    workspace: &Path,
    branch: &str,
    base: Option<&str>,
    path_hint: Option<&str>,
) -> Result<Worktree, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Branch name cannot be empty".to_owned());
    }
    let validation = Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("Could not validate branch name: {error}"))?;
    if !validation.status.success() {
        return Err(format!("Invalid branch name: {branch}"));
    }

    let existing = discover(workspace)?;
    let primary = existing
        .first()
        .ok_or_else(|| "Git did not report a primary worktree".to_owned())?;
    let repository_name = primary
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    let hint = path_hint
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .unwrap_or_else(|| branch.rsplit('/').next().unwrap_or(branch));
    if !hint
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
        || hint == "."
        || hint == ".."
    {
        return Err(
            "Worktree path hint may contain only letters, numbers, '-', '_', and '.'".to_owned(),
        );
    }
    let parent = primary
        .path
        .parent()
        .ok_or_else(|| "Primary worktree has no parent directory".to_owned())?;
    let path = parent.join(format!("{repository_name}-{hint}"));
    if path.exists() {
        return Err(format!("Worktree path already exists: {}", path.display()));
    }

    let base = base
        .map(str::trim)
        .filter(|base| !base.is_empty())
        .unwrap_or("HEAD");
    let output = Command::new("git")
        .args(["worktree", "add", "-b", branch])
        .arg("--")
        .arg(&path)
        .arg(base)
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("Could not create Git worktree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not create Git worktree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(Worktree {
        path,
        label: branch.to_owned(),
        branch: Some(branch.to_owned()),
    })
}

fn parse_porcelain(output: &str) -> Vec<Worktree> {
    output
        .split("\n\n")
        .filter_map(|record| {
            let mut path = None;
            let mut branch = None;
            let mut detached = false;
            for line in record.lines() {
                if let Some(value) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(value));
                } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(value.to_owned());
                } else if line == "detached" {
                    detached = true;
                }
            }
            let path = path?;
            let label = branch.clone().unwrap_or_else(|| {
                if detached {
                    path.file_name().map_or_else(
                        || "detached".to_owned(),
                        |name| name.to_string_lossy().into(),
                    )
                } else {
                    path.file_name().map_or_else(
                        || path.display().to_string(),
                        |name| name.to_string_lossy().into(),
                    )
                }
            });
            Some(Worktree {
                path,
                label,
                branch,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branches_and_detached_worktrees() {
        let worktrees = parse_porcelain(
            "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\n\
             worktree /repo-task\nHEAD def\ndetached\n\n",
        );

        assert_eq!(
            worktrees,
            vec![
                Worktree {
                    path: PathBuf::from("/repo"),
                    label: "main".to_owned(),
                    branch: Some("main".to_owned()),
                },
                Worktree {
                    path: PathBuf::from("/repo-task"),
                    label: "repo-task".to_owned(),
                    branch: None,
                },
            ]
        );
    }

    #[test]
    fn preserves_branch_names_with_slashes() {
        let worktrees =
            parse_porcelain("worktree /repo-feature\nHEAD abc\nbranch refs/heads/feature/tabs\n");

        assert_eq!(worktrees[0].label, "feature/tabs");
    }
}
