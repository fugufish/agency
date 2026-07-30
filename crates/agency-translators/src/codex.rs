use agency_translator_api::{
    ClientId, ContentBlock, Conversation, ConversationEvent, ConversationUpdate, EventPayload,
    ExportResult, ImportResult, LiveEventTranslator, MessageRole, NativeArtifact, NativeEnvelope,
    SessionTranslator, TRANSLATOR_PROTOCOL_VERSION, TranslationError, TranslationReport,
    TranslatorDescriptor,
};
use serde_json::{Value, json};

const CLIENT: &str = "codex";

pub struct CodexTranslator;

impl LiveEventTranslator for CodexTranslator {
    fn translate_live(&self, event: &Value) -> Result<Vec<ConversationUpdate>, TranslationError> {
        let Some(method) = event.get("method").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        match method {
            "item/agentMessage/delta" => {
                let Some(delta) = event.pointer("/params/delta").and_then(Value::as_str) else {
                    return Ok(Vec::new());
                };
                Ok(vec![ConversationUpdate::AppendText {
                    event_id: event
                        .pointer("/params/itemId")
                        .and_then(Value::as_str)
                        .unwrap_or("codex-live-assistant")
                        .to_owned(),
                    source: ClientId::new(CLIENT),
                    delta: delta.to_owned(),
                    native: Some(native(event.clone())),
                }])
            }
            "item/started" | "item/completed" => {
                let Some(item) = event.pointer("/params/item") else {
                    return Ok(Vec::new());
                };
                let kind = item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("activity");
                if matches!(kind, "agentMessage" | "userMessage") {
                    return Ok(Vec::new());
                }
                if kind == "fileChange" && method == "item/started" {
                    return Ok(Vec::new());
                }
                if kind != "fileChange" && method == "item/completed" {
                    return Ok(Vec::new());
                }
                let name = item
                    .get("name")
                    .or_else(|| item.get("command"))
                    .and_then(Value::as_str)
                    .unwrap_or(kind)
                    .to_owned();
                Ok(vec![ConversationUpdate::Append {
                    event: ConversationEvent {
                        id: item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("codex-live-tool")
                            .to_owned(),
                        parent_id: None,
                        turn_id: event
                            .pointer("/params/turnId")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        source: ClientId::new(CLIENT),
                        payload: EventPayload::ToolCall {
                            id: item.get("id").and_then(Value::as_str).map(str::to_owned),
                            name,
                            input: item.clone(),
                        },
                        native: Some(native(event.clone())),
                    },
                }])
            }
            _ => Ok(Vec::new()),
        }
    }
}

impl SessionTranslator for CodexTranslator {
    fn descriptor(&self) -> TranslatorDescriptor {
        TranslatorDescriptor {
            client: ClientId::new(CLIENT),
            protocol_version: TRANSLATOR_PROTOCOL_VERSION,
            format_versions: vec!["app-server-v2".to_owned()],
            can_import: true,
            can_export: true,
        }
    }

