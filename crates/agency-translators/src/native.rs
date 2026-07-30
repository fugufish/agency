use crate::claude::ClaudeTranslator;
use agency_translator_api::{Conversation, NativeArtifact, SessionTranslator, TranslationError};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCompatibility {
    pub installed: bool,
    pub version: Option<String>,
    pub supported: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSession {
    pub id: String,
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
}

pub struct ClaudeNativeStore {
    projects: PathBuf,
}

impl ClaudeNativeStore {
    pub fn from_home(home: &Path) -> Self {
        Self {
            projects: home.join(".claude/projects"),
        }
    }

    pub fn discover(&self, cwd: &Path) -> Result<Vec<InstalledSession>, TranslationError> {
        let directory = self.project_directory(cwd);
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = fs::read_dir(&directory)
            .map_err(|error| io_error("read Claude project directory", &directory, error))?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let id = path.file_stem()?.to_str()?.to_owned();
                (path.extension().and_then(|value| value.to_str()) == Some("jsonl")).then_some(
                    InstalledSession {
                        id,
                        path,
                        backup: None,
                    },
                )
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }

    pub fn probe() -> ClientCompatibility {
        match Command::new("claude").arg("--version").output() {
            Ok(output) if output.status.success() => {
                let output = String::from_utf8_lossy(&output.stdout);
                let version = output
                    .split_whitespace()
                    .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))
                    .map(str::to_owned);
                let supported =
                    version.as_deref().and_then(|value| value.split('.').next()) == Some("2");
                ClientCompatibility {
                    installed: true,
                    supported,
                    detail: version.as_ref().map_or_else(
                        || "version could not be parsed".to_owned(),
                        |v| {
                            if supported {
                                format!("Claude Code {v}")
                            } else {
                                format!("unsupported Claude Code format version {v}")
                            }
                        },
                    ),
                    version,
                }
            }
            Ok(output) => ClientCompatibility {
                installed: true,
                version: None,
                supported: false,
                detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            },
            Err(error) => ClientCompatibility {
                installed: false,
                version: None,
                supported: false,
                detail: error.to_string(),
            },
        }
    }

    pub fn install_new(
        &self,
        cwd: &Path,
        conversation: &Conversation,
    ) -> Result<InstalledSession, TranslationError> {
        let compatibility = Self::probe();
        if !compatibility.supported {
            return Err(TranslationError::new(format!(
                "Claude Code is not compatible: {}",
                compatibility.detail
            )));
        }
        let directory = self.project_directory(cwd);
        fs::create_dir_all(&directory)
            .map_err(|error| io_error("create Claude project directory", &directory, error))?;
        let _lock = DirectoryLock::acquire(&directory)?;
        let session_id = native_uuid();
        let path = directory.join(format!("{session_id}.jsonl"));
        let artifact = ClaudeTranslator.export(conversation)?;
        let source = decorate_claude_jsonl(
            artifact.artifact,
            &session_id,
            cwd,
            compatibility.version.as_deref(),
        )?;
        ClaudeTranslator.validate(&NativeArtifact::JsonLines(source.clone()))?;
        let backup = atomic_write(&path, source.as_bytes())?;
        update_session_index(&directory, cwd, &session_id, &path, &source)?;
        Ok(InstalledSession {
            id: session_id,
            path,
            backup,
        })
    }

    fn project_directory(&self, cwd: &Path) -> PathBuf {
        let encoded = cwd
            .to_string_lossy()
            .chars()
            .map(|character| {
                if character == '/' || character == '\\' {
                    '-'
                } else {
                    character
                }
            })
            .collect::<String>();
        self.projects.join(encoded)
    }
}

struct DirectoryLock {
    path: PathBuf,
}

