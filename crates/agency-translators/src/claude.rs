use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use agency_translator_api::{
    ClientId, ContentBlock, Conversation, ConversationEvent, ConversationUpdate, EventPayload,
    ExportResult, ImportResult, LiveEventTranslator, MessageRole, NativeArtifact, NativeEnvelope,
    SessionTranslator, TRANSLATOR_PROTOCOL_VERSION, TranslationError, TranslationReport,
    TranslatorDescriptor, tools,
};
use serde_json::{Value, json};

const CLIENT: &str = "claude-code";
const LIVE_ASSISTANT: &str = "claude-live-assistant";

/// Claude Code reports a turn in pieces: text arrives as deltas, a tool's
/// arguments arrive complete in the assistant message that follows them, and a
/// tool's outcome arrives in a later user message. The translator remembers the
/// pieces of the turn in flight so it can report each tool call once, in the
/// canonical [`tools`] vocabulary, and complete it in place when it finishes.
#[derive(Default)]
pub struct ClaudeTranslator {
    live: Mutex<LiveState>,
}

#[derive(Default)]
struct LiveState {
    message_id: Option<String>,
    streamed_messages: HashSet<String>,
    pending_tools: HashMap<String, PendingTool>,
}

struct PendingTool {
    name: String,
    input: Value,
}

