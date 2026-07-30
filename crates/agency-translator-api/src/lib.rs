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
    Append {
        event: ConversationEvent,
    },
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

pub trait SessionTranslator: Send + Sync {
    fn descriptor(&self) -> TranslatorDescriptor;
    fn import(&self, artifact: &NativeArtifact) -> Result<ImportResult, TranslationError>;
    fn export(&self, conversation: &Conversation) -> Result<ExportResult, TranslationError>;
    fn validate(&self, artifact: &NativeArtifact) -> Result<(), TranslationError>;
}

pub trait LiveEventTranslator: Send + Sync {
    fn translate_live(&self, event: &Value) -> Result<Vec<ConversationUpdate>, TranslationError>;
}
