use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agency_agents::Provider;
use serde::{Deserialize, Serialize};

use crate::config::{path_component, workspace_config_directory};
use crate::worktrees::Worktree;

const LEGACY_SESSION_REGISTRY_FILE: &str = "sessions.json";
const SESSION_CONFIG_FILE: &str = "session.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum StoredProvider {
    Codex,
    Claude,
}

impl From<StoredProvider> for Provider {
    fn from(provider: StoredProvider) -> Self {
        match provider {
            StoredProvider::Codex => Self::Codex,
            StoredProvider::Claude => Self::Claude,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub conversation_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub codex_id: Option<String>,
    #[serde(default)]
    pub claude_id: Option<String>,
    #[serde(default)]
    pub updated_at_millis: u64,
}

impl SessionRecord {
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    pub fn binding(&self, provider: Provider) -> Option<&str> {
        match provider {
            Provider::Codex => self.codex_id.as_deref(),
            Provider::Claude => self.claude_id.as_deref(),
        }
    }

    pub fn contains(&self, provider: Provider, id: &str) -> bool {
        self.binding(provider) == Some(id)
    }

    fn set_binding(&mut self, provider: Provider, id: String) {
        match provider {
            Provider::Codex => self.codex_id = Some(id),
            Provider::Claude => self.claude_id = Some(id),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredSessionRecord {
    Legacy {
        provider: StoredProvider,
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default = "new_conversation_id")]
        conversation_id: String,
    },
    Current(SessionRecord),
}

pub struct SessionRegistry {
    records: Vec<SessionRecord>,
    sessions_directory: PathBuf,
}

impl SessionRegistry {
    pub fn empty(workspace: &Path) -> Self {
        Self {
            records: Vec::new(),
            sessions_directory: worktree_sessions_directory(workspace),
        }
    }

    pub fn load(workspace: &Path) -> Result<Self, String> {
        let sessions_directory = worktree_sessions_directory(workspace);
        let mut records = Vec::new();
        if sessions_directory.exists() {
            let entries = fs::read_dir(&sessions_directory).map_err(|error| {
                format!("Could not read {}: {error}", sessions_directory.display())
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!(
                        "Could not read an entry in {}: {error}",
                        sessions_directory.display()
                    )
                })?;
                let path = entry.path().join(SESSION_CONFIG_FILE);
                if !path.is_file() {
                    continue;
                }
                let source = fs::read_to_string(&path)
                    .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
                let mut record: SessionRecord = serde_json::from_str(&source)
                    .map_err(|error| format!("Could not parse {}: {error}", path.display()))?;
                if record.updated_at_millis == 0 {
                    record.updated_at_millis = modified_at_millis(&path);
                }
                records.push(record);
            }
            return Ok(Self {
                records,
                sessions_directory,
            });
        }

        let legacy_path = workspace_config_directory(workspace).join(LEGACY_SESSION_REGISTRY_FILE);
        if !legacy_path.exists() {
            return Ok(Self {
                records,
                sessions_directory,
            });
        }
        let source = fs::read_to_string(&legacy_path)
            .map_err(|error| format!("Could not read {}: {error}", legacy_path.display()))?;
        let stored: Vec<StoredSessionRecord> = serde_json::from_str(&source)
            .map_err(|error| format!("Could not parse {}: {error}", legacy_path.display()))?;
        for stored in stored {
            let (conversation_id, name, provider, id) = match stored {
                StoredSessionRecord::Current(record) => {
                    if let Some(existing) = records
                        .iter_mut()
                        .find(|existing| existing.conversation_id == record.conversation_id)
                    {
                        existing.codex_id = existing.codex_id.take().or(record.codex_id);
                        existing.claude_id = existing.claude_id.take().or(record.claude_id);
                        existing.name = existing.name.take().or(record.name);
                    } else {
                        records.push(record);
                    }
                    continue;
                }
                StoredSessionRecord::Legacy {
                    provider,
                    id,
                    name,
                    conversation_id,
                } => (conversation_id, name, provider.into(), id),
            };
            if let Some(existing) = records
                .iter_mut()
                .find(|record| record.conversation_id == conversation_id)
            {
                existing.set_binding(provider, id);
                existing.name = existing.name.take().or(name);
            } else {
                let mut record = SessionRecord {
                    conversation_id,
                    name,
                    codex_id: None,
                    claude_id: None,
                    updated_at_millis: now_millis(),
                };
                record.set_binding(provider, id);
                records.push(record);
            }
        }
        let registry = Self {
            records,
            sessions_directory,
        };
        registry.save()?;
        Ok(registry)
    }

    pub fn records(&self) -> &[SessionRecord] {
        &self.records
    }

    pub fn record(
        &mut self,
        provider: Provider,
        id: String,
        name: Option<String>,
    ) -> Result<(), String> {
        if let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.contains(provider, &id))
        {
            if name.is_some() && record.name != name {
                record.name = name;
                record.updated_at_millis = now_millis();
                return self.save();
            }
            return Ok(());
        }
        self.records.push(SessionRecord {
            conversation_id: new_conversation_id(),
            name,
            codex_id: (provider == Provider::Codex).then_some(id.clone()),
            claude_id: (provider == Provider::Claude).then_some(id),
            updated_at_millis: now_millis(),
        });
        self.save()
    }

    pub fn record_binding(
        &mut self,
        conversation_id: String,
        provider: Provider,
        id: String,
        name: Option<String>,
    ) -> Result<(), String> {
        if let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.conversation_id == conversation_id)
        {
            record.set_binding(provider, id);
            if name.is_some() {
                record.name = name;
            }
            record.updated_at_millis = now_millis();
            return self.save();
        }
        self.records.push(SessionRecord {
            conversation_id,
            name,
            codex_id: (provider == Provider::Codex).then_some(id.clone()),
            claude_id: (provider == Provider::Claude).then_some(id),
            updated_at_millis: now_millis(),
        });
        self.save()
    }