    fn import(&self, artifact: &NativeArtifact) -> Result<ImportResult, TranslationError> {
        let NativeArtifact::Json(value) = artifact else {
            return Err(TranslationError::new(
                "Codex translator expects a JSON artifact",
            ));
        };
        let items: Vec<&Value> = if let Some(turns) = value
            .pointer("/result/thread/turns")
            .and_then(Value::as_array)
        {
            turns
                .iter()
                .flat_map(|turn| {
                    turn.get("items")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .collect()
        } else if let Some(items) = value.as_array() {
            items.iter().collect()
        } else {
            return Err(TranslationError::new(
                "Codex artifact is neither a thread response nor a history array",
            ));
        };
        let mut events = Vec::new();
        let mut report = TranslationReport::exact(items.len());
        for (index, item) in items.into_iter().enumerate() {
            if let Some(event) = import_item(item.clone(), index) {
                events.push(event);
            } else {
                report.warn(
                    None,
                    "unsupported Codex history item was retained only natively",
                );
            }
        }
        report.translated_events = events.len();
        Ok(ImportResult {
            conversation: Conversation::new(events),
            report,
        })
    }

    fn export(&self, conversation: &Conversation) -> Result<ExportResult, TranslationError> {
        let mut items = Vec::new();
        let mut report = TranslationReport::exact(conversation.events.len());
        for event in &conversation.events {
            if let Some(native) = &event.native
                && native.client.0 == CLIENT
            {
                items.push(native.raw.clone());
            } else if let Some(item) = export_event(event, &mut report) {
                items.push(item);
            }
        }
        report.translated_events = items.len();
        Ok(ExportResult {
            artifact: NativeArtifact::Json(Value::Array(items)),
            report,
        })
    }

    fn validate(&self, artifact: &NativeArtifact) -> Result<(), TranslationError> {
        self.import(artifact).map(|_| ())
    }
}

fn native(raw: Value) -> NativeEnvelope {
    NativeEnvelope {
        client: ClientId::new(CLIENT),
        format_version: Some("app-server-v2".to_owned()),
        raw,
    }
}

fn import_item(raw: Value, index: usize) -> Option<ConversationEvent> {
    let (role, content) = match raw.get("type").and_then(Value::as_str)? {
        "userMessage" => (
            MessageRole::User,
            raw.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(import_content)
                .collect(),
        ),
        "agentMessage" => (
            MessageRole::Assistant,
            vec![ContentBlock::Text {
                text: raw
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }],
        ),
        "message" => {
            let role = match raw.get("role").and_then(Value::as_str)? {
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                "developer" => MessageRole::Developer,
                _ => return None,
            };
            (
                role,
                raw.get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(import_content)
                    .collect(),
            )
        }
        "commandExecution" | "fileChange" => {
            let id = raw.get("id").and_then(Value::as_str).map(str::to_owned);
            return Some(ConversationEvent {
                id: id
                    .clone()
                    .unwrap_or_else(|| format!("codex-item-{}", index + 1)),
                parent_id: None,
                turn_id: None,
                source: ClientId::new(CLIENT),
                payload: EventPayload::ToolCall {
                    id,
                    name: raw
                        .get("command")
                        .or_else(|| raw.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            raw.get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("activity")
                        })
                        .to_owned(),
                    input: raw.clone(),
                },
                native: Some(native(raw)),
            });
        }
        _ => return None,
    };
    Some(ConversationEvent {
        id: raw
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("codex-item-{}", index + 1)),
        parent_id: None,
        turn_id: None,
        source: ClientId::new(CLIENT),
        payload: EventPayload::Message { role, content },
        native: Some(NativeEnvelope {
            client: ClientId::new(CLIENT),
            format_version: Some("app-server-v2".to_owned()),
            raw,
        }),
    })
}

