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