    pub fn name_if_missing(
        &mut self,
        provider: Provider,
        id: &str,
        name: String,
    ) -> Result<(), String> {
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.contains(provider, id))
        else {
            return self.record(provider, id.to_owned(), Some(name));
        };
        if record.name.is_none() {
            record.name = Some(name);
            record.updated_at_millis = now_millis();
            self.save()
        } else {
            Ok(())
        }
    }

    pub fn remove(&mut self, index: usize) -> Result<SessionRecord, String> {
        if index >= self.records.len() {
            return Err("Session no longer exists".to_owned());
        }
        let record = self.records.remove(index);
        let directory = self.session_directory(record.conversation_id());
        if let Err(error) = fs::remove_dir_all(&directory) {
            self.records.insert(index, record);
            return Err(format!("Could not remove {}: {error}", directory.display()));
        }
        Ok(record)
    }

    pub fn session_directory(&self, conversation_id: &str) -> PathBuf {
        self.sessions_directory
            .join(path_component(conversation_id))
    }

    fn save(&self) -> Result<(), String> {
        for record in &self.records {
            let directory = self.session_directory(record.conversation_id());
            fs::create_dir_all(&directory)
                .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
            let path = directory.join(SESSION_CONFIG_FILE);
            let data = serde_json::to_string_pretty(record)
                .map_err(|error| format!("Could not encode agent session: {error}"))?;
            fs::write(&path, format!("{data}\n"))
                .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
        }
        Ok(())
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn modified_at_millis(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// Sessions live beside the worktree that produced them. `git worktree remove`
/// deletes a worktree directory wholesale, ignored files included, so a
/// worktree's history is collected with it and nothing has to sweep for
/// orphans later.
pub fn worktree_sessions_directory(workspace: &Path) -> PathBuf {
    workspace_config_directory(workspace).join("sessions")
}

/// Where history moves when nothing claims it. Never deleted: a legacy key
/// with no live worktree is still a user's conversation history.
const LEGACY_SESSIONS_DIRECTORY: &str = "legacy-sessions";

/// Sessions used to live under the primary worktree, in
/// `.agency/worktrees/<encoded-branch>/sessions/`, with the literal `root`
/// standing in for the primary itself. Checkouts now occupy exactly that
/// namespace under exactly that encoding, so every stale key is a directory
/// name a future worktree cannot have — `create` refuses a path that already
/// exists, permanently, for a path the user never sees.
///
/// Runs against the **primary** worktree, which `worktrees[0]` always is: git
/// reports the primary first, and the discovery fallback in `build` yields a
/// single entry. Taking the list rather than a path is what keys the migration
/// to the primary no matter which worktree Agency was launched from; passing
/// the active worktree instead would look in the wrong repository root, find
/// nothing, and strand the history behind the destination-exists guard.
///
/// Every entry under `.agency/worktrees/` that is not a live checkout and that
/// holds a `sessions/` directory is claimed:
///
/// - `root` moves beside the primary,
/// - a key that encodes some live worktree's branch moves inside that worktree,
/// - anything else moves to `.agency/legacy-sessions/<key>`, freeing the name.
///
/// Best effort and infallible. A launch that cannot move history is still a
/// launch that should start, and an existing destination is never overwritten.
pub fn migrate_legacy_sessions(worktrees: &[Worktree]) {
    let Some(primary) = worktrees.first() else {
        return;
    };
    let legacy_directory = workspace_config_directory(&primary.path).join("worktrees");
    let Ok(entries) = fs::read_dir(&legacy_directory) else {
        return;
    };
    for entry in entries.flatten() {
        let key_directory = entry.path();
        if !key_directory.is_dir() || holds_a_live_worktree(&key_directory, worktrees) {
            continue;
        }
        if !key_directory.join("sessions").is_dir() {
            continue;
        }
        let Some(key) = key_directory.file_name().and_then(|key| key.to_str()) else {
            continue;
        };

        if key == "root" {
            adopt_legacy_sessions(&key_directory, &worktree_sessions_directory(&primary.path));
        } else if let Some(owner) = worktrees.iter().find(|worktree| {
            worktree
                .branch
                .as_deref()
                .is_some_and(|branch| path_component(branch) == key)
        }) {
            adopt_legacy_sessions(&key_directory, &worktree_sessions_directory(&owner.path));
        } else {
            let destination = workspace_config_directory(&primary.path)
                .join(LEGACY_SESSIONS_DIRECTORY)
                .join(key);
            if destination.exists() {
                continue;
            }
            if let Some(parent) = destination.parent()
                && fs::create_dir_all(parent).is_err()
            {
                continue;
            }
            let _ = fs::rename(&key_directory, &destination);
        }
    }
}

/// A checkout is never migration's business. Matches a worktree sitting at the
/// entry *or* beneath it, so a directory that turned out to contain live work
/// is left alone rather than moved out from under git.
fn holds_a_live_worktree(directory: &Path, worktrees: &[Worktree]) -> bool {
    let canonical = fs::canonicalize(directory);
    worktrees.iter().any(|worktree| {
        worktree.path.starts_with(directory)
            || canonical.as_ref().is_ok_and(|canonical| {
                fs::canonicalize(&worktree.path)
                    .is_ok_and(|worktree| worktree.starts_with(canonical))
            })
    })
}

/// Moves `<key>/sessions` to `destination` unless something already lives
/// there, then collects the key directory if the move emptied it.
fn adopt_legacy_sessions(key_directory: &Path, destination: &Path) {
    let legacy = key_directory.join("sessions");
    if destination.exists() {
        return;
    }
    if let Some(parent) = destination.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    if fs::rename(&legacy, destination).is_ok() {
        let _ = fs::remove_dir(key_directory);
    }
}

pub fn new_conversation_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("conversation-{}-{timestamp}", std::process::id())
}

pub fn name_from_prompt(prompt: &str) -> Option<String> {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }

    const MAX_CHARS: usize = 48;
    if normalized.chars().count() <= MAX_CHARS {
        return Some(normalized);
    }
    let mut name = normalized.chars().take(MAX_CHARS - 1).collect::<String>();
    name.push('…');
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn session_records_round_trip_provider_and_id() {
        let record = SessionRecord {
            conversation_id: "conversation-123".to_owned(),
            name: Some("Fix terminal rendering".to_owned()),
            codex_id: Some("thread-123".to_owned()),
            claude_id: None,
            updated_at_millis: 123,
        };
        let encoded = serde_json::to_string(&record).unwrap();
        let decoded: SessionRecord = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, record);
        assert_eq!(decoded.binding(Provider::Codex), Some("thread-123"));
    }

    #[test]
    fn derives_a_compact_name_from_the_first_prompt() {
        assert_eq!(
            name_from_prompt("  Fix   the terminal\nrendering bug  "),
            Some("Fix the terminal rendering bug".to_owned())
        );
        assert_eq!(name_from_prompt(" \n "), None);
        assert!(name_from_prompt(&"x".repeat(80)).unwrap().ends_with('…'));
    }

    #[test]
    fn older_session_records_load_without_a_name() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!(
            "agency-session-migration-{}-{unique}",
            std::process::id()
        ));
        let directory = workspace_config_directory(&workspace);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(LEGACY_SESSION_REGISTRY_FILE),
            r#"[{"provider":"claude","id":"session-123"}]"#,
        )
        .unwrap();
        let registry = SessionRegistry::load(&workspace).unwrap();
        assert_eq!(
            registry.records()[0].binding(Provider::Claude),
            Some("session-123")
        );
        assert!(
            registry.records()[0]
                .conversation_id
                .starts_with("conversation-")
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn removing_a_session_is_persisted() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!(
            "agency-session-remove-{}-{unique}",
            std::process::id()
        ));
        let mut registry = SessionRegistry::empty(&workspace);
        registry
            .record(
                Provider::Codex,
                "first".to_owned(),
                Some("First".to_owned()),
            )
            .unwrap();
        registry
            .record(
                Provider::Claude,
                "second".to_owned(),
                Some("Second".to_owned()),
            )
            .unwrap();

        let removed = registry.remove(0).unwrap();
        let reloaded = SessionRegistry::load(&workspace).unwrap();

        assert_eq!(removed.binding(Provider::Codex), Some("first"));
        assert_eq!(reloaded.records().len(), 1);
        assert_eq!(
            reloaded.records()[0].binding(Provider::Claude),
            Some("second")
        );

        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn provider_bindings_share_a_logical_conversation() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!(
            "agency-session-binding-{}-{unique}",
            std::process::id()
        ));
        let mut registry = SessionRegistry::empty(&workspace);
        registry
            .record_binding(
                "conversation-one".to_owned(),
                Provider::Codex,
                "codex-one".to_owned(),
                Some("Shared work".to_owned()),
            )
            .unwrap();
        registry
            .record_binding(
                "conversation-one".to_owned(),
                Provider::Claude,
                "claude-one".to_owned(),
                Some("Shared work".to_owned()),
            )
            .unwrap();
        let reloaded = SessionRegistry::load(&workspace).unwrap();
        assert_eq!(reloaded.records().len(), 1);
        assert_eq!(
            reloaded.records()[0].binding(Provider::Codex),
            Some("codex-one")
        );
        assert_eq!(
            reloaded.records()[0].binding(Provider::Claude),
            Some("claude-one")
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }

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

    /// A scratch directory that is not a repository. The migration never shells
    /// out to git, so a plain directory is enough to drive it.
    fn scratch(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace =
            std::env::temp_dir().join(format!("agency-{name}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        workspace
    }

    fn worktree(path: &Path, branch: &str) -> Worktree {
        Worktree {
            path: path.to_path_buf(),
            label: branch.to_owned(),
            branch: Some(branch.to_owned()),
        }
    }

    fn write_legacy_session(directory: &Path, conversation: &str, codex: &str) {
        std::fs::create_dir_all(directory.join(conversation)).unwrap();
        std::fs::write(
            directory.join(conversation).join(SESSION_CONFIG_FILE),
            format!(r#"{{"conversation_id":"{conversation}","codex_id":"{codex}"}}"#),
        )
        .unwrap();
    }

    /// Sessions used to be keyed by branch under the primary, with the literal
    /// `root` standing in for the primary itself. `root` moves beside the
    /// primary; the other keys are handled by the cases below, because a
    /// repository being upgraded still holds one directory per branch it ever
    /// ran a session on, in the namespace checkouts now occupy.
    #[test]
    fn legacy_root_sessions_move_beside_the_primary_worktree() {
        let workspace = scratch("session-root-migration");
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

        migrate_legacy_sessions(&[worktree(&workspace, "main")]);

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
        let workspace = scratch("session-root-noop");
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

        migrate_legacy_sessions(&[worktree(&workspace, "main")]);

        let registry = SessionRegistry::load(&workspace).unwrap();
        assert_eq!(registry.records().len(), 1);
        assert_eq!(
            registry.records()[0].binding(Provider::Codex),
            Some("codex-new")
        );

        std::fs::remove_dir_all(workspace).unwrap();
    }

    /// A branch that ran sessions under the old layout and has a worktree
    /// today: its history belongs inside that worktree, where the rest of the
    /// worktree's history now lives and where removal will collect it.
    #[test]
    fn legacy_branch_sessions_move_inside_the_worktree_that_owns_them() {
        let workspace = scratch("session-branch-migration");
        // A worktree created under the old sibling scheme: still live, still a
        // tab, and its history is still keyed by branch under the primary.
        let checkout = scratch("session-branch-sibling");
        let legacy = workspace_config_directory(&workspace)
            .join("worktrees")
            .join(path_component("feature/tabs"))
            .join("sessions");
        write_legacy_session(&legacy, "conversation-1", "codex-1");

        migrate_legacy_sessions(&[
            worktree(&workspace, "main"),
            worktree(&checkout, "feature/tabs"),
        ]);

        let registry = SessionRegistry::load(&checkout).unwrap();
        assert_eq!(registry.records().len(), 1);
        assert_eq!(
            registry.records()[0].binding(Provider::Codex),
            Some("codex-1")
        );
        assert!(
            SessionRegistry::load(&workspace)
                .unwrap()
                .records()
                .is_empty(),
            "the primary must not adopt another worktree's history"
        );
        assert!(
            !legacy.parent().unwrap().exists(),
            "the legacy key must be freed for a checkout of that branch"
        );

        std::fs::remove_dir_all(workspace).unwrap();
        std::fs::remove_dir_all(checkout).unwrap();
    }

    /// A branch nobody has checked out. The name still has to be freed, or
    /// `create` refuses that branch forever — but the history behind it is a
    /// user's conversations, so it is moved aside rather than deleted.
    #[test]
    fn orphaned_legacy_sessions_move_aside_rather_than_being_deleted() {
        let workspace = scratch("session-orphan-migration");
        let legacy = workspace_config_directory(&workspace)
            .join("worktrees")
            .join("abandoned")
            .join("sessions");
        write_legacy_session(&legacy, "conversation-1", "codex-1");

        migrate_legacy_sessions(&[worktree(&workspace, "main")]);

        assert!(
            !workspace_config_directory(&workspace)
                .join("worktrees")
                .join("abandoned")
                .exists(),
            "the key must be freed"
        );
        assert!(
            workspace_config_directory(&workspace)
                .join("legacy-sessions")
                .join("abandoned")
                .join("sessions")
                .join("conversation-1")
                .join(SESSION_CONFIG_FILE)
                .is_file(),
            "the history must survive the move"
        );

        std::fs::remove_dir_all(workspace).unwrap();
    }

    /// The property that matters most: a live checkout sits under
    /// `.agency/worktrees/` too, so a migration that claims one moves a user's
    /// working tree out from under git.
    ///
    /// Both shapes are here, because the two arms fail differently. A checkout
    /// whose key still encodes its branch would have its own tracked
    /// `sessions/` directory relocated into `.agency/sessions/`; a checkout
    /// whose key no longer matches any branch — git lets a branch be renamed
    /// after the worktree is made — would be moved wholesale into
    /// `legacy-sessions/`, checkout, `.git` file and all.
    #[test]
    fn a_live_checkout_is_left_untouched() {
        let workspace = scratch("session-live-checkout");
        let worktrees_directory = workspace_config_directory(&workspace).join("worktrees");
        let named = worktrees_directory.join("feature");
        let renamed = worktrees_directory.join("stale-name");
        for checkout in [&named, &renamed] {
            std::fs::create_dir_all(checkout).unwrap();
            std::fs::write(checkout.join(".git"), "gitdir: elsewhere\n").unwrap();
            // A tracked directory that happens to be called `sessions`. The
            // migration has no business reading a checkout's contents at all.
            std::fs::create_dir_all(checkout.join("sessions")).unwrap();
            std::fs::write(checkout.join("sessions").join("fixture.rs"), "fn main() {}").unwrap();
        }

        migrate_legacy_sessions(&[
            worktree(&workspace, "main"),
            worktree(&named, "feature"),
            worktree(&renamed, "renamed"),
        ]);

        for checkout in [&named, &renamed] {
            assert!(
                checkout.join("sessions").join("fixture.rs").is_file(),
                "nothing inside a live checkout may be moved: {}",
                checkout.display()
            );
            assert!(checkout.join(".git").is_file());
            assert!(
                !worktree_sessions_directory(checkout).exists(),
                "the checkout's own files must not be re-filed as session history"
            );
        }
        assert!(
            !workspace_config_directory(&workspace)
                .join(LEGACY_SESSIONS_DIRECTORY)
                .exists(),
            "a live checkout must never be moved aside"
        );

        std::fs::remove_dir_all(workspace).unwrap();
    }

    /// The reported failure: a session recorded under the old layout on branch
    /// `feature` left `.agency/worktrees/feature/` behind, and `create` refuses
    /// a path that already exists — so that branch could never get a worktree
    /// again, for a directory the user never sees.
    #[test]
    fn migration_frees_a_branch_name_a_legacy_session_directory_had_claimed() {
        let root = crate::worktrees::tests_support::repository("session-legacy-blocks-create");
        write_legacy_session(
            &workspace_config_directory(&root)
                .join("worktrees")
                .join("feature")
                .join("sessions"),
            "conversation-1",
            "codex-1",
        );

        migrate_legacy_sessions(&crate::worktrees::discover(&root).unwrap());

        let created = crate::worktrees::create(&root, "feature", None).unwrap();
        assert_eq!(
            created.path,
            workspace_config_directory(&root)
                .join("worktrees")
                .join("feature")
        );
        assert!(
            workspace_config_directory(&root)
                .join("legacy-sessions")
                .join("feature")
                .join("sessions")
                .join("conversation-1")
                .join(SESSION_CONFIG_FILE)
                .is_file()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Launched from a linked worktree — the habit the old sibling layout
    /// encouraged — the migration must still work on the primary. Keying it to
    /// the active worktree looks in a directory the legacy layout never used,
    /// finds nothing, and strands the history the moment a new session writes
    /// `.agency/sessions/` and trips the destination-exists guard.
    #[test]
    fn the_migration_runs_against_the_primary_when_launched_from_a_worktree() {
        let root = crate::worktrees::tests_support::repository("session-launch-from-worktree");
        let linked = crate::worktrees::create(&root, "feature", None).unwrap();
        write_legacy_session(
            &workspace_config_directory(&root)
                .join("worktrees")
                .join("root")
                .join("sessions"),
            "conversation-1",
            "codex-1",
        );

        // Exactly what `build` does when Agency starts inside a worktree.
        let worktrees = crate::worktrees::discover(&linked.path).unwrap();
        migrate_legacy_sessions(&worktrees);

        let registry = SessionRegistry::load(&root).unwrap();
        assert_eq!(registry.records().len(), 1);
        assert_eq!(
            registry.records()[0].binding(Provider::Codex),
            Some("codex-1")
        );
        assert!(
            !workspace_config_directory(&root)
                .join("worktrees")
                .join("root")
                .exists()
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