fn import_content(value: &Value) -> Option<ContentBlock> {
    match value.get("type").and_then(Value::as_str) {
        Some("text" | "input_text" | "output_text") => Some(ContentBlock::Text {
            text: value.get("text")?.as_str()?.to_owned(),
        }),
        Some("image" | "input_image") => Some(ContentBlock::Image {
            media_type: None,
            data: value
                .get("url")
                .or_else(|| value.get("image_url"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        _ => None,
    }
}

fn export_event(event: &ConversationEvent, report: &mut TranslationReport) -> Option<Value> {
    let EventPayload::Message { role, content } = &event.payload else {
        report.warn(
            Some(event.id.clone()),
            "non-message event cannot yet be projected into Codex history",
        );
        return None;
    };
    match role {
        MessageRole::User => Some(json!({
            "type": "message",
            "role": "user",
            "content": content.iter().map(|block| export_content(block, false)).collect::<Vec<_>>(),
        })),
        MessageRole::Assistant => Some(json!({
            "type": "message",
            "role": "assistant",
            "content": content.iter().map(|block| export_content(block, true)).collect::<Vec<_>>(),
        })),
        MessageRole::System | MessageRole::Developer => {
            report.warn(
                Some(event.id.clone()),
                "system or developer message was converted to a model-visible history message",
            );
            Some(json!({
                "type": "message",
                "role": "developer",
                "content": content.iter().map(|block| export_content(block, false)).collect::<Vec<_>>(),
            }))
        }
    }
}

fn export_content(block: &ContentBlock, assistant: bool) -> Value {
    match block {
        ContentBlock::Text { text } => json!({
            "type": if assistant { "output_text" } else { "input_text" },
            "text": text,
        }),
        ContentBlock::Image { data, .. } => json!({ "type": "input_image", "image_url": data }),
        ContentBlock::Attachment { kind, reference } => {
            json!({ "type": "input_text", "text": format!("[{kind} attachment: {reference}]") })
        }
    }
}

#[cfg(test)]
mod file_change_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn waits_for_completed_file_changes() {
        let started = json!({
            "method": "item/started",
            "params": {
                "turnId": "turn-1",
                "item": {
                    "id": "change-1",
                    "type": "fileChange",
                    "status": "inProgress",
                    "changes": [{
                        "path": "src/lib.rs",
                        "kind": "update",
                        "diff": "@@ -1 +1 @@\n-old\n+new\n"
                    }]
                }
            }
        });
        assert!(CodexTranslator.translate_live(&started).unwrap().is_empty());

        let completed = json!({
            "method": "item/completed",
            "params": {
                "turnId": "turn-1",
                "item": {
                    "id": "change-1",
                    "type": "fileChange",
                    "status": "completed",
                    "changes": [{
                        "path": "src/lib.rs",
                        "kind": "update",
                        "diff": "@@ -1 +1 @@\n-old\n+new\n"
                    }]
                }
            }
        });
        let updates = CodexTranslator.translate_live(&completed).unwrap();
        assert_eq!(updates.len(), 1);
        let ConversationUpdate::Append { event } = &updates[0] else {
            panic!("file changes should append a completed tool event");
        };
        assert_eq!(event.id, "change-1");
        let EventPayload::ToolCall { input, .. } = &event.payload else {
            panic!("file changes should remain normalized tool calls");
        };
        assert_eq!(input["status"], "completed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agency_translator_api::Fidelity;

    #[test]
    fn imports_app_server_thread_response() {
        let artifact = NativeArtifact::Json(json!({
            "result": { "thread": { "turns": [{ "items": [
                {"type":"userMessage","content":[{"type":"text","text":"Fix it"}]},
                {"type":"agentMessage","text":"Working on it"},
                {"type":"commandExecution","command":"cargo test"}
            ]}]}}
        }));
        let result = CodexTranslator.import(&artifact).unwrap();
        assert_eq!(result.conversation.events.len(), 3);
        assert_eq!(result.report.fidelity, Fidelity::Exact);
    }

    #[test]
    fn exports_foreign_messages_as_responses_history() {
        let conversation = Conversation::new(vec![ConversationEvent {
            id: "one".to_owned(),
            parent_id: None,
            turn_id: None,
            source: ClientId::new("claude-code"),
            payload: EventPayload::Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "Hello".to_owned(),
                }],
            },
            native: None,
        }]);
        let exported = CodexTranslator.export(&conversation).unwrap();
        assert!(matches!(
            exported.artifact,
            NativeArtifact::Json(Value::Array(_))
        ));
    }

    #[test]
    fn translates_live_delta_for_canonical_consumers() {
        let updates = CodexTranslator
            .translate_live(&json!({
                "method": "item/agentMessage/delta",
                "params": {"itemId": "message-one", "delta": "Hello"}
            }))
            .unwrap();
        assert!(matches!(
            &updates[0],
            ConversationUpdate::AppendText {
                event_id,
                delta,
                ..
            } if event_id == "message-one" && delta == "Hello"
        ));
    }
}
