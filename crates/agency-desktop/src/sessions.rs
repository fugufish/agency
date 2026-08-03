use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agency_agents::Provider;
use serde::{Deserialize, Serialize};

use crate::config::{path_component, workspace_config_directory};

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
}
