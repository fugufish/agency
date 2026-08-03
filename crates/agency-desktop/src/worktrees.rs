use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config;

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

/// Creates `branch` and checks it out under the **primary** worktree's
/// `.agency/worktrees/`, keyed by the encoded branch name.
///
/// The parent is resolved from the primary rather than from `workspace` on
/// purpose. An agent working inside worktree A that creates worktree B would
/// otherwise nest B inside A, and removing A would take B — and every session
/// B recorded — with it.
pub fn create(workspace: &Path, branch: &str, base: Option<&str>) -> Result<Worktree, String> {
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
    let path = config::worktrees_directory(&primary.path).join(config::path_component(branch));
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

/// Removes the worktree checked out on `branch`, and with it everything the
/// worktree directory holds — session history included, since git deletes the
/// directory wholesale and Agency's session data is ignored rather than
/// untracked.
///
/// The branch is left in place: removing a checkout is not abandoning the work.
/// There is no force option. Git refuses a worktree with modified or untracked
/// files, and that refusal is deliberate here — the checkout is recoverable
/// from git, but the history removal destroys is not.
pub fn remove(workspace: &Path, branch: &str) -> Result<Worktree, String> {
    let branch = branch.trim();
    let existing = discover(workspace)?;
    let primary = existing
        .first()
        .ok_or_else(|| "Git did not report a primary worktree".to_owned())?
        .clone();
    let target = existing
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch))
        .ok_or_else(|| format!("No worktree is checked out on {branch}"))?;
    if target.path == primary.path {
        return Err(format!(
            "{branch} is the primary worktree and cannot be removed"
        ));
    }

    // Run from the primary, never from `workspace`: an agent inside the
    // worktree it is removing would otherwise have git working from a
    // directory that is being deleted underneath it.
    let output = Command::new("git")
        .args(["worktree", "remove"])
        .arg(&target.path)
        .current_dir(&primary.path)
        .output()
        .map_err(|error| format!("Could not remove Git worktree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not remove Git worktree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(target)
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

/// Real repositories for the tests that exercise git rather than the parser.
/// Lives outside `mod tests` because the reducer tests in `main.rs` need the
/// same fixture.
#[cfg(test)]
pub mod tests_support {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn git(root: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A repository whose `.gitignore` matches the one this project ships, so
    /// the tests exercise the same ignore rules production does. Without the
    /// `.agency/` entries a worktree holding a session reports untracked files
    /// and `git worktree remove` refuses it.
    pub fn repository(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "agency-worktree-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "--initial-branch", "main"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        git(&root, &["config", "user.name", "Agency Test"]);
        std::fs::write(root.join("README.md"), "test\n").unwrap();
        std::fs::write(
            root.join(".gitignore"),
            ".agency/sessions/\n.agency/worktrees/**\n",
        )
        .unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "init"]);
        root
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::repository;
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

    #[test]
    fn creates_the_worktree_under_the_primary_dot_agency_directory() {
        let root = repository("create-placement");

        let worktree = create(&root, "feature", None).unwrap();

        assert_eq!(
            worktree.path,
            root.join(".agency").join("worktrees").join("feature")
        );
        assert_eq!(worktree.branch.as_deref(), Some("feature"));
        assert!(worktree.path.join("README.md").is_file());

        std::fs::remove_dir_all(root).unwrap();
    }

    /// A slashed branch is one directory, not two. The encoding is what lets a
    /// checkout and its session history be found by the same key.
    #[test]
    fn encodes_a_slashed_branch_into_one_path_component() {
        let root = repository("create-encoding");

        let worktree = create(&root, "feature/tabs", None).unwrap();

        assert_eq!(
            worktree.path,
            root.join(".agency")
                .join("worktrees")
                .join("feature%2Ftabs")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// The guard the whole layout rests on. Creating from inside a worktree
    /// must resolve against the primary — nesting B inside A would mean
    /// removing A silently destroys B and everything B ever recorded.
    #[test]
    fn creates_under_the_primary_even_when_called_from_another_worktree() {
        let root = repository("create-recursion");
        let first = create(&root, "first", None).unwrap();

        let second = create(&first.path, "second", None).unwrap();

        assert_eq!(
            second.path,
            root.join(".agency").join("worktrees").join("second")
        );
        assert!(!second.path.starts_with(&first.path));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_a_branch_that_already_has_a_worktree() {
        let root = repository("create-duplicate");
        create(&root, "feature", None).unwrap();

        let error = create(&root, "feature", None).unwrap_err();

        assert!(
            error.contains("already exists"),
            "unexpected error: {error}"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// The behaviour the whole layout rests on: ignored files neither block the
    /// removal nor survive it, so a worktree's session history is collected
    /// with its checkout and no sweep is needed. Pinned here so a git upgrade
    /// that changes it fails in this test rather than in production.
    #[test]
    fn removing_a_worktree_takes_its_session_history_with_it() {
        let root = repository("remove-sessions");
        let worktree = create(&root, "feature", None).unwrap();
        let sessions = worktree.path.join(".agency").join("sessions").join("one");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("session.json"),
            r#"{"conversation_id":"one"}"#,
        )
        .unwrap();
        std::fs::write(sessions.join("image.bin"), vec![0u8; 200_000]).unwrap();

        let removed = remove(&root, "feature").unwrap();

        assert_eq!(removed.path, worktree.path);
        assert!(!worktree.path.exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Removing a checkout is not abandoning the work.
    #[test]
    fn removing_a_worktree_leaves_the_branch_in_place() {
        let root = repository("remove-branch");
        create(&root, "feature", None).unwrap();

        remove(&root, "feature").unwrap();

        let output = Command::new("git")
            .args(["branch", "--list", "feature"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("feature"),
            "the branch should survive removal"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// There is no force flag. Session history is not recoverable from git the
    /// way a checkout is, so refusing here is what keeps removal from being a
    /// surprise.
    #[test]
    fn refuses_to_remove_a_dirty_worktree() {
        let root = repository("remove-dirty");
        let worktree = create(&root, "feature", None).unwrap();
        std::fs::write(worktree.path.join("README.md"), "edited\n").unwrap();

        let error = remove(&root, "feature").unwrap_err();

        assert!(
            error.contains("Could not remove Git worktree"),
            "unexpected error: {error}"
        );
        assert!(
            worktree.path.exists(),
            "the worktree must survive a refusal"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_remove_the_primary_worktree() {
        let root = repository("remove-primary");

        let error = remove(&root, "main").unwrap_err();

        assert!(
            error.contains("primary worktree"),
            "unexpected error: {error}"
        );
        assert!(root.join("README.md").is_file());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_remove_a_branch_with_no_worktree() {
        let root = repository("remove-unknown");

        let error = remove(&root, "nonexistent").unwrap_err();

        assert!(
            error.contains("nonexistent"),
            "the error should name the branch: {error}"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// An agent inside a worktree may remove it. Running git from the primary
    /// rather than from the caller's workspace means the command is not
    /// executing inside the directory it is deleting.
    #[test]
    fn removes_a_worktree_from_inside_that_worktree() {
        let root = repository("remove-self");
        let worktree = create(&root, "feature", None).unwrap();

        let removed = remove(&worktree.path, "feature").unwrap();

        assert_eq!(removed.path, worktree.path);
        assert!(!worktree.path.exists());

        std::fs::remove_dir_all(root).unwrap();
    }
}
