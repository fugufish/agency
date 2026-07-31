//! Codex and Claude Code describe the same work in different protocols. These
//! tests replay a recorded exchange of each — the same command, the same edit —
//! and assert both translate into the same canonical events, so a transcript
//! renders either agent identically.

use agency_translator_api::{
    Conversation, ConversationUpdate, EventPayload, LiveEventTranslator, tools,
};
use agency_translators::{claude::ClaudeTranslator, codex::CodexTranslator};
use serde_json::{Value, json};

/// Applies updates the way the transcript does, so work reported twice is
/// replaced in place rather than repeated.
fn conversation(translator: &dyn LiveEventTranslator, stream: Vec<Value>) -> Conversation {
    let mut conversation = Conversation::default();
    for value in stream {
        for update in translator.translate_live(&value).unwrap() {
            let ConversationUpdate::Append { event } = update else {
                continue;
            };
            match conversation
                .events
                .iter_mut()
                .find(|existing| existing.id == event.id)
            {
                Some(existing) => existing.payload = event.payload,
                None => conversation.events.push(event),
            }
        }
    }
    conversation
}

fn tool_inputs(conversation: &Conversation, kind: &str) -> Vec<Value> {
    conversation
        .events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolCall { input, .. } => Some(input),
            _ => None,
        })
        .filter(|input| tools::kind(input) == Some(kind))
        .cloned()
        .collect()
}

fn codex_item(method: &str, item: Value) -> Value {
    json!({ "method": method, "params": { "turnId": "turn-1", "item": item } })
}

fn claude_tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "id": "msg-one",
            "role": "assistant",
            "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]
        }
    })
}

fn claude_tool_result(id: &str, content: &str, result: Value) -> Value {
    json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": id, "content": content}]
        },
        "tool_use_result": result
    })
}

#[test]
fn a_command_is_reported_the_same_way_by_both_agents() {
    let codex = conversation(
        &CodexTranslator,
        vec![
            codex_item(
                "item/started",
                json!({
                    "id": "exec-1", "type": "commandExecution", "command": "echo hello-agency",
                    "status": "inProgress", "aggregatedOutput": null, "exitCode": null
                }),
            ),
            codex_item(
                "item/completed",
                json!({
                    "id": "exec-1", "type": "commandExecution", "command": "echo hello-agency",
                    "status": "completed", "aggregatedOutput": "hello-agency\n", "exitCode": 0
                }),
            ),
        ],
    );
    let claude = conversation(
        &ClaudeTranslator::default(),
        vec![
            claude_tool_use(
                "toolu-1",
                "Bash",
                json!({"command": "echo hello-agency", "description": "Say hello"}),
            ),
            claude_tool_result(
                "toolu-1",
                "hello-agency",
                json!({"stdout": "hello-agency\n", "stderr": "", "interrupted": false}),
            ),
        ],
    );

    let codex = tool_inputs(&codex, tools::COMMAND_EXECUTION);
    let claude = tool_inputs(&claude, tools::COMMAND_EXECUTION);
    assert_eq!(codex.len(), 1, "the command is reported once, not twice");
    assert_eq!(claude.len(), 1, "the command is reported once, not twice");
    for input in [&codex[0], &claude[0]] {
        assert_eq!(input["command"], "echo hello-agency");
        assert_eq!(tools::status(input), tools::COMPLETED);
        assert_eq!(input["aggregatedOutput"], "hello-agency\n");
    }
}

#[test]
fn an_edit_is_reported_the_same_way_by_both_agents() {
    let hunk = "@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n";
    let codex = conversation(
        &CodexTranslator,
        vec![
            codex_item(
                "item/started",
                json!({
                    "id": "change-1", "type": "fileChange", "status": "inProgress",
                    "changes": [{"path": "/repo/sample.txt", "kind": {"type": "update", "move_path": null}, "diff": hunk}]
                }),
            ),
            codex_item(
                "item/completed",
                json!({
                    "id": "change-1", "type": "fileChange", "status": "completed",
                    "changes": [{"path": "/repo/sample.txt", "kind": {"type": "update", "move_path": null}, "diff": hunk}]
                }),
            ),
        ],
    );
    let claude = conversation(
        &ClaudeTranslator::default(),
        vec![
            claude_tool_use(
                "toolu-2",
                "Edit",
                json!({"file_path": "/repo/sample.txt", "old_string": "beta", "new_string": "BETA"}),
            ),
            claude_tool_result(
                "toolu-2",
                "updated",
                json!({
                    "filePath": "/repo/sample.txt",
                    "structuredPatch": [{
                        "oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
                        "lines": [" alpha", "-beta", "+BETA", " gamma"]
                    }]
                }),
            ),
        ],
    );

    let codex = tool_inputs(&codex, tools::FILE_CHANGE);
    let claude = tool_inputs(&claude, tools::FILE_CHANGE);
    assert_eq!(codex.len(), 1, "the change is reported once it is applied");
    assert_eq!(claude.len(), 1, "the change is reported once it is applied");
    for input in [&codex[0], &claude[0]] {
        assert_eq!(tools::status(input), tools::COMPLETED);
        let change = &input["changes"][0];
        assert_eq!(change["path"], "/repo/sample.txt");
        assert_eq!(tools::change_kind(change), "update");
        assert_eq!(change["diff"], hunk);
    }
}

#[test]
fn provider_specific_work_keeps_its_own_shape() {
    let claude = conversation(
        &ClaudeTranslator::default(),
        vec![claude_tool_use(
            "toolu-3",
            "Read",
            json!({"file_path": "/repo/src/lib.rs"}),
        )],
    );
    let reads = tool_inputs(&claude, tools::FILE_READ);
    assert_eq!(reads[0]["path"], "/repo/src/lib.rs");
    assert_eq!(tools::status(&reads[0]), tools::IN_PROGRESS);

    let codex = conversation(
        &CodexTranslator,
        vec![codex_item(
            "item/started",
            json!({"id": "mcp-1", "type": "mcpToolCall", "name": "search", "status": "inProgress"}),
        )],
    );
    assert!(tool_inputs(&codex, tools::FILE_READ).is_empty());
    assert_eq!(codex.events.len(), 1, "other tools are still reported");
}