impl DirectoryLock {
    fn acquire(directory: &Path) -> Result<Self, TranslationError> {
        let path = directory.join(".agency-translation.lock");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| writeln!(file, "{}", std::process::id()))
            .map_err(|error| {
                TranslationError::new(format!(
                    "could not lock Claude session store {}: {error}",
                    directory.display()
                ))
            })?;
        Ok(Self { path })
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn decorate_claude_jsonl(
    artifact: NativeArtifact,
    session_id: &str,
    cwd: &Path,
    version: Option<&str>,
) -> Result<String, TranslationError> {
    let NativeArtifact::JsonLines(source) = artifact else {
        return Err(TranslationError::new(
            "Claude translator produced a non-JSONL artifact",
        ));
    };
    let mut ids = HashMap::new();
    let mut entries = Vec::new();
    let mut previous = None;
    for (index, line) in source.lines().enumerate() {
        let mut entry: Value = serde_json::from_str(line).map_err(|error| {
            TranslationError::new(format!("invalid translated Claude event: {error}"))
        })?;
        let canonical_id = entry
            .get("uuid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let id = native_uuid();
        ids.insert(canonical_id, id.clone());
        let parent = entry
            .get("parentUuid")
            .and_then(Value::as_str)
            .and_then(|id| ids.get(id))
            .cloned()
            .or_else(|| previous.clone());
        let entry_type = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let object = entry
            .as_object_mut()
            .ok_or_else(|| TranslationError::new("Claude JSONL entry is not an object"))?;
        object.insert("uuid".to_owned(), Value::String(id.clone()));
        object.insert(
            "parentUuid".to_owned(),
            parent.map_or(Value::Null, Value::String),
        );
        object.insert("sessionId".to_owned(), Value::String(session_id.to_owned()));
        object.insert(
            "cwd".to_owned(),
            Value::String(cwd.to_string_lossy().into_owned()),
        );
        object.insert("isSidechain".to_owned(), Value::Bool(false));
        object.insert("userType".to_owned(), Value::String("external".to_owned()));
        object.insert("entrypoint".to_owned(), Value::String("sdk-cli".to_owned()));
        object.insert("gitBranch".to_owned(), Value::String("HEAD".to_owned()));
        object.insert("timestamp".to_owned(), Value::String(rfc3339_now()));
        if entry_type == "user" {
            object.insert(
                "permissionMode".to_owned(),
                Value::String("auto".to_owned()),
            );
            object.insert("promptId".to_owned(), Value::String(native_uuid()));
            object.insert("promptSource".to_owned(), Value::String("sdk".to_owned()));
        }
        if let Some(version) = version {
            object.insert("version".to_owned(), Value::String(version.to_owned()));
        }
        if entry_type == "assistant"
            && let Some(message) = object.get_mut("message").and_then(Value::as_object_mut)
        {
            message.insert(
                "id".to_owned(),
                Value::String(format!("msg_agency_{}", id.replace('-', ""))),
            );
            message.insert("type".to_owned(), Value::String("message".to_owned()));
            message.insert(
                "model".to_owned(),
                Value::String("claude-imported".to_owned()),
            );
            message.insert("stop_reason".to_owned(), Value::Null);
            message.insert("stop_sequence".to_owned(), Value::Null);
            message.insert("stop_details".to_owned(), Value::Null);
            message.insert(
                "usage".to_owned(),
                json!({
                    "input_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                    "output_tokens": 0,
                    "server_tool_use": {
                        "web_search_requests": 0,
                        "web_fetch_requests": 0
                    },
                    "service_tier": "standard"
                }),
            );
            object.insert("effort".to_owned(), Value::String("high".to_owned()));
            object.insert(
                "requestId".to_owned(),
                Value::String(format!("req_agency_{}", id.replace('-', ""))),
            );
            object.insert("session_id".to_owned(), Value::Null);
        }
        entries.push(serde_json::to_string(&entry).map_err(|error| {
            TranslationError::new(format!(
                "could not encode Claude entry {}: {error}",
                index + 1
            ))
        })?);
        previous = Some(id);
    }
    if let Some(leaf_uuid) = previous {
        entries.push(
            serde_json::to_string(&json!({
                "type": "last-prompt",
                "sessionId": session_id,
                "leafUuid": leaf_uuid,
            }))
            .map_err(|error| TranslationError::new(error.to_string()))?,
        );
    }
    Ok(format!("{}\n", entries.join("\n")))
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<Option<PathBuf>, TranslationError> {
    let backup = if path.exists() {
        let backup = path.with_extension(format!("jsonl.agency-backup-{}", timestamp()));
        fs::copy(path, &backup).map_err(|error| io_error("back up native session", path, error))?;
        Some(backup)
    } else {
        None
    };
    let temporary = path.with_extension(format!("jsonl.agency-tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| io_error("create temporary native session", &temporary, error))?;
        file.write_all(data)
            .and_then(|_| file.sync_all())
            .map_err(|error| io_error("write temporary native session", &temporary, error))?;
        fs::rename(&temporary, path)
            .map_err(|error| io_error("install native session", path, error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|_| backup)
}

fn update_session_index(
    directory: &Path,
    cwd: &Path,
    session_id: &str,
    session_path: &Path,
    source: &str,
) -> Result<(), TranslationError> {
    let path = directory.join("sessions-index.json");
    let mut index = if path.exists() {
        serde_json::from_str::<Value>(
            &fs::read_to_string(&path)
                .map_err(|error| io_error("read Claude session index", &path, error))?,
        )
        .map_err(|error| {
            TranslationError::new(format!(
                "could not parse Claude session index {}: {error}",
                path.display()
            ))
        })?
    } else {
        json!({
            "version": 1,
            "originalPath": cwd,
            "entries": [],
        })
    };
    let entries = index
        .get_mut("entries")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| TranslationError::new("Claude session index has no entries array"))?;
    entries.retain(|entry| entry.get("sessionId").and_then(Value::as_str) != Some(session_id));
    let first_prompt = source
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|entry| entry.get("type").and_then(Value::as_str) == Some("user"))
        .and_then(|entry| entry.pointer("/message/content").cloned())
        .map(|content| {
            if let Some(text) = content.as_str() {
                text.to_owned()
            } else {
                content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        })
        .unwrap_or_default();
    entries.push(json!({
        "sessionId": session_id,
        "fullPath": session_path,
        "fileMtime": timestamp(),
        "firstPrompt": first_prompt,
        "summary": first_prompt,
        "messageCount": source.lines().filter(|line| {
            serde_json::from_str::<Value>(line).ok().is_some_and(|entry| {
                matches!(
                    entry.get("type").and_then(Value::as_str),
                    Some("user" | "assistant")
                )
            })
        }).count(),
        "gitBranch": "HEAD",
        "projectPath": cwd,
        "isSidechain": false,
    }));
    let data = serde_json::to_vec_pretty(&index).map_err(|error| {
        TranslationError::new(format!("could not encode session index: {error}"))
    })?;
    atomic_write(&path, &data)?;
    Ok(())
}

fn native_uuid() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed) as u128;
    let value = time ^ sequence.rotate_left(37) ^ (std::process::id() as u128).rotate_left(71);
    let hex = format!("{value:032x}");
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn rfc3339_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let days = seconds / 86_400;
    let day_seconds = seconds % 86_400;
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;

    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis()
    )
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> TranslationError {
    TranslationError::new(format!("{action} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agency_translator_api::{
        ClientId, ContentBlock, ConversationEvent, EventPayload, MessageRole,
    };

    #[test]
    fn decorates_translated_events_with_native_session_metadata() {
        let conversation = Conversation::new(vec![
            ConversationEvent {
                id: "canonical-one".to_owned(),
                parent_id: None,
                turn_id: None,
                source: ClientId::new("codex"),
                payload: EventPayload::Message {
                    role: MessageRole::User,
                    content: vec![ContentBlock::Text {
                        text: "Hello".to_owned(),
                    }],
                },
                native: None,
            },
            ConversationEvent {
                id: "canonical-two".to_owned(),
                parent_id: Some("canonical-one".to_owned()),
                turn_id: None,
                source: ClientId::new("codex"),
                payload: EventPayload::Message {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "Hi".to_owned(),
                    }],
                },
                native: None,
            },
        ]);
        let artifact = ClaudeTranslator.export(&conversation).unwrap().artifact;
        let source = decorate_claude_jsonl(
            artifact,
            "session-id",
            Path::new("/tmp/project"),
            Some("2.1.0"),
        )
        .unwrap();
        let entries = source
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(entries[0]["sessionId"], "session-id");
        assert_eq!(entries[0]["cwd"], "/tmp/project");
        assert_eq!(entries[0]["promptSource"], "sdk");
        assert_eq!(entries[1]["message"]["type"], "message");
        assert!(
            entries[1]["message"]["id"]
                .as_str()
                .unwrap()
                .starts_with("msg_agency_")
        );
        assert_eq!(entries[2]["type"], "last-prompt");
    }

    #[test]
    fn discovery_is_scoped_to_the_encoded_project_directory() {
        let root = std::env::temp_dir().join(format!("agency-native-store-{}", native_uuid()));
        let store = ClaudeNativeStore {
            projects: root.clone(),
        };
        let project = store.project_directory(Path::new("/work/tree"));
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("one.jsonl"), "{}\n").unwrap();
        fs::write(project.join("ignored.txt"), "").unwrap();
        let sessions = store.discover(Path::new("/work/tree")).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "one");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacing_a_native_file_creates_a_recoverable_backup() {
        let root = std::env::temp_dir().join(format!("agency-native-write-{}", native_uuid()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        fs::write(&path, b"old\n").unwrap();
        let backup = atomic_write(&path, b"new\n").unwrap().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "old\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn indexes_installed_sessions_for_claude_resume_discovery() {
        let root = std::env::temp_dir().join(format!("agency-native-index-{}", native_uuid()));
        fs::create_dir_all(&root).unwrap();
        let session_path = root.join("session-one.jsonl");
        let source = r#"{"type":"user","message":{"role":"user","content":"Hello"}}"#;
        fs::write(&session_path, source).unwrap();
        update_session_index(
            &root,
            Path::new("/work/project"),
            "session-one",
            &session_path,
            source,
        )
        .unwrap();
        let index: Value =
            serde_json::from_str(&fs::read_to_string(root.join("sessions-index.json")).unwrap())
                .unwrap();
        assert_eq!(index["entries"][0]["sessionId"], "session-one");
        assert_eq!(index["entries"][0]["firstPrompt"], "Hello");
        assert_eq!(index["entries"][0]["messageCount"], 1);
        fs::remove_dir_all(root).unwrap();
    }
}