impl LiveEventTranslator for ClaudeTranslator {
    fn translate_live(&self, value: &Value) -> Result<Vec<ConversationUpdate>, TranslationError> {
        // Work a subagent does belongs to its own conversation, the same way
        // imported sidechains stay out of the main history.
        if value
            .get("parent_tool_use_id")
            .and_then(Value::as_str)
            .is_some()
        {
            return Ok(Vec::new());
        }
        match value.get("type").and_then(Value::as_str) {
            Some("stream_event") => Ok(self.translate_stream_event(value)),
            Some("assistant") => Ok(self.translate_assistant(value)),
            Some("user") => Ok(self.translate_tool_results(value)),
            Some("result") => {
                // The turn is over: every tool it started has been answered and
                // its message ids will not be seen again.
                *self.state() = LiveState::default();
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }
}

impl ClaudeTranslator {
    fn state(&self) -> std::sync::MutexGuard<'_, LiveState> {
        self.live.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Streams assistant text. Tool arguments also stream, as partial JSON, but
    /// only the assistant message that follows them can be read, so tool calls
    /// wait for it.
    fn translate_stream_event(&self, value: &Value) -> Vec<ConversationUpdate> {
        let event = &value["event"];
        if let Some(id) = event.pointer("/message/id").and_then(Value::as_str) {
            self.state().message_id = Some(id.to_owned());
        }
        let Some(delta) = event.pointer("/delta/text").and_then(Value::as_str) else {
            return Vec::new();
        };
        let block = event.get("index").and_then(Value::as_u64).unwrap_or(0);
        let event_id = {
            let mut state = self.state();
            match state.message_id.clone() {
                Some(message) => {
                    state.streamed_messages.insert(message.clone());
                    format!("{message}-{block}")
                }
                None => LIVE_ASSISTANT.to_owned(),
            }
        };
        vec![ConversationUpdate::AppendText {
            event_id,
            source: ClientId::new(CLIENT),
            delta: delta.to_owned(),
            native: Some(native(value.clone())),
        }]
    }

    /// Reports the tool calls in a completed assistant message, where their
    /// arguments are whole, and any text that was not streamed.
    fn translate_assistant(&self, value: &Value) -> Vec<ConversationUpdate> {
        let message_id = value
            .pointer("/message/id")
            .and_then(Value::as_str)
            .unwrap_or(LIVE_ASSISTANT);
        let streamed = self.state().streamed_messages.contains(message_id);
        let mut updates = Vec::new();

        for (index, block) in value
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            match block.get("type").and_then(Value::as_str) {
                Some("text") if !streamed => {
                    let Some(text) = block.get("text").and_then(Value::as_str) else {
                        continue;
                    };
                    updates.push(ConversationUpdate::Append {
                        event: ConversationEvent {
                            id: format!("{message_id}-{index}"),
                            parent_id: None,
                            turn_id: None,
                            source: ClientId::new(CLIENT),
                            payload: EventPayload::Message {
                                role: MessageRole::Assistant,
                                content: vec![ContentBlock::Text {
                                    text: text.to_owned(),
                                }],
                            },
                            native: Some(native(value.clone())),
                        },
                    });
                }
                Some("tool_use") => {
                    let Some(id) = block.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_owned();
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    let started = started_tool_call(&name, &input);
                    self.state().pending_tools.insert(
                        id.to_owned(),
                        PendingTool {
                            name: name.clone(),
                            input,
                        },
                    );
                    if let Some((name, input)) = started {
                        updates.push(ConversationUpdate::Append {
                            event: tool_event(id, name, input, native(value.clone())),
                        });
                    }
                }
                _ => {}
            }
        }

        updates
    }

    /// Completes the tool calls whose results have arrived. A result carries
    /// what the tool actually did — a command's output, a file's patch — so it
    /// replaces the started call instead of appearing beside it.
    fn translate_tool_results(&self, value: &Value) -> Vec<ConversationUpdate> {
        value
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            .filter_map(|block| {
                let id = block.get("tool_use_id").and_then(Value::as_str)?;
                let pending = self.state().pending_tools.remove(id)?;
                let (name, input) = finished_tool_call(
                    &pending.name,
                    &pending.input,
                    tool_use_result(value),
                    block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )?;
                Some(ConversationUpdate::Append {
                    event: tool_event(id, name, input, native(value.clone())),
                })
            })
            .collect()
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
        let mut pending = HashMap::new();
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
            import_tool_entries(&raw, &mut events, &mut pending);
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

fn tool_event(id: &str, name: String, input: Value, native: NativeEnvelope) -> ConversationEvent {
    ConversationEvent {
        id: id.to_owned(),
        parent_id: None,
        turn_id: None,
        source: ClientId::new(CLIENT),
        payload: EventPayload::ToolCall {
            id: Some(id.to_owned()),
            name,
            input,
        },
        native: Some(native),
    }
}

/// The work every agent does, whichever tool Claude Code used to do it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Work {
    Command,
    Read,
    Change,
}

fn work(tool: &str) -> Option<Work> {
    match tool {
        "Bash" => Some(Work::Command),
        "Read" | "NotebookRead" => Some(Work::Read),
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => Some(Work::Change),
        _ => None,
    }
}

fn tool_use_result(value: &Value) -> Option<&Value> {
    value
        .get("tool_use_result")
        .or_else(|| value.get("toolUseResult"))
        .filter(|result| !result.is_null())
}

/// What a starting tool call should show. File changes are withheld until they
/// finish, because only the result reports the patch that was applied — the
/// same rule Codex follows for its own file changes.
fn started_tool_call(tool: &str, input: &Value) -> Option<(String, Value)> {
    match work(tool) {
        Some(Work::Command) => {
            let command = input.get("command").and_then(Value::as_str)?;
            Some((
                command.to_owned(),
                tools::command_execution(command, tools::IN_PROGRESS, None, None),
            ))
        }
        Some(Work::Read) => {
            let path = input.get("file_path").and_then(Value::as_str)?;
            Some((
                tools::FILE_READ.to_owned(),
                tools::file_read(path, tools::IN_PROGRESS, None),
            ))
        }
        Some(Work::Change) => None,
        None => Some((tool.to_owned(), input.clone())),
    }
}

/// What a finished tool call should show, given the result Claude Code reported
/// for it. Tools outside the shared vocabulary keep the call they already
/// showed, so their results do not repeat them.
fn finished_tool_call(
    tool: &str,
    input: &Value,
    result: Option<&Value>,
    is_error: bool,
) -> Option<(String, Value)> {
    let status = if is_error {
        tools::FAILED
    } else {
        tools::COMPLETED
    };
    match work(tool)? {
        Work::Command => {
            let command = input.get("command").and_then(Value::as_str)?;
            let output = result.map(command_output);
            Some((
                command.to_owned(),
                tools::command_execution(
                    command,
                    status,
                    output.as_deref(),
                    result
                        .and_then(|result| result.get("exitCode"))
                        .and_then(Value::as_i64),
                ),
            ))
        }
        Work::Read => {
            let path = result
                .and_then(|result| result.pointer("/file/filePath"))
                .and_then(Value::as_str)
                .or_else(|| input.get("file_path").and_then(Value::as_str))?;
            let lines = result
                .and_then(|result| result.pointer("/file/numLines"))
                .and_then(Value::as_u64);
            Some((
                tools::FILE_READ.to_owned(),
                tools::file_read(path, status, lines),
            ))
        }
        Work::Change => {
            let changes = changed_files(input, result);
            if changes.is_empty() {
                // Nothing the result describes as a patch — a refused write, or
                // an edit of something that is not a text file. Report the call
                // itself rather than hiding the work.
                return Some((tool.to_owned(), input.clone()));
            }
            Some((
                tools::FILE_CHANGE.to_owned(),
                tools::file_change(status, changes),
            ))
        }
    }
}

fn command_output(result: &Value) -> String {
    let stream = |key: &str| {
        result
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let stdout = stream("stdout");
    let stderr = stream("stderr");
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, _) => stderr,
        (_, true) => stdout,
        _ => format!("{}\n{stderr}", stdout.trim_end()),
    }
}

/// The files a write, edit, or notebook edit changed, as canonical changes.
fn changed_files(input: &Value, result: Option<&Value>) -> Vec<Value> {
    let Some(path) = result
        .and_then(|result| result.get("filePath"))
        .and_then(Value::as_str)
        .or_else(|| input.get("file_path").and_then(Value::as_str))
    else {
        return Vec::new();
    };
    let created = result
        .and_then(|result| result.get("type"))
        .and_then(Value::as_str)
        == Some("create");
    let hunks = result
        .and_then(|result| result.get("structuredPatch"))
        .and_then(Value::as_array)
        .map(|hunks| hunks.iter().filter_map(structured_hunk).collect::<String>())
        .filter(|diff| !diff.is_empty());

    if let Some(diff) = hunks {
        return vec![tools::change(
            path,
            if created { "add" } else { "update" },
            &diff,
        )];
    }
    let content = result
        .and_then(|result| result.get("content"))
        .and_then(Value::as_str)
        .or_else(|| input.get("content").and_then(Value::as_str));
    match content.filter(|_| created) {
        Some(content) => vec![tools::change(path, "add", &tools::added_file_hunk(content))],
        None => Vec::new(),
    }
}

fn structured_hunk(hunk: &Value) -> Option<String> {
    let number = |key: &str| hunk.get(key).and_then(Value::as_u64).unwrap_or_default();
    let lines = hunk
        .get("lines")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    Some(tools::hunk(
        number("oldStart"),
        number("oldLines"),
        number("newStart"),
        number("newLines"),
        &lines,
    ))
}

fn import_entry(raw: Value, index: usize) -> Option<ConversationEvent> {
    let entry_type = raw.get("type").and_then(Value::as_str)?;
    let role = match entry_type {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "system" => MessageRole::System,
        _ => return None,
    };
    let content = import_content(raw.pointer("/message/content")?);
    if content.is_empty() {
        // Tool calls and their results are the whole content of these entries,
        // and they are imported as tool events rather than empty messages.
        return None;
    }
    let payload = EventPayload::Message { role, content };
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

/// Imports the tool calls and results of one history entry, normalizing them
/// exactly as the live stream does: a started call is recorded, and its result
/// completes that call in place instead of adding a second event.
fn import_tool_entries(
    raw: &Value,
    events: &mut Vec<ConversationEvent>,
    pending: &mut HashMap<String, PendingTool>,
) {
    for block in raw
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => {
                let Some(id) = block.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_owned();
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                if let Some((canonical, canonical_input)) = started_tool_call(&name, &input) {
                    events.push(tool_event(
                        id,
                        canonical,
                        canonical_input,
                        native(raw.clone()),
                    ));
                }
                pending.insert(id.to_owned(), PendingTool { name, input });
            }
            Some("tool_result") => {
                let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(call) = pending.remove(id) else {
                    continue;
                };
                let Some((name, input)) = finished_tool_call(
                    &call.name,
                    &call.input,
                    tool_use_result(raw),
                    block
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                ) else {
                    continue;
                };
                let completed = tool_event(id, name, input, native(raw.clone()));
                match events.iter().position(|event| event.id == id) {
                    Some(position) => events[position] = completed,
                    None => events.push(completed),
                }
            }
            _ => {}
        }
    }
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

