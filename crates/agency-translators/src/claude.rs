use agency_translator_api::{
    ClientId, ContentBlock, Conversation, ConversationEvent, ConversationUpdate, EventPayload,
    ExportResult, ImportResult, LiveEventTranslator, MessageRole, NativeArtifact, NativeEnvelope,
    SessionTranslator, TRANSLATOR_PROTOCOL_VERSION, TranslationError, TranslationReport,
    TranslatorDescriptor,
};
use serde_json::{Value, json};

const CLIENT: &str = "claude-code";

pub struct ClaudeTranslator;

impl LiveEventTranslator for ClaudeTranslator {
    fn translate_live(&self, value: &Value) -> Result<Vec<ConversationUpdate>, TranslationError> {
        if value.get("type").and_then(Value::as_str) != Some("stream_event") {
            return Ok(Vec::new());
        }
        let event = &value["event"];
        if let Some(delta) = event.pointer("/delta/text").and_then(Value::as_str) {
            return Ok(vec![ConversationUpdate::AppendText {
                event_id: event
                    .get("message_id")
                    .or_else(|| value.get("message_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("claude-live-assistant")
                    .to_owned(),
                source: ClientId::new(CLIENT),
                delta: delta.to_owned(),
                native: Some(native(value.clone())),
            }]);
        }
        if event.get("type").and_then(Value::as_str) == Some("content_block_start")
            && let Some(block) = event.get("content_block")
            && block.get("type").and_then(Value::as_str) == Some("tool_use")
        {
            let id = block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("claude-live-tool")
                .to_owned();
            return Ok(vec![ConversationUpdate::Append {
                event: ConversationEvent {
                    id: id.clone(),
                    parent_id: None,
                    turn_id: None,
                    source: ClientId::new(CLIENT),
                    payload: EventPayload::ToolCall {
                        id: Some(id),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_owned(),
                        input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                    },
                    native: Some(native(value.clone())),
                },
            }]);
        }
        Ok(Vec::new())
    }
}

impl SessionTranslator for ClaudeTranslator {
    fn descriptor(&self) -> TranslatorDescriptor {
        TranslatorDescriptor {
            client: ClientId::new(CLIENT),
            protocol_version: TRANSLATOR_PROTOCOL_VERSION,
            format_versions: vec!["jsonl".to_owned()],
            can_import: true,
            can_export: true,
        }
    }

    fn import(&self, artifact: &NativeArtifact) -> Result<ImportResult, TranslationError> {
        let NativeArtifact::JsonLines(source) = artifact else {
            return Err(TranslationError::new(
                "Claude translator expects a JSON Lines artifact",
            ));
        };
        let mut events = Vec::new();
        let mut report = TranslationReport::exact(0);
        for (index, line) in source.lines().enumerate() {
            let raw: Value = serde_json::from_str(line).map_err(|error| {
                TranslationError::new(format!(
                    "invalid Claude JSONL at line {}: {error}",
                    index + 1
                ))
            })?;
            if raw.get("isSidechain").and_then(Value::as_bool) == Some(true) {
                report.warn(
                    raw.get("uuid").and_then(Value::as_str).map(str::to_owned),
                    "sidechain event was omitted from the main conversation",
                );
                continue;
            }
            if let Some(event) = import_entry(raw.clone(), index) {
                events.push(event);
            }
            events.extend(import_tool_entries(&raw, index));
        }
        report.source_events = source.lines().count();
        report.translated_events = events.len();
        Ok(ImportResult {
            conversation: Conversation::new(events),
            report,
        })
    }

    fn export(&self, conversation: &Conversation) -> Result<ExportResult, TranslationError> {
        let mut lines = Vec::new();
        let mut report = TranslationReport::exact(conversation.events.len());
        for event in &conversation.events {
            if let Some(native) = &event.native
                && native.client.0 == CLIENT
            {
                lines.push(serde_json::to_string(&native.raw).map_err(|error| {
                    TranslationError::new(format!("could not encode Claude event: {error}"))
                })?);
                continue;
            }
            let Some(entry) = export_event(event, &mut report) else {
                continue;
            };
            lines.push(serde_json::to_string(&entry).map_err(|error| {
                TranslationError::new(format!("could not encode Claude event: {error}"))
            })?);
        }
        report.translated_events = lines.len();
        Ok(ExportResult {
            artifact: NativeArtifact::JsonLines(format!("{}\n", lines.join("\n"))),
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
        format_version: None,
        raw,
    }
}

fn import_entry(raw: Value, index: usize) -> Option<ConversationEvent> {
    let entry_type = raw.get("type").and_then(Value::as_str)?;
    let role = match entry_type {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        _ => return None,
    };
    let payload = EventPayload::Message {
        role,
        content: import_content(raw.pointer("/message/content")?),
    };
    Some(ConversationEvent {
        id: raw
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("claude-line-{}", index + 1)),
        parent_id: raw
            .get("parentUuid")
            .and_then(Value::as_str)
            .map(str::to_owned),
        turn_id: None,
        source: ClientId::new(CLIENT),
        payload,
        native: Some(NativeEnvelope {
            client: ClientId::new(CLIENT),
            format_version: None,
            raw,
        }),
    })
}

fn import_content(content: &Value) -> Vec<ContentBlock> {
    if let Some(text) = content.as_str() {
        return vec![ContentBlock::Text {
            text: text.to_owned(),
        }];
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => Some(ContentBlock::Text {
                text: block.get("text")?.as_str()?.to_owned(),
            }),
            Some("image") => Some(ContentBlock::Image {
                media_type: block
                    .pointer("/source/media_type")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                data: block
                    .pointer("/source/data")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }),
            _ => None,
        })
        .collect()
}

fn import_tool_entries(raw: &Value, index: usize) -> Vec<ConversationEvent> {
    raw.pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(block_index, block)| {
            let block_type = block.get("type").and_then(Value::as_str)?;
            let id = block
                .get("id")
                .or_else(|| block.get("tool_use_id"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let payload = match block_type {
                "tool_use" => EventPayload::ToolCall {
                    id: id.clone(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_owned(),
                    input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                },
                "tool_result" => EventPayload::ToolResult {
                    tool_call_id: id.clone(),
                    content: import_content(block.get("content").unwrap_or(&Value::Null)),
                    is_error: block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
                _ => return None,
            };
            Some(ConversationEvent {
                id: id.unwrap_or_else(|| {
                    format!("claude-line-{}-block-{}", index + 1, block_index + 1)
                }),
                parent_id: raw.get("uuid").and_then(Value::as_str).map(str::to_owned),
                turn_id: None,
                source: ClientId::new(CLIENT),
                payload,
                native: Some(native(raw.clone())),
            })
        })
        .collect()
}

fn export_event(event: &ConversationEvent, report: &mut TranslationReport) -> Option<Value> {
    let EventPayload::Message { role, content } = &event.payload else {
        report.warn(
            Some(event.id.clone()),
            "non-message event cannot yet be projected into Claude JSONL",
        );
        return None;
    };
    let entry_type = match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System | MessageRole::Developer => {
            report.warn(
                Some(event.id.clone()),
                "system or developer message was projected as a user-visible system record",
            );
            "system"
        }
    };
    Some(json!({
        "type": entry_type,
        "uuid": event.id,
        "parentUuid": event.parent_id,
        "isSidechain": false,
        "message": {
            "role": match role {
                MessageRole::Assistant => "assistant",
                _ => "user",
            },
            "content": export_content(content),
        }
    }))
}

fn export_content(content: &[ContentBlock]) -> Vec<Value> {
    content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
            ContentBlock::Image { media_type, data } => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type.as_deref().unwrap_or("application/octet-stream"),
                    "data": data,
                }
            }),
            ContentBlock::Attachment { kind, reference } => json!({
                "type": "text",
                "text": format!("[{kind} attachment: {reference}]"),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agency_translator_api::Fidelity;

    #[test]
    fn imports_mainline_and_preserves_native_records() {
        let artifact = NativeArtifact::JsonLines(
            r#"{"type":"user","uuid":"one","parentUuid":null,"message":{"role":"user","content":"Hello"}}
{"type":"assistant","uuid":"two","parentUuid":"one","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}]}}
{"type":"assistant","uuid":"side","isSidechain":true,"message":{"role":"assistant","content":"hidden"}}"#
                .to_owned(),
        );
        let result = ClaudeTranslator.import(&artifact).unwrap();
        assert_eq!(result.conversation.events.len(), 2);
        assert_eq!(result.report.fidelity, Fidelity::Equivalent);
        assert!(result.conversation.events[0].native.is_some());
    }

    #[test]
    fn native_round_trip_is_exact() {
        let artifact = NativeArtifact::JsonLines(
            "{\"type\":\"user\",\"uuid\":\"one\",\"message\":{\"role\":\"user\",\"content\":\"Hello\"}}\n"
                .to_owned(),
        );
        let imported = ClaudeTranslator.import(&artifact).unwrap();
        let exported = ClaudeTranslator.export(&imported.conversation).unwrap();
        assert_eq!(exported.report.fidelity, Fidelity::Exact);
        ClaudeTranslator.validate(&exported.artifact).unwrap();
    }

    #[test]
    fn translates_live_tool_start_for_canonical_consumers() {
        let updates = ClaudeTranslator
            .translate_live(&json!({
                "type": "stream_event",
                "event": {
                    "type": "content_block_start",
                    "content_block": {
                        "type": "tool_use",
                        "id": "tool-one",
                        "name": "Bash",
                        "input": {"command": "cargo test"}
                    }
                }
            }))
            .unwrap();
        assert!(matches!(
            &updates[0],
            ConversationUpdate::Append {
                event: ConversationEvent {
                    payload: EventPayload::ToolCall { name, .. },
                    ..
                }
            } if name == "Bash"
        ));
    }
}
