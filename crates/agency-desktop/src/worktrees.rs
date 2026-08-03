use std::fs;
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
    ensure_agency_ignored(&primary.path)?;

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

/// The ignore rules Agency needs in every repository it operates on: one line
/// per thing Agency writes into a repository. `config.local.toml` arrives from
/// `/init` and must never be committed, so without a rule it sits untracked —
/// and one untracked file is all it takes for `git worktree remove` to refuse.
/// `legacy-sessions/` is where the startup migration parks history it cannot
/// attribute to a live worktree.
const AGENCY_EXCLUDE_RULES: [&str; 4] = [
    ".agency/sessions/",
    ".agency/worktrees/",
    ".agency/legacy-sessions/",
    ".agency/config.local.toml",
];

/// Teaches the repository to ignore what Agency stores in it.
///
/// Git refuses to remove a worktree that reports untracked files, and a
/// worktree's session history lives inside it — so without these rules, every
/// worktree Agency creates becomes unremovable the moment it records a
/// session, and there is deliberately no force option to escape with.
///
/// The rules go in `info/exclude` rather than `.gitignore`: it lives in the
/// common directory, so one write covers the primary and every linked
/// worktree, and it is not a tracked file, so Agency never edits something the
/// user committed.
pub fn ensure_agency_ignored(workspace: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("Could not locate the Git directory: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not locate the Git directory: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let common = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    let exclude = common.join("info").join("exclude");
    let existing = fs::read_to_string(&exclude).unwrap_or_default();
    let missing = AGENCY_EXCLUDE_RULES
        .iter()
        .filter(|rule| !existing.lines().any(|line| line.trim() == **rule))
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    for rule in missing {
        updated.push_str(rule);
        updated.push('\n');
    }
    if let Some(parent) = exclude.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }
    fs::write(&exclude, updated)
        .map_err(|error| format!("Could not write {}: {error}", exclude.display()))
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
        // Never inherit the developer's own ~/.config/git/ignore: a global
        // `.agency/` rule would supply the guarantee the product is supposed to
        // supply, and these tests would pass against a no-op writer.
        git(&root, &["config", "core.excludesFile", "/dev/null"]);
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

    /// A repository with no ignore rules at all — the state a user's project
    /// is in before Agency touches it. The rule-bearing `repository` fixture
    /// cannot catch a missing product-side guarantee, because it supplies the
    /// guarantee itself.
    pub fn repository_without_ignore_rules(name: &str) -> PathBuf {
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
        // See `repository`: without this the fixture inherits whatever the
        // developer running the suite happens to ignore globally, which is the
        // one thing a fixture proving a missing ignore rule must not do.
        git(&root, &["config", "core.excludesFile", "/dev/null"]);
        std::fs::write(root.join("README.md"), "test\n").unwrap();
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

    /// An agent inside a worktree may remove it: calling `remove` with
    /// `workspace` equal to the worktree being removed must still resolve and
    /// succeed via `discover`. This does not exercise the `current_dir`
    /// choice in the implementation — on Linux, `git worktree remove` can run
    /// from inside the directory it is deleting, since POSIX permits
    /// unlinking a process's own cwd. `current_dir(&primary.path)` is kept
    /// for platforms that do not allow that.
    #[test]
    fn removes_a_worktree_from_inside_that_worktree() {
        let root = repository("remove-self");
        let worktree = create(&root, "feature", None).unwrap();

        let removed = remove(&worktree.path, "feature").unwrap();

        assert_eq!(removed.path, worktree.path);
        assert!(!worktree.path.exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    use super::tests_support::repository_without_ignore_rules;

    /// The bug this task fixes. A user's repository does not ignore
    /// `.agency/`, so the session history Agency writes inside a worktree
    /// reads as untracked, and `git worktree remove` refuses it — forever,
    /// since there is no force option. Agency has to supply the rule itself.
    #[test]
    fn a_worktree_stays_removable_in_a_repository_with_no_ignore_rules() {
        let root = repository_without_ignore_rules("no-ignore-rules");
        let worktree = create(&root, "feature", None).unwrap();
        let sessions = worktree.path.join(".agency").join("sessions").join("one");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("session.json"),
            r#"{"conversation_id":"one"}"#,
        )
        .unwrap();

        let removed = remove(&root, "feature").unwrap();

        assert_eq!(removed.path, worktree.path);
        assert!(!worktree.path.exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignore_rules_are_written_once_and_survive_existing_content() {
        let root = repository_without_ignore_rules("exclude-idempotent");
        let exclude = root.join(".git").join("info").join("exclude");
        std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        std::fs::write(&exclude, "# a user's own rule\nbuild/\n").unwrap();

        ensure_agency_ignored(&root).unwrap();
        let after_first = std::fs::read_to_string(&exclude).unwrap();
        ensure_agency_ignored(&root).unwrap();
        let after_second = std::fs::read_to_string(&exclude).unwrap();

        assert_eq!(after_first, after_second, "the writer must be idempotent");
        assert!(
            after_first.contains("build/"),
            "existing rules must survive"
        );
        assert_eq!(
            after_first
                .lines()
                .filter(|line| line.trim() == ".agency/sessions/")
                .count(),
            1
        );
        assert!(
            after_first
                .lines()
                .any(|line| line.trim() == ".agency/worktrees/")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// One write has to cover every worktree, which is why the rule goes in
    /// the common directory rather than in each checkout — and it has to cover
    /// everything Agency writes, since one untracked file is enough for
    /// `git worktree remove` to refuse forever. The primary is asserted too:
    /// without the `.agency/worktrees/` rule it reports `?? .agency/`, and
    /// `git add -A` stages the checkout as an embedded repository.
    #[test]
    fn ignore_rules_apply_inside_a_linked_worktree() {
        let root = repository_without_ignore_rules("exclude-linked");
        let worktree = create(&root, "feature", None).unwrap();
        let sessions = worktree.path.join(".agency").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("session.json"), "{}").unwrap();
        // What `/init` writes into the worktree it runs in. `config.toml` is
        // tracked; this one must never be committed, so it can only be ignored.
        std::fs::write(
            worktree.path.join(".agency").join("config.local.toml"),
            "[workspace]\n",
        )
        .unwrap();

        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&worktree.path)
            .output()
            .unwrap();

        assert!(
            String::from_utf8_lossy(&output.stdout).trim().is_empty(),
            "nothing Agency writes may read as untracked inside the worktree: {}",
            String::from_utf8_lossy(&output.stdout)
        );

        let primary = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&primary.stdout).trim().is_empty(),
            "the checkout must not read as untracked in the primary"
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
