# Worktree Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the worktree directory the unit of work — checkout and session history together under the primary's `.agency/worktrees/<branch>/` — and complete the create/list/remove loop through MCP, RPC, and typed events.

**Architecture:** `crates/agency-desktop/src/config.rs` becomes the single owner of the `.agency` layout. `worktrees.rs` stays the only module that shells out to git for worktree operations and grows a `remove`. Session storage stops reaching back to the primary repository and simply resolves to `<workspace>/.agency/sessions`. The RPC handlers become effects that publish typed events into the existing bus, and one reducer owns `worktrees` / `active_worktree` for all three paths (startup, create, remove).

**Tech Stack:** Rust 2024 edition, `rust-version` 1.95, `iced` 0.14 (the app's `Task`/`Element` types), `serde_json`, and the `git` CLI invoked through `std::process::Command`. No new dependencies.

## Global Constraints

- Rust edition 2024, `rust-version = "1.95"` (workspace `Cargo.toml`). Do not add dependencies; every task uses `std`, `serde_json`, or crates already listed in `crates/agency-desktop/Cargo.toml`.
- CI runs exactly `cargo build --workspace --locked` and `cargo test --workspace --all-targets --locked`. There is no clippy gate, but leave no new warnings.
- Run `cargo fmt --all` before every commit. The repository has a `style: cargo fmt --all` commit in its history; formatting drift is treated as a defect.
- Tests use `std::env::temp_dir()` with a `format!("agency-<name>-{}-{unique}", std::process::id())` name and clean up with `std::fs::remove_dir_all`. There is no `tempfile` dependency — do not add one.
- Per `CLAUDE.md`, all state transitions are typed `AppEvent`s published through the event bus. A view, input handler, or RPC handler must not mutate another feature's state directly.
- Per `CLAUDE.md`, resolution paths must not branch on which provider is calling. `SessionContext.provider` is a `String` and must be passed through as data.
- No hexadecimal colors or one-off palettes. This plan touches no views beyond the explorer's entry collection, which renders no color.
- Never delete a branch. `git worktree remove` leaves the branch in place; keep it that way.
- `remove` never takes a `force` flag. Refusing a dirty worktree is the guard that keeps session-history deletion from being a surprise.

**Reference spec:** `docs/superpowers/specs/2026-08-03-worktree-integration-design.md`

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/agency-desktop/src/config.rs` | Owns the `.agency` layout: config directory, worktrees directory, path-component encoding | Modify — gains `worktrees_directory`, receives `path_component` |
| `crates/agency-desktop/src/sessions.rs` | Session records and their on-disk registry | Modify — `worktree_sessions_directory` simplified, git plumbing deleted, migration added |
| `crates/agency-desktop/src/worktrees.rs` | The only git-worktree shell-out | Modify — `create` re-homed and loses `path_hint`, `remove` added |
| `crates/agency-desktop/src/main.rs` | App state, reducer, RPC effects, explorer walk | Modify — three `AppEvent` variants, one reducer, RPC handlers become effects, explorer skip |
| `crates/agency-mcp/src/lib.rs` | MCP tool schemas and RPC forwarding | Modify — `remove_worktree` added, `path_hint` dropped |
| `.gitignore` | Keeps worktree and session data out of git | Modify — `.agency/sessions/` added |
| `README.md` | Documents the agent tool surface | Modify — `remove_worktree` listed |

---

### Task 1: Sessions live beside the worktree that owns them

Session history currently resolves to the *primary* repository root through three git invocations. It becomes a plain join against the workspace, which is what lets a worktree's history die with the worktree.

**Files:**
- Modify: `crates/agency-desktop/src/config.rs` — add `path_component` and `worktrees_directory`
- Modify: `crates/agency-desktop/src/sessions.rs:320-365` — replace `worktree_sessions_directory`, delete `git_output` and `path_component`
- Modify: `.gitignore`
- Test: `crates/agency-desktop/src/config.rs` (tests module), `crates/agency-desktop/src/sessions.rs` (tests module)

**Interfaces:**
- Consumes: `workspace_config_directory(&Path) -> PathBuf` (already in `config.rs:7` region, returns `workspace.join(".agency")`)
- Produces:
  - `config::path_component(value: &str) -> String` — percent-encodes anything outside `[A-Za-z0-9._-]`, returns `"_"` for empty input
  - `config::worktrees_directory(primary: &Path) -> PathBuf` — `<primary>/.agency/worktrees`
  - `sessions::worktree_sessions_directory(workspace: &Path) -> PathBuf` — `<workspace>/.agency/sessions`, unchanged signature

- [ ] **Step 1: Write the failing test for the session directory**

Add to the `mod tests` block at the bottom of `crates/agency-desktop/src/sessions.rs`:

```rust
    /// Sessions belong to the worktree that produced them, so the path is a
    /// plain join and not a git query. Asserted against a directory that is not
    /// a repository at all — the previous implementation shelled out to
    /// `rev-parse` and could not answer here.
    #[test]
    fn sessions_live_beside_the_worktree_that_owns_them() {
        let workspace = Path::new("/work/project");

        assert_eq!(
            worktree_sessions_directory(workspace),
            Path::new("/work/project/.agency/sessions")
        );
        assert_eq!(
            worktree_sessions_directory(Path::new("/work/project/.agency/worktrees/feature")),
            Path::new("/work/project/.agency/worktrees/feature/.agency/sessions")
        );
    }
```

- [ ] **Step 2: Write the failing test for the shared path encoder**

Add to the `mod tests` block at the bottom of `crates/agency-desktop/src/config.rs`:

```rust
    #[test]
    fn path_components_encode_anything_a_directory_name_cannot_hold() {
        assert_eq!(path_component("feature"), "feature");
        assert_eq!(path_component("feature/tabs"), "feature%2Ftabs");
        assert_eq!(path_component("fix.v2_final-1"), "fix.v2_final-1");
        assert_eq!(path_component(""), "_");
    }

    #[test]
    fn worktrees_live_under_the_primary_dot_agency_directory() {
        assert_eq!(
            worktrees_directory(Path::new("/work/project")),
            Path::new("/work/project/.agency/worktrees")
        );
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --package agency-desktop sessions_live_beside path_components_encode worktrees_live_under`

Expected: FAIL. The `config.rs` tests fail to compile with `cannot find function 'path_component' in this scope` and `cannot find function 'worktrees_directory' in this scope`. Once those exist, `sessions_live_beside_the_worktree_that_owns_them` fails on the assertion, since the current implementation returns `/work/project/.agency/worktrees/root/sessions`.

- [ ] **Step 4: Move the path encoder into `config.rs`**

Add to `crates/agency-desktop/src/config.rs`, near `workspace_config_directory`:

```rust
/// Everything Agency stores per worktree lives under this directory.
pub fn worktrees_directory(primary: &Path) -> PathBuf {
    workspace_config_directory(primary).join("worktrees")
}

/// A branch name is not a directory name. Percent-encoding everything outside
/// the portable set keeps `feature/tabs` one component deep and round-trips
/// unambiguously, which matters because the encoded name is how a checkout and
/// its session history find each other.
pub fn path_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    if encoded.is_empty() {
        "_".to_owned()
    } else {
        encoded
    }
}
```

- [ ] **Step 5: Simplify the session directory and delete the git plumbing**

In `crates/agency-desktop/src/sessions.rs`, replace the whole of `worktree_sessions_directory` (currently lines 320-353) and `git_output` (currently lines 355-365) with:

```rust
/// Sessions live beside the worktree that produced them. `git worktree remove`
/// deletes a worktree directory wholesale, ignored files included, so a
/// worktree's history is collected with it and nothing has to sweep for
/// orphans later.
pub fn worktree_sessions_directory(workspace: &Path) -> PathBuf {
    workspace_config_directory(workspace).join("sessions")
}
```

Then delete the now-unused `path_component` function (currently lines 367-381) and update the import at the top of the file:

```rust
use crate::config::{path_component, workspace_config_directory};
```

Remove `use std::process::Command;` from the top of `sessions.rs` — nothing in the file shells out any more. Leave `use std::fs;`, `use std::path::{Path, PathBuf};`, and the `SystemTime`/`UNIX_EPOCH` import alone; they are still used.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --package agency-desktop`

Expected: PASS, with no `unused import` or `dead_code` warnings. If `path_component` reports as unused in `sessions.rs`, the call site at line 284 (`.join(path_component(conversation_id))`) was missed — it must now resolve to the `config::` import.

- [ ] **Step 7: Ignore the new session directory**

Edit `.gitignore`. It currently reads:

```
/target
*.swp
*.swo
.DS_Store
.agency/sessions.json
.agency/worktrees/**
.agency/config.local.toml
```

Add `.agency/sessions/` after the `sessions.json` line:

```
/target
*.swp
*.swo
.DS_Store
.agency/sessions.json
.agency/sessions/
.agency/worktrees/**
.agency/config.local.toml
```

This line is load-bearing, not housekeeping. A nested worktree checks out this same `.gitignore`, and `git worktree remove` refuses any worktree reporting untracked files. Without it, every worktree becomes unremovable as soon as it holds a session.

- [ ] **Step 8: Verify the whole workspace still builds and commit**

Run: `cargo fmt --all && cargo build --workspace --locked && cargo test --workspace --all-targets --locked`

Expected: PASS.

```bash
git add crates/agency-desktop/src/config.rs crates/agency-desktop/src/sessions.rs .gitignore
git commit -m "feat(desktop): store sessions beside the worktree that owns them"
```

---

### Task 2: Move existing primary history to the new location

Task 1 changed where the primary worktree's history is read from. Any history already on disk sits at the old path and would silently vanish from the UI. One rename at startup fixes that.

**Files:**
- Modify: `crates/agency-desktop/src/sessions.rs` — add `migrate_legacy_root_sessions`
- Modify: `crates/agency-desktop/src/main.rs:947-951` — call it in `build`
- Test: `crates/agency-desktop/src/sessions.rs` (tests module)

**Interfaces:**
- Consumes: `config::workspace_config_directory`, `sessions::worktree_sessions_directory` (Task 1)
- Produces: `sessions::migrate_legacy_root_sessions(workspace: &Path)` — infallible, no return value, no-op on every launch after the first

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/agency-desktop/src/sessions.rs`:

```rust
    /// Sessions used to be keyed by branch under the primary, with the literal
    /// `root` standing in for the primary itself. That directory is the only
    /// one this migration can claim: every other key belonged to a worktree
    /// whose history now lives inside the worktree.
    #[test]
    fn legacy_root_sessions_move_beside_the_primary_worktree() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!(
            "agency-session-root-migration-{}-{unique}",
            std::process::id()
        ));
        let legacy = workspace_config_directory(&workspace)
            .join("worktrees")
            .join("root")
            .join("sessions")
            .join("conversation-1");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join(SESSION_CONFIG_FILE),
            r#"{"conversation_id":"conversation-1","codex_id":"codex-1"}"#,
        )
        .unwrap();

        migrate_legacy_root_sessions(&workspace);

        let registry = SessionRegistry::load(&workspace).unwrap();
        assert_eq!(registry.records().len(), 1);
        assert_eq!(
            registry.records()[0].binding(Provider::Codex),
            Some("codex-1")
        );
        assert!(
            !workspace_config_directory(&workspace)
                .join("worktrees")
                .join("root")
                .exists()
        );

        std::fs::remove_dir_all(workspace).unwrap();
    }

    /// A second launch must not clobber history written since the first.
    #[test]
    fn the_root_session_migration_does_not_overwrite_current_history() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!(
            "agency-session-root-noop-{}-{unique}",
            std::process::id()
        ));
        let legacy = workspace_config_directory(&workspace)
            .join("worktrees")
            .join("root")
            .join("sessions")
            .join("conversation-old");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join(SESSION_CONFIG_FILE),
            r#"{"conversation_id":"conversation-old","codex_id":"codex-old"}"#,
        )
        .unwrap();
        let current = worktree_sessions_directory(&workspace).join("conversation-new");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(
            current.join(SESSION_CONFIG_FILE),
            r#"{"conversation_id":"conversation-new","codex_id":"codex-new"}"#,
        )
        .unwrap();

        migrate_legacy_root_sessions(&workspace);

        let registry = SessionRegistry::load(&workspace).unwrap();
        assert_eq!(registry.records().len(), 1);
        assert_eq!(
            registry.records()[0].binding(Provider::Codex),
            Some("codex-new")
        );

        std::fs::remove_dir_all(workspace).unwrap();
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --package agency-desktop legacy_root_sessions the_root_session_migration`

Expected: FAIL to compile — `cannot find function 'migrate_legacy_root_sessions' in this scope`.

- [ ] **Step 3: Write the migration**

Add to `crates/agency-desktop/src/sessions.rs`, directly below `worktree_sessions_directory`:

```rust
/// Sessions used to live under the primary worktree keyed by branch, with
/// `root` for the primary itself. Moves that one directory into place beside
/// the primary. Silent on failure: a launch that cannot move history is still
/// a launch that should start.
pub fn migrate_legacy_root_sessions(workspace: &Path) {
    let config = workspace_config_directory(workspace);
    let legacy_root = config.join("worktrees").join("root");
    let legacy = legacy_root.join("sessions");
    let current = worktree_sessions_directory(workspace);
    if !legacy.is_dir() || current.exists() {
        return;
    }
    if let Some(parent) = current.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    if fs::rename(&legacy, &current).is_ok() {
        let _ = fs::remove_dir(&legacy_root);
    }
}
```

- [ ] **Step 4: Call it at startup**

In `crates/agency-desktop/src/main.rs`, inside `build`, the code currently reads:

```rust
        let cwd = worktrees[active_worktree].path.clone();
        let (sessions, session_notice) = match SessionRegistry::load(&cwd) {
```

Insert the migration between those two statements — after `cwd` resolves to the active worktree, before anything reads sessions:

```rust
        let cwd = worktrees[active_worktree].path.clone();
        sessions::migrate_legacy_root_sessions(&cwd);
        let (sessions, session_notice) = match SessionRegistry::load(&cwd) {
```

Check the imports at the top of `main.rs`. If `sessions` is imported as `use sessions::{SessionRecord, SessionRegistry};` rather than as a module path, add `mod sessions;` usage accordingly — the module is already declared, so `sessions::migrate_legacy_root_sessions` resolves without a new import.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --package agency-desktop`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/agency-desktop/src/sessions.rs crates/agency-desktop/src/main.rs
git commit -m "feat(desktop): move legacy root session history beside the primary worktree"
```

---

### Task 3: Create worktrees under the primary's `.agency/worktrees/`

`create` writes to a sibling directory keyed by a second identifier (`path_hint`) with its own validation rules. It moves inside `.agency/worktrees/`, keyed by the branch, resolved against the primary so a worktree can never nest inside another worktree.

**Files:**
- Modify: `crates/agency-desktop/src/worktrees.rs:32-106` — `create`
- Modify: `crates/agency-desktop/src/main.rs:2635-2668` — the `worktree.create` RPC arm loses its `path_hint` argument
- Modify: `crates/agency-mcp/src/lib.rs:84-106` — the `create_worktree` schema loses `path_hint`
- Test: `crates/agency-desktop/src/worktrees.rs` (tests module)

**Interfaces:**
- Consumes: `config::worktrees_directory`, `config::path_component` (Task 1); `worktrees::discover` (unchanged)
- Produces: `worktrees::create(workspace: &Path, branch: &str, base: Option<&str>) -> Result<Worktree, String>` — three parameters, down from four

- [ ] **Step 1: Add the shared test-repository helper**

The existing tests in `worktrees.rs` are pure parser tests; these are the first that need a real repository, because the behavior under test *is* git's. Task 4 needs the same helper from `main.rs`, so it goes in a module both can reach rather than inside `mod tests`.

Add to `crates/agency-desktop/src/worktrees.rs`, above the existing `#[cfg(test)] mod tests`:

```rust
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
```

Then add to the existing `mod tests` block in the same file, so the tests below can call them unqualified:

```rust
    use super::tests_support::repository;
```

- [ ] **Step 2: Write the failing tests for placement**

Add to the same `mod tests` block:

```rust
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
            root.join(".agency").join("worktrees").join("feature%2Ftabs")
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --package agency-desktop --lib worktrees` — or, since `main.rs` is a binary target, `cargo test --package agency-desktop --bin agency creates_the_worktree encodes_a_slashed creates_under_the_primary refuses_a_branch`

Expected: FAIL to compile — `create` takes four arguments, these calls pass three.

- [ ] **Step 4: Rewrite `create`**

Replace the whole of `create` in `crates/agency-desktop/src/worktrees.rs` (currently lines 32-106) with:

```rust
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
```

Add the import at the top of `worktrees.rs`:

```rust
use crate::config;
```

`git worktree add` creates leading directories, so `.agency/worktrees/` does not need to exist beforehand.

- [ ] **Step 5: Drop `path_hint` from the RPC call site**

In `crates/agency-desktop/src/main.rs`, the `worktree.create` arm currently reads:

```rust
                    branch.and_then(|branch| {
                        let base = call.params.get("base").and_then(serde_json::Value::as_str);
                        let path_hint = call
                            .params
                            .get("path_hint")
                            .and_then(serde_json::Value::as_str);
                        worktrees::create(&call.context.workspace, branch, base, path_hint).map(
```

Replace those lines with:

```rust
                    branch.and_then(|branch| {
                        let base = call.params.get("base").and_then(serde_json::Value::as_str);
                        worktrees::create(&call.context.workspace, branch, base).map(
```

Leave the rest of the arm alone; Task 4 rewrites it.

- [ ] **Step 6: Drop `path_hint` from the MCP schema**

In `crates/agency-mcp/src/lib.rs`, the `create_worktree` entry in `tools()` currently declares three properties. Replace that entry with:

```rust
        {
            "name": "create_worktree",
            "description": "Create a Git worktree and branch in the caller's Agency workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch": {
                        "type": "string",
                        "description": "New local branch name."
                    },
                    "base": {
                        "type": "string",
                        "description": "Existing revision from which to create the branch."
                    }
                },
                "required": ["branch"],
                "additionalProperties": false
            }
        }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo fmt --all && cargo test --workspace --all-targets --locked`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/agency-desktop/src/worktrees.rs crates/agency-desktop/src/main.rs crates/agency-mcp/src/lib.rs
git commit -m "feat(desktop): home worktrees under the primary's .agency directory"
```

---

### Task 4: Route worktree state changes through typed events

The `worktree.create` RPC handler writes to `self.worktrees` and `self.active_worktree` inline, which the event rule in `CLAUDE.md` forbids, and startup discovery reaches the same fields by a second path. Both converge on one reducer.

**Files:**
- Modify: `crates/agency-desktop/src/main.rs:842-908` — `AppEvent` gains two variants
- Modify: `crates/agency-desktop/src/main.rs` — `reduce_event` gains two arms; two reducer methods added near `select_worktree`
- Modify: `crates/agency-desktop/src/main.rs:2635-2668` — the `worktree.create` arm becomes an effect
- Modify: `crates/agency-desktop/src/main.rs:2411-2417` — `select_worktree`'s guard
- Test: `crates/agency-desktop/src/main.rs` (tests module)

**Interfaces:**
- Consumes: `worktrees::discover`, `worktrees::create` (Task 3), `Agency::emit`, `Agency::drain_events`, `Agency::for_testing`
- Produces:
  - `AppEvent::WorktreesDiscovered { worktrees: Vec<Worktree> }`
  - `AppEvent::WorktreeCreated { worktree: Worktree }`
  - `Agency::worktrees_discovered(&mut self, worktrees: Vec<Worktree>)`
  - `Agency::worktree_created(&mut self, worktree: Worktree)`

- [ ] **Step 1: Write the failing reducer tests**

Add to the `mod tests` block in `crates/agency-desktop/src/main.rs`. Note `Agency::for_testing()` builds against the real current directory, so these tests override `cwd` and `worktrees` the way `switching_worktrees_reseeds_agency_commands_and_requests_a_reload` already does.

```rust
    fn worktree_at(path: &std::path::Path, branch: &str) -> Worktree {
        Worktree {
            path: path.to_path_buf(),
            label: branch.to_owned(),
            branch: Some(branch.to_owned()),
        }
    }

    /// Discovery is the single source of truth for the tab strip, so the active
    /// tab is re-derived from cwd rather than carried across. Git reports the
    /// primary first, which is why index 0 is the fallback.
    #[test]
    fn discovering_worktrees_reresolves_the_active_tab_from_cwd() {
        let mut agency = Agency::for_testing();
        let primary = std::path::PathBuf::from("/repo");
        let feature = std::path::PathBuf::from("/repo/.agency/worktrees/feature");
        agency.cwd = feature.clone();

        let _ = agency.reduce_event(AppEvent::WorktreesDiscovered {
            worktrees: vec![
                worktree_at(&primary, "main"),
                worktree_at(&feature, "feature"),
            ],
        });

        assert_eq!(agency.worktrees.len(), 2);
        assert_eq!(agency.active_worktree, 1);
    }

    #[test]
    fn discovering_worktrees_falls_back_to_the_primary_when_cwd_is_gone() {
        let mut agency = Agency::for_testing();
        agency.cwd = std::path::PathBuf::from("/repo/.agency/worktrees/deleted");

        let _ = agency.reduce_event(AppEvent::WorktreesDiscovered {
            worktrees: vec![worktree_at(std::path::Path::new("/repo"), "main")],
        });

        assert_eq!(agency.active_worktree, 0);
    }

    /// An agent creating a worktree in the background must not move the user's
    /// view. The tab appears; focus stays put.
    ///
    /// This one needs a real repository: the reducer re-discovers rather than
    /// pushing the payload onto the list, so a fabricated path would never show
    /// up in the result.
    #[test]
    fn a_created_worktree_appends_a_tab_without_moving_focus() {
        let root = worktrees::tests_support::repository("created-tab");
        let mut agency = Agency::for_testing();
        agency.cwd = root.clone();
        agency.worktrees = vec![worktree_at(&root, "main")];
        agency.active_worktree = 0;
        let created = worktrees::create(&root, "feature", None).unwrap();

        let _ = agency.reduce_event(AppEvent::WorktreeCreated { worktree: created });

        assert_eq!(agency.worktrees.len(), 2);
        assert!(
            agency
                .worktrees
                .iter()
                .any(|worktree| worktree.branch.as_deref() == Some("feature"))
        );
        assert_eq!(agency.active_worktree, 0);
        assert_eq!(agency.cwd, root);

        std::fs::remove_dir_all(root).unwrap();
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --package agency-desktop --bin agency discovering_worktrees a_created_worktree`

Expected: FAIL to compile — `no variant named 'WorktreesDiscovered' found for enum 'AppEvent'`.

- [ ] **Step 3: Add the event variants**

In `crates/agency-desktop/src/main.rs`, add to the `AppEvent` enum, next to `SelectWorktree(usize)`:

```rust
    /// The tab strip as git reports it. Published at startup and after any
    /// change to the worktree set, so all three paths land in one reducer.
    WorktreesDiscovered {
        worktrees: Vec<Worktree>,
    },
    WorktreeCreated {
        worktree: Worktree,
    },
```

- [ ] **Step 4: Add the reducer methods**

In `crates/agency-desktop/src/main.rs`, add directly above `fn select_worktree`:

```rust
    /// The one place `worktrees` and `active_worktree` are written. An empty
    /// list is refused rather than rendered: git always reports at least the
    /// primary, so an empty result means the query failed, and dropping every
    /// tab would leave nothing to switch back to.
    fn worktrees_discovered(&mut self, worktrees: Vec<Worktree>) {
        if worktrees.is_empty() {
            return;
        }
        self.active_worktree = worktrees
            .iter()
            .position(|worktree| worktree.path == self.cwd)
            .unwrap_or(0);
        self.worktrees = worktrees;
    }

    /// Re-discovers rather than pushing the new worktree onto the list, so the
    /// tab strip stays exactly what git reports. A worktree created in some
    /// other repository simply will not appear, which is the correct outcome
    /// and needs no workspace comparison to arrange.
    fn worktree_created(&mut self, worktree: Worktree) {
        match worktrees::discover(&self.cwd) {
            Ok(discovered) => {
                self.worktrees_discovered(discovered);
                self.notice = Some(format!("Created worktree {}", worktree.label));
            }
            Err(error) => self.notice = Some(error),
        }
    }
```

- [ ] **Step 5: Dispatch the events**

In `reduce_event`, add next to the `AppEvent::SelectWorktree(index)` arm:

```rust
            AppEvent::WorktreesDiscovered { worktrees } => self.worktrees_discovered(worktrees),
            AppEvent::WorktreeCreated { worktree } => self.worktree_created(worktree),
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --package agency-desktop --bin agency discovering_worktrees a_created_worktree`

Expected: PASS, all three.

- [ ] **Step 7: Turn the RPC handler into an effect**

In `crates/agency-desktop/src/main.rs`, the `worktree.create` arm currently re-discovers and assigns `self.worktrees` / `self.active_worktree` inline. Replace the whole arm with:

```rust
                "worktree.create" => {
                    let branch = call
                        .params
                        .get("branch")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "create_worktree requires a branch".to_owned());
                    branch.and_then(|branch| {
                        let base = call.params.get("base").and_then(serde_json::Value::as_str);
                        worktrees::create(&call.context.workspace, branch, base).map(|worktree| {
                            let value = worktree_json(worktree.clone());
                            self.emit(AppEvent::WorktreeCreated { worktree });
                            serde_json::json!({
                                "caller": rpc_caller(&call.context),
                                "worktree": value
                            })
                        })
                    })
                }
```

`Worktree` already derives `Clone`, so `worktree_json` taking ownership costs one clone and keeps its signature.

- [ ] **Step 8: Publish discovery at startup**

In `build`, the worktree list is currently assigned directly into the struct literal. Leave that — it is the initial value, not a transition — but publish the event so any facet that later observes discovery sees it. Find the block near the end of `build` that runs `if spawn_agent_and_rpc {` after `let startup_notice = agency.notice.take();`, and add before it:

```rust
        let discovered = agency.worktrees.clone();
        agency.emit(AppEvent::WorktreesDiscovered {
            worktrees: discovered,
        });
```

The clone is bound first rather than written inline as the argument, so the immutable borrow of `agency.worktrees` ends before `emit` takes `&mut agency`.

- [ ] **Step 9: Correct `select_worktree`'s guard**

In `select_worktree`, the guard currently reads:

```rust
        if index == self.active_worktree {
            return;
        }
```

Replace it with a check against the actual invariant — "we are already there" — because Task 5 publishes `SelectWorktree(0)` after removing the active worktree, at a moment when `active_worktree` has already been clamped and could equal the requested index while `cwd` still points at the deleted checkout:

```rust
        if worktree.path == self.cwd {
            return;
        }
```

This is equivalent in every existing path, since `active_worktree` is by construction the index whose path is `cwd`.

- [ ] **Step 10: Run the full suite and commit**

Run: `cargo fmt --all && cargo test --workspace --all-targets --locked`

Expected: PASS, including the pre-existing `switching_worktrees_reseeds_agency_commands_and_requests_a_reload`, which relies on the guard changed in Step 9. That test pushes a worktree with a distinct temp path and switches to it, so `worktree.path == self.cwd` is false and it still proceeds.

```bash
git add crates/agency-desktop/src/main.rs crates/agency-desktop/src/worktrees.rs
git commit -m "feat(desktop): reduce worktree discovery and creation from typed events"
```

---

### Task 5: Remove worktrees end to end

The missing third of the loop: `worktrees::remove`, the `WorktreeRemoved` event, the `worktree.remove` RPC method, and the `remove_worktree` MCP tool.

**Files:**
- Modify: `crates/agency-desktop/src/worktrees.rs` — add `remove`
- Modify: `crates/agency-desktop/src/main.rs` — `AppEvent::WorktreeRemoved`, `worktree_removed` reducer, `worktree.remove` RPC arm
- Modify: `crates/agency-mcp/src/lib.rs` — `remove_worktree` tool and method mapping
- Modify: `README.md:110-111` — list the third tool
- Test: `crates/agency-desktop/src/worktrees.rs`, `crates/agency-desktop/src/main.rs`

**Interfaces:**
- Consumes: `worktrees::discover`, `config::worktrees_directory`, `Agency::emit`, `tests_support::{git, repository}` (Task 4)
- Produces:
  - `worktrees::remove(workspace: &Path, branch: &str) -> Result<Worktree, String>`
  - `AppEvent::WorktreeRemoved { branch: String }`
  - `Agency::worktree_removed(&mut self, branch: &str)`

- [ ] **Step 1: Write the failing tests for `remove`**

Add to the `mod tests` block in `crates/agency-desktop/src/worktrees.rs`:

```rust
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
        std::fs::write(sessions.join("session.json"), r#"{"conversation_id":"one"}"#).unwrap();
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
        assert!(worktree.path.exists(), "the worktree must survive a refusal");

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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --package agency-desktop --bin agency removing_a_worktree refuses_to_remove removes_a_worktree_from_inside`

Expected: FAIL to compile — `cannot find function 'remove' in this scope`.

- [ ] **Step 3: Write `remove`**

Add to `crates/agency-desktop/src/worktrees.rs`, below `create`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --package agency-desktop --bin agency removing_a_worktree refuses_to_remove removes_a_worktree_from_inside`

Expected: PASS, all six.

- [ ] **Step 5: Write the failing reducer tests**

Add to the `mod tests` block in `crates/agency-desktop/src/main.rs`:

```rust
    #[test]
    fn removing_an_inactive_worktree_drops_its_tab_and_keeps_focus() {
        let mut agency = Agency::for_testing();
        let primary = std::path::PathBuf::from("/repo");
        agency.cwd = primary.clone();
        agency.worktrees = vec![
            worktree_at(&primary, "main"),
            worktree_at(
                std::path::Path::new("/repo/.agency/worktrees/feature"),
                "feature",
            ),
        ];
        agency.active_worktree = 0;

        let _ = agency.reduce_event(AppEvent::WorktreeRemoved {
            branch: "feature".to_owned(),
        });

        assert_eq!(agency.worktrees.len(), 1);
        assert_eq!(agency.active_worktree, 0);
        assert_eq!(agency.cwd, primary);
        assert!(agency.drain_events().is_empty());
    }

    /// Removing the worktree the user is looking at cannot fail the caller —
    /// the tool would then succeed or fail based on which tab happens to be
    /// focused. The app moves to the primary instead, as a follow-up event so
    /// ordering stays deterministic rather than recursing into the handler.
    #[test]
    fn removing_the_active_worktree_falls_back_to_the_primary_tab() {
        let mut agency = Agency::for_testing();
        let primary = std::path::PathBuf::from("/repo");
        let feature = std::path::PathBuf::from("/repo/.agency/worktrees/feature");
        agency.cwd = feature.clone();
        agency.worktrees = vec![
            worktree_at(&primary, "main"),
            worktree_at(&feature, "feature"),
        ];
        agency.active_worktree = 1;

        let _ = agency.reduce_event(AppEvent::WorktreeRemoved {
            branch: "feature".to_owned(),
        });

        assert_eq!(agency.worktrees.len(), 1);
        assert!(
            agency
                .drain_events()
                .iter()
                .any(|event| matches!(event, AppEvent::SelectWorktree(0))),
            "the app must move off the worktree it just deleted"
        );
    }
```

- [ ] **Step 6: Run the reducer tests to verify they fail**

Run: `cargo test --package agency-desktop --bin agency removing_an_inactive_worktree removing_the_active_worktree`

Expected: FAIL to compile — `no variant named 'WorktreeRemoved' found for enum 'AppEvent'`.

- [ ] **Step 7: Add the event, the reducer, and the dispatch**

In `crates/agency-desktop/src/main.rs`, add to `AppEvent` next to `WorktreeCreated`:

```rust
    WorktreeRemoved {
        branch: String,
    },
```

Add the reducer method next to `worktree_created`:

```rust
    /// Drops the tab. If the user was looking at it, the move to the primary is
    /// published as a follow-up event rather than called directly, so ordering
    /// stays deterministic and `select_worktree`'s teardown — revoking RPC
    /// capabilities, clearing agents, reloading sessions — runs exactly once.
    fn worktree_removed(&mut self, branch: &str) {
        let was_active = self
            .worktrees
            .get(self.active_worktree)
            .is_some_and(|worktree| worktree.branch.as_deref() == Some(branch));
        self.worktrees
            .retain(|worktree| worktree.branch.as_deref() != Some(branch));
        if self.worktrees.is_empty() {
            return;
        }
        if was_active {
            self.active_worktree = self
                .active_worktree
                .min(self.worktrees.len().saturating_sub(1));
            self.emit(AppEvent::SelectWorktree(0));
        } else {
            self.active_worktree = self
                .worktrees
                .iter()
                .position(|worktree| worktree.path == self.cwd)
                .unwrap_or(0);
        }
        self.notice = Some(format!("Removed worktree {branch}"));
    }
```

Add the dispatch arm in `reduce_event`, next to `WorktreeCreated`:

```rust
            AppEvent::WorktreeRemoved { branch } => self.worktree_removed(&branch),
```

- [ ] **Step 8: Run the reducer tests to verify they pass**

Run: `cargo test --package agency-desktop --bin agency removing_an_inactive_worktree removing_the_active_worktree`

Expected: PASS.

- [ ] **Step 9: Add the RPC arm**

In `handle_rpc_calls` in `crates/agency-desktop/src/main.rs`, add after the `"worktree.create"` arm:

```rust
                "worktree.remove" => {
                    let branch = call
                        .params
                        .get("branch")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "remove_worktree requires a branch".to_owned());
                    branch.and_then(|branch| {
                        worktrees::remove(&call.context.workspace, branch).map(|worktree| {
                            let value = worktree_json(worktree);
                            self.emit(AppEvent::WorktreeRemoved {
                                branch: branch.to_owned(),
                            });
                            serde_json::json!({
                                "caller": rpc_caller(&call.context),
                                "worktree": value
                            })
                        })
                    })
                }
```

- [ ] **Step 10: Write the provider-neutrality test**

`CLAUDE.md` requires every resolution path to be covered by a test using a provider that matches no shipped agent. `SessionContext.provider` is a `String`, and the worktree path must treat it as opaque data. Add to the `mod tests` block in `crates/agency-desktop/src/main.rs`:

```rust
    /// Worktree resolution must not care who is calling. `blueprint` is not a
    /// provider Agency ships, and the caller block has to carry it through
    /// unchanged — a `match` on provider anywhere in this path fails here
    /// rather than at the next integration.
    #[test]
    fn worktree_calls_resolve_for_a_provider_agency_does_not_ship() {
        let context = SessionContext {
            conversation_id: "conversation-1".to_owned(),
            workspace: std::path::PathBuf::from("/repo"),
            provider: "blueprint".to_owned(),
            provider_session_id: Some("blueprint-9".to_owned()),
            generation: 3,
        };

        assert_eq!(
            rpc_caller(&context),
            serde_json::json!({
                "agency_session_id": "conversation-1",
                "provider": "blueprint",
                "provider_session_id": "blueprint-9",
                "generation": 3
            })
        );
    }
```

- [ ] **Step 11: Add the MCP tool**

In `crates/agency-mcp/src/lib.rs`, add a third entry to the array returned by `tools()`, after `create_worktree`:

```rust
        {
            "name": "remove_worktree",
            "description": "Remove a Git worktree from the caller's Agency workspace. The branch is kept; the worktree's session history is deleted with it. Refuses a worktree with uncommitted changes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch": {
                        "type": "string",
                        "description": "Branch whose worktree should be removed."
                    }
                },
                "required": ["branch"],
                "additionalProperties": false
            }
        }
```

And add the method mapping in `call_tool`:

```rust
    let method = match name {
        "list_worktrees" => "worktree.list",
        "create_worktree" => "worktree.create",
        "remove_worktree" => "worktree.remove",
        _ => return rpc_error(id, -32602, format!("Unknown Agency tool: {name}")),
    };
```

- [ ] **Step 12: Document the tool**

In `README.md`, the agent tools list currently reads:

```
- `list_worktrees`
- `create_worktree`
```

Replace it with:

```
- `list_worktrees`
- `create_worktree`
- `remove_worktree`
```

- [ ] **Step 13: Run the full suite and commit**

Run: `cargo fmt --all && cargo build --workspace --locked && cargo test --workspace --all-targets --locked`

Expected: PASS.

```bash
git add crates/agency-desktop/src/worktrees.rs crates/agency-desktop/src/main.rs crates/agency-mcp/src/lib.rs README.md
git commit -m "feat: remove worktrees through MCP, RPC, and a typed event"
```

---

### Task 6: Keep the explorer out of the worktrees directory

Worktrees now live inside the primary's tree. Expanding `.agency/worktrees` would render a full copy of the repository inside itself, once per worktree, and let the same file be opened under two paths.

**Files:**
- Modify: `crates/agency-desktop/src/main.rs:5599-5631` — `collect_explorer_entries` takes a `root`
- Modify: `crates/agency-desktop/src/main.rs:2452-2456` — `explorer_entries` passes it
- Test: `crates/agency-desktop/src/main.rs` (tests module)

**Interfaces:**
- Consumes: `config::worktrees_directory` (Task 1)
- Produces: `collect_explorer_entries(root: &Path, directory: &Path, depth: usize, expanded: &HashSet<PathBuf>, entries: &mut Vec<ExplorerEntry>)`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/agency-desktop/src/main.rs`:

```rust
    /// The skip matches the resolved path, not the directory name, so a
    /// `worktrees` directory that is part of the project stays visible.
    #[test]
    fn the_explorer_hides_the_worktrees_directory_but_not_others() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "agency-explorer-worktrees-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".agency").join("worktrees").join("feature")).unwrap();
        std::fs::create_dir_all(root.join("docs").join("worktrees")).unwrap();
        let mut expanded = HashSet::new();
        expanded.insert(root.join(".agency"));
        expanded.insert(root.join("docs"));

        let mut entries = Vec::new();
        collect_explorer_entries(&root, &root, 0, &expanded, &mut entries);
        let paths = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();

        assert!(paths.contains(&root.join(".agency")));
        assert!(
            !paths.contains(&root.join(".agency").join("worktrees")),
            "the worktrees directory must not appear in the primary's tree"
        );
        assert!(paths.contains(&root.join("docs").join("worktrees")));

        std::fs::remove_dir_all(root).unwrap();
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --package agency-desktop --bin agency the_explorer_hides_the_worktrees_directory`

Expected: FAIL to compile — `collect_explorer_entries` takes four arguments, this call passes five.

- [ ] **Step 3: Add the `root` parameter and the skip**

In `crates/agency-desktop/src/main.rs`, change the signature and the recursive call:

```rust
/// Walks the workspace for the file explorer.
///
/// `root` is threaded through the recursion so the workspace's own
/// `.agency/worktrees` can be skipped: every worktree is a full checkout of
/// this repository, so rendering it here would show the project inside itself
/// and let one file be opened under two paths. Matched by resolved path rather
/// than by name, so a `worktrees` directory that belongs to the project stays
/// visible.
fn collect_explorer_entries(
    root: &std::path::Path,
    directory: &std::path::Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    entries: &mut Vec<ExplorerEntry>,
) {
```

Inside the `for child in children` loop, add the skip immediately after `let path = child.path();`:

```rust
    let worktrees_directory = config::worktrees_directory(root);
    for child in children {
        let path = child.path();
        if path == worktrees_directory {
            continue;
        }
        let directory = child.file_type().is_ok_and(|kind| kind.is_dir());
```

Hoist `worktrees_directory` above the loop as shown so it is computed once per directory rather than once per entry.

Update the recursive call at the bottom of the loop:

```rust
        if directory && expanded.contains(&path) {
            collect_explorer_entries(root, &path, depth + 1, expanded, entries);
        }
```

- [ ] **Step 4: Update the caller**

In `explorer_entries`:

```rust
    fn explorer_entries(&self) -> Vec<ExplorerEntry> {
        let mut entries = Vec::new();
        collect_explorer_entries(&self.cwd, &self.cwd, 0, &self.explorer.expanded, &mut entries);
        entries
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --package agency-desktop --bin agency the_explorer_hides_the_worktrees_directory`

Expected: PASS.

- [ ] **Step 6: Run the full suite and commit**

Run: `cargo fmt --all && cargo build --workspace --locked && cargo test --workspace --all-targets --locked`

Expected: PASS.

```bash
git add crates/agency-desktop/src/main.rs
git commit -m "fix(desktop): keep the explorer out of the worktrees directory"
```

---

## Verification

After Task 6, confirm the whole feature by hand — the automated tests cover the units, but the loop through a running app is what proves the wiring.

- [ ] Run `cargo build --workspace --locked && cargo test --workspace --all-targets --locked`
- [ ] Launch the app in this repository, start an agent, and ask it to call `create_worktree` with branch `scratch/verify`
- [ ] Confirm a new tab appears, that focus does **not** move to it, and that the checkout is at `.agency/worktrees/scratch%2Fverify`
- [ ] Confirm `git status` in the repository root is clean
- [ ] Switch to the new tab, start a session, then switch back to the primary and confirm the session list differs per tab
- [ ] Ask the agent to call `remove_worktree` with branch `scratch/verify`; confirm the tab disappears and `git branch --list scratch/verify` still reports the branch
- [ ] Delete the leftover branch by hand: `git branch -D scratch/verify`
