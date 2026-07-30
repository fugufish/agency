use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agency_agents::Provider;
use serde::{Deserialize, Serialize};

use crate::config::workspace_config_directory;

const SESSION_REGISTRY_FILE: &str = "sessions.json";

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
    path: PathBuf,
}

impl SessionRegistry {
    pub fn empty(workspace: &Path) -> Self {
        Self {
            records: Vec::new(),
            path: workspace_config_directory(workspace).join(SESSION_REGISTRY_FILE),
        }
    }

    pub fn load(workspace: &Path) -> Result<Self, String> {
        let path = workspace_config_directory(workspace).join(SESSION_REGISTRY_FILE);
        if !path.exists() {
            return Ok(Self::empty(workspace));
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        let stored: Vec<StoredSessionRecord> = serde_json::from_str(&source)
            .map_err(|error| format!("Could not parse {}: {error}", path.display()))?;
        let mut records: Vec<SessionRecord> = Vec::new();
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
                };
                record.set_binding(provider, id);
                records.push(record);
            }
        }
        Ok(Self { records, path })
    }

    pub fn records(&self) -> &[SessionRecord] {
        &self.records
    }

    pub fn find(&self, provider: Provider, id: &str) -> Option<&SessionRecord> {
        self.records
            .iter()
            .find(|record| record.contains(provider, id))
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
                return self.save();
            }
            return Ok(());
        }
        self.records.push(SessionRecord {
            conversation_id: new_conversation_id(),
            name,
            codex_id: (provider == Provider::Codex).then_some(id.clone()),
            claude_id: (provider == Provider::Claude).then_some(id),
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
            return self.save();
        }
        self.records.push(SessionRecord {
            conversation_id,
            name,
            codex_id: (provider == Provider::Codex).then_some(id.clone()),
            claude_id: (provider == Provider::Claude).then_some(id),
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
        if let Err(error) = self.save() {
            self.records.insert(index, record);
            return Err(error);
        }
        Ok(record)
    }

    fn save(&self) -> Result<(), String> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| "Session registry has no parent directory".to_owned())?;
        fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
        let data = serde_json::to_string_pretty(&self.records)
            .map_err(|error| format!("Could not encode agent sessions: {error}"))?;
        fs::write(&self.path, format!("{data}\n"))
            .map_err(|error| format!("Could not write {}: {error}", self.path.display()))
    }
}

fn new_conversation_id() -> String {
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
            directory.join(SESSION_REGISTRY_FILE),
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
}