    fn assistant(block: Value) -> Value {
        json!({
            "type": "assistant",
            "message": {"id": "msg-one", "role": "assistant", "content": [block]}
        })
    }

    fn tool_result(id: &str, content: &str, result: Value) -> Value {
        json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": id, "content": content}
            ]},
            "tool_use_result": result
        })
    }

    fn tool_call(update: &ConversationUpdate) -> (&str, &str, &Value) {
        let ConversationUpdate::Append {
            event:
                ConversationEvent {
                    id,
                    payload: EventPayload::ToolCall { name, input, .. },
                    ..
                },
        } = update
        else {
            panic!("expected a tool call, got {update:?}");
        };
        (id, name, input)
    }

    #[test]
    fn imports_mainline_and_preserves_native_records() {
        let artifact = NativeArtifact::JsonLines(
            r#"{"type":"user","uuid":"one","parentUuid":null,"message":{"role":"user","content":"Hello"}}
{"type":"assistant","uuid":"two","parentUuid":"one","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}]}}
{"type":"assistant","uuid":"side","isSidechain":true,"message":{"role":"assistant","content":"hidden"}}"#
                .to_owned(),
        );
        let result = ClaudeTranslator::default().import(&artifact).unwrap();
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
        let translator = ClaudeTranslator::default();
        let imported = translator.import(&artifact).unwrap();
        let exported = translator.export(&imported.conversation).unwrap();
        assert_eq!(exported.report.fidelity, Fidelity::Exact);
        translator.validate(&exported.artifact).unwrap();
    }

    #[test]
    fn tool_arguments_are_only_read_once_the_assistant_message_completes() {
        let translator = ClaudeTranslator::default();
        // The streamed start of a tool call carries no arguments yet.
        let started = translator
            .translate_live(&json!({
                "type": "stream_event",
                "event": {
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {"type": "tool_use", "id": "tool-one", "name": "Bash", "input": {}}
                }
            }))
            .unwrap();
        assert!(started.is_empty());

        let updates = translator
            .translate_live(&assistant(json!({
                "type": "tool_use",
                "id": "tool-one",
                "name": "Bash",
                "input": {"command": "cargo test", "description": "Run tests"}
            })))
            .unwrap();
        let (id, name, input) = tool_call(&updates[0]);
        assert_eq!((id, name), ("tool-one", "cargo test"));
        assert_eq!(tools::kind(input), Some(tools::COMMAND_EXECUTION));
        assert_eq!(input["command"], "cargo test");
        assert_eq!(input["status"], tools::IN_PROGRESS);
    }

    #[test]
    fn command_results_complete_the_command_that_started_them() {
        let translator = ClaudeTranslator::default();
        translator
            .translate_live(&assistant(json!({
                "type": "tool_use", "id": "tool-one", "name": "Bash",
                "input": {"command": "cargo test"}
            })))
            .unwrap();

        let updates = translator
            .translate_live(&tool_result(
                "tool-one",
                "ok",
                json!({"stdout": "3 passed\n", "stderr": "", "interrupted": false}),
            ))
            .unwrap();

        let (id, name, input) = tool_call(&updates[0]);
        assert_eq!((id, name), ("tool-one", "cargo test"));
        assert_eq!(input["status"], tools::COMPLETED);
        assert_eq!(input["aggregatedOutput"], "3 passed\n");
    }

    #[test]
    fn failed_commands_keep_their_output_and_report_failure() {
        let translator = ClaudeTranslator::default();
        translator
            .translate_live(&assistant(json!({
                "type": "tool_use", "id": "tool-one", "name": "Bash",
                "input": {"command": "cargo build"}
            })))
            .unwrap();
        let failure = json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "tool-one", "content": "boom", "is_error": true}
            ]},
            "tool_use_result": {"stdout": "", "stderr": "error: boom\n"}
        });

        let updates = translator.translate_live(&failure).unwrap();
        let (_, _, input) = tool_call(&updates[0]);
        assert_eq!(input["status"], tools::FAILED);
        assert_eq!(input["aggregatedOutput"], "error: boom\n");
    }

    #[test]
    fn reads_report_the_file_they_opened() {
        let translator = ClaudeTranslator::default();
        let started = translator
            .translate_live(&assistant(json!({
                "type": "tool_use", "id": "read-one", "name": "Read",
                "input": {"file_path": "/repo/src/lib.rs"}
            })))
            .unwrap();
        let (_, name, input) = tool_call(&started[0]);
        assert_eq!(name, tools::FILE_READ);
        assert_eq!(input["path"], "/repo/src/lib.rs");
        assert_eq!(input["status"], tools::IN_PROGRESS);

        let finished = translator
            .translate_live(&tool_result(
                "read-one",
                "1\talpha\n",
                json!({"type": "text", "file": {"filePath": "/repo/src/lib.rs", "numLines": 42}}),
            ))
            .unwrap();
        let (_, _, input) = tool_call(&finished[0]);
        assert_eq!(input["status"], tools::COMPLETED);
        assert_eq!(input["lines"], 42);
    }

    #[test]
    fn edits_become_the_same_file_change_codex_reports() {
        let translator = ClaudeTranslator::default();
        let started = translator
            .translate_live(&assistant(json!({
                "type": "tool_use", "id": "edit-one", "name": "Edit",
                "input": {"file_path": "/repo/sample.txt", "old_string": "beta", "new_string": "BETA"}
            })))
            .unwrap();
        assert!(
            started.is_empty(),
            "a file change is only reported once its patch is known"
        );

        let updates = translator
            .translate_live(&tool_result(
                "edit-one",
                "updated",
                json!({
                    "filePath": "/repo/sample.txt",
                    "structuredPatch": [{
                        "oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
                        "lines": [" alpha", "-beta", "+BETA", " gamma"]
                    }]
                }),
            ))
            .unwrap();

        let (id, name, input) = tool_call(&updates[0]);
        assert_eq!((id, name), ("edit-one", tools::FILE_CHANGE));
        assert_eq!(input["status"], tools::COMPLETED);
        assert_eq!(input["changes"][0]["path"], "/repo/sample.txt");
        assert_eq!(input["changes"][0]["kind"], "update");
        assert_eq!(
            input["changes"][0]["diff"],
            "@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n"
        );
    }

    #[test]
    fn written_files_are_reported_as_created_with_their_contents() {
        let translator = ClaudeTranslator::default();
        translator
            .translate_live(&assistant(json!({
                "type": "tool_use", "id": "write-one", "name": "Write",
                "input": {"file_path": "/repo/made.txt", "content": "done\n"}
            })))
            .unwrap();

        let updates = translator
            .translate_live(&tool_result(
                "write-one",
                "File created successfully at: /repo/made.txt",
                json!({
                    "type": "create",
                    "filePath": "/repo/made.txt",
                    "content": "done\n",
                    "structuredPatch": []
                }),
            ))
            .unwrap();

        let (_, name, input) = tool_call(&updates[0]);
        assert_eq!(name, tools::FILE_CHANGE);
        assert_eq!(input["changes"][0]["kind"], "add");
        assert_eq!(input["changes"][0]["diff"], "@@ -0,0 +1,1 @@\n+done\n");
    }

    #[test]
    fn a_refused_write_is_still_reported() {
        let translator = ClaudeTranslator::default();
        translator
            .translate_live(&assistant(json!({
                "type": "tool_use", "id": "write-two", "name": "Write",
                "input": {"file_path": "/repo/made.txt", "content": "done\n"}
            })))
            .unwrap();

        let refusal = json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "write-two", "content": "denied", "is_error": true}
            ]}
        });
        let updates = translator.translate_live(&refusal).unwrap();

        let (_, name, input) = tool_call(&updates[0]);
        assert_eq!(name, "Write");
        assert_eq!(input["file_path"], "/repo/made.txt");
    }

    #[test]
    fn tools_outside_the_shared_vocabulary_keep_their_own_input() {
        let translator = ClaudeTranslator::default();
        let updates = translator
            .translate_live(&assistant(json!({
                "type": "tool_use", "id": "grep-one", "name": "Grep",
                "input": {"pattern": "todo"}
            })))
            .unwrap();
        let (_, name, input) = tool_call(&updates[0]);
        assert_eq!(name, "Grep");
        assert_eq!(input["pattern"], "todo");

        let results = translator
            .translate_live(&tool_result("grep-one", "3 matches", json!({})))
            .unwrap();
        assert!(
            results.is_empty(),
            "results only complete calls in the shared vocabulary"
        );
    }

    #[test]
    fn streamed_text_is_kept_per_block_and_not_repeated_by_the_message() {
        let translator = ClaudeTranslator::default();
        translator
            .translate_live(&json!({
                "type": "stream_event",
                "event": {"type": "message_start", "message": {"id": "msg-one"}}
            }))
            .unwrap();
        let first = translator
            .translate_live(&json!({
                "type": "stream_event",
                "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Checking"}}
            }))
            .unwrap();
        let second = translator
            .translate_live(&json!({
                "type": "stream_event",
                "event": {"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "Found it"}}
            }))
            .unwrap();
        let ids = [&first[0], &second[0]].map(|update| match update {
            ConversationUpdate::AppendText { event_id, .. } => event_id.clone(),
            other => panic!("expected streamed text, got {other:?}"),
        });
        assert_eq!(ids, ["msg-one-0".to_owned(), "msg-one-2".to_owned()]);

        assert!(
            translator
                .translate_live(&assistant(json!({"type": "text", "text": "Checking"})))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn text_that_never_streamed_is_still_reported() {
        let updates = ClaudeTranslator::default()
            .translate_live(&assistant(json!({"type": "text", "text": "All done"})))
            .unwrap();
        assert!(matches!(
            &updates[0],
            ConversationUpdate::Append {
                event: ConversationEvent {
                    payload: EventPayload::Message { role: MessageRole::Assistant, content },
                    ..
                }
            } if content == &[ContentBlock::Text { text: "All done".to_owned() }]
        ));
    }

    #[test]
    fn subagent_work_stays_out_of_the_main_transcript() {
        let translator = ClaudeTranslator::default();
        let mut nested = assistant(json!({
            "type": "tool_use", "id": "tool-one", "name": "Bash",
            "input": {"command": "cargo test"}
        }));
        nested["parent_tool_use_id"] = json!("toolu-task");

        assert!(translator.translate_live(&nested).unwrap().is_empty());
    }

    #[test]
    fn a_finished_turn_releases_the_state_it_was_holding() {
        let translator = ClaudeTranslator::default();
        translator
            .translate_live(&assistant(json!({
                "type": "tool_use", "id": "tool-one", "name": "Bash",
                "input": {"command": "cargo test"}
            })))
            .unwrap();

        translator
            .translate_live(&json!({"type": "result", "subtype": "success"}))
            .unwrap();

        assert!(
            translator
                .translate_live(&tool_result("tool-one", "ok", json!({"stdout": "ok"})))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn imported_history_normalizes_tools_the_same_way_the_stream_does() {
        let artifact = NativeArtifact::JsonLines(
            r#"{"type":"user","uuid":"one","message":{"role":"user","content":"Fix it"}}
{"type":"assistant","uuid":"two","message":{"role":"assistant","content":[{"type":"tool_use","id":"edit-one","name":"Edit","input":{"file_path":"/repo/sample.txt","old_string":"beta","new_string":"BETA"}}]}}
{"type":"user","uuid":"three","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"edit-one","content":"updated"}]},"toolUseResult":{"filePath":"/repo/sample.txt","structuredPatch":[{"oldStart":1,"oldLines":3,"newStart":1,"newLines":3,"lines":[" alpha","-beta","+BETA"," gamma"]}]}}
{"type":"assistant","uuid":"four","message":{"role":"assistant","content":[{"type":"tool_use","id":"bash-one","name":"Bash","input":{"command":"cargo test"}}]}}
{"type":"user","uuid":"five","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"bash-one","content":"ok"}]},"toolUseResult":{"stdout":"3 passed\n","stderr":""}}"#
                .to_owned(),
        );

        let events = ClaudeTranslator::default()
            .import(&artifact)
            .unwrap()
            .conversation
            .events;

        let kinds = events
            .iter()
            .map(|event| match &event.payload {
                EventPayload::Message { role, .. } => format!("{role:?}"),
                EventPayload::ToolCall { name, .. } => name.clone(),
                other => panic!("unexpected payload {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(kinds, ["User", tools::FILE_CHANGE, "cargo test"]);

        let EventPayload::ToolCall { input, .. } = &events[1].payload else {
            unreachable!()
        };
        assert_eq!(input["changes"][0]["kind"], "update");
        let EventPayload::ToolCall { input, .. } = &events[2].payload else {
            unreachable!()
        };
        assert_eq!(input["aggregatedOutput"], "3 passed\n");
    }
}
