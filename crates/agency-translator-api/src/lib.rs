use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

pub const TRANSLATOR_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(pub String);

impl ClientId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub events: Vec<ConversationEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConversationUpdate {
    /// Adds an event, or replaces the event that already carries the same id.
    /// Providers report long-running work twice — once when it starts and once
    /// when it finishes — and the finished report replaces the started one in
    /// place instead of repeating it.
    Append { event: ConversationEvent },
    AppendText {
        event_id: String,
        source: ClientId,
        delta: String,
        native: Option<NativeEnvelope>,
    },
}

impl Conversation {
    pub fn new(events: Vec<ConversationEvent>) -> Self {
        Self { events }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationEvent {
    pub id: String,
    pub parent_id: Option<String>,
    pub turn_id: Option<String>,
    pub source: ClientId,
    pub payload: EventPayload,
    pub native: Option<NativeEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum EventPayload {
    Message {
        role: MessageRole,
        content: Vec<ContentBlock>,
    },
    /// A tool invocation. `input` uses the canonical [`tools`] vocabulary when
    /// the call is one every provider performs, so transcripts render the same
    /// work identically no matter which agent did it.
    ToolCall {
        id: Option<String>,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_call_id: Option<String>,
        content: Vec<ContentBlock>,
        is_error: bool,
    },
    Summary {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Developer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: Option<String>,
        data: String,
    },
    Attachment {
        kind: String,
        reference: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeEnvelope {
    pub client: ClientId,
    pub format_version: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NativeArtifact {
    Json(Value),
    JsonLines(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fidelity {
    Exact,
    Equivalent,
    Summarized,
    Lossy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationWarning {
    pub event_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationReport {
    pub fidelity: Fidelity,
    pub source_events: usize,
    pub translated_events: usize,
    pub warnings: Vec<TranslationWarning>,
}

impl TranslationReport {
    pub fn exact(events: usize) -> Self {
        Self {
            fidelity: Fidelity::Exact,
            source_events: events,
            translated_events: events,
            warnings: Vec::new(),
        }
    }

    pub fn warn(&mut self, event_id: Option<String>, message: impl Into<String>) {
        self.warnings.push(TranslationWarning {
            event_id,
            message: message.into(),
        });
        if self.fidelity == Fidelity::Exact {
            self.fidelity = Fidelity::Equivalent;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportResult {
    pub conversation: Conversation,
    pub report: TranslationReport,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportResult {
    pub artifact: NativeArtifact,
    pub report: TranslationReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslatorDescriptor {
    pub client: ClientId,
    pub protocol_version: u32,
    pub format_versions: Vec<String>,
    pub can_import: bool,
    pub can_export: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationError {
    pub message: String,
}

impl TranslationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TranslationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for TranslationError {}

/// The canonical, provider-neutral vocabulary for tool calls.
///
/// Agents describe the same work in different ways: Codex reports app-server
/// items, Claude Code reports `tool_use` blocks completed by `tool_result`
/// payloads. Each translator normalizes the work every agent does — running a
/// command, changing a file, reading a file — into these shapes, so a single
/// set of transcript handlers renders both agents identically. Provider
/// specific tool calls keep their own input and render generically.
pub mod tools {
    use serde_json::{Value, json};

    /// A command run in the workspace shell.
    /// `{"type":"commandExecution","command":..,"status":..,"aggregatedOutput":..,"exitCode":..}`
    pub const COMMAND_EXECUTION: &str = "commandExecution";
    /// One or more files written, created, or deleted.
    /// `{"type":"fileChange","status":..,"changes":[{"path":..,"kind":..,"diff":..}]}`
    pub const FILE_CHANGE: &str = "fileChange";
    /// A file read into the agent's context.
    /// `{"type":"fileRead","status":..,"path":..,"lines":..}`
    pub const FILE_READ: &str = "fileRead";

    pub const IN_PROGRESS: &str = "inProgress";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";

    /// The canonical kind of a tool call input, if it has one.
    pub fn kind(input: &Value) -> Option<&str> {
        input.get("type").and_then(Value::as_str)
    }

    /// The reported status, defaulting to completed for inputs that omit it.
    pub fn status(input: &Value) -> &str {
        input
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or(COMPLETED)
    }

    pub fn is_finished(input: &Value) -> bool {
        status(input) != IN_PROGRESS
    }

    pub fn command_execution(
        command: &str,
        status: &str,
        output: Option<&str>,
        exit_code: Option<i64>,
    ) -> Value {
        json!({
            "type": COMMAND_EXECUTION,
            "command": command,
            "status": status,
            "aggregatedOutput": output,
            "exitCode": exit_code,
        })
    }

    pub fn file_read(path: &str, status: &str, lines: Option<u64>) -> Value {
        json!({
            "type": FILE_READ,
            "path": path,
            "status": status,
            "lines": lines,
        })
    }

    pub fn file_change(status: &str, changes: Vec<Value>) -> Value {
        json!({
            "type": FILE_CHANGE,
            "status": status,
            "changes": changes,
        })
    }

    /// One changed file. `diff` holds unified diff hunks without file headers,
    /// which the transcript adds from `path` when it renders the change.
    pub fn change(path: &str, kind: &str, diff: &str) -> Value {
        json!({ "path": path, "kind": kind, "diff": diff })
    }

    /// The kind of a change. Codex reports `{"type":"update"}` while Claude
    /// Code and imported history report a plain `"update"`.
    pub fn change_kind(change: &Value) -> &str {
        let kind = change.get("kind");
        kind.and_then(Value::as_str)
            .or_else(|| {
                kind.and_then(|kind| kind.get("type"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("update")
    }

    /// A unified diff hunk built from prefixed lines (` `, `-`, `+`).
    pub fn hunk(
        old_start: u64,
        old_lines: u64,
        new_start: u64,
        new_lines: u64,
        body: &str,
    ) -> String {
        let body = body.strip_suffix('\n').unwrap_or(body);
        format!("@@ -{old_start},{old_lines} +{new_start},{new_lines} @@\n{body}\n")
    }

    /// The hunk for a file that is created whole.
    pub fn added_file_hunk(content: &str) -> String {
        let lines = content.strip_suffix('\n').unwrap_or(content);
        let body = lines
            .split('\n')
            .map(|line| format!("+{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        hunk(0, 0, 1, lines.split('\n').count() as u64, &body)
    }
}

pub trait SessionTranslator: Send + Sync {
    fn descriptor(&self) -> TranslatorDescriptor;
    fn import(&self, artifact: &NativeArtifact) -> Result<ImportResult, TranslationError>;
    fn export(&self, conversation: &Conversation) -> Result<ExportResult, TranslationError>;
    fn validate(&self, artifact: &NativeArtifact) -> Result<(), TranslationError>;
}

pub trait LiveEventTranslator: Send + Sync {
    fn translate_live(&self, event: &Value) -> Result<Vec<ConversationUpdate>, TranslationError>;
}

#[cfg(test)]
mod tests {
    use super::tools;
    use serde_json::json;

    #[test]
    fn change_kinds_read_both_provider_spellings() {
        assert_eq!(tools::change_kind(&json!({"kind": "add"})), "add");
        assert_eq!(
            tools::change_kind(&json!({"kind": {"type": "delete", "move_path": null}})),
            "delete"
        );
        assert_eq!(tools::change_kind(&json!({})), "update");
    }

    #[test]
    fn hunks_are_rendered_the_way_both_agents_report_them() {
        assert_eq!(
            tools::hunk(1, 3, 1, 3, " alpha\n-beta\n+BETA\n gamma"),
            "@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n"
        );
        assert_eq!(tools::added_file_hunk("done\n"), "@@ -0,0 +1,1 @@\n+done\n");
    }

    #[test]
    fn statuses_default_to_completed_but_report_progress() {
        let running = tools::command_execution("cargo test", tools::IN_PROGRESS, None, None);
        assert_eq!(tools::kind(&running), Some(tools::COMMAND_EXECUTION));
        assert!(!tools::is_finished(&running));
        assert!(tools::is_finished(&json!({"type": "fileChange"})));
    }
}
