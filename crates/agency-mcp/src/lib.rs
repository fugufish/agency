use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use agency_rpc::{ENV_RPC_SOCKET, ENV_SESSION_TOKEN, Request};
use serde_json::{Value, json};

pub fn run() -> Result<(), String> {
    let socket = std::env::var_os(ENV_RPC_SOCKET)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{ENV_RPC_SOCKET} is not set"))?;
    let token =
        std::env::var(ENV_SESSION_TOKEN).map_err(|_| format!("{ENV_SESSION_TOKEN} is not set"))?;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line.map_err(|error| format!("Could not read MCP request: {error}"))?;
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                write_json(
                    &mut stdout,
                    &rpc_error(Value::Null, -32700, format!("Parse error: {error}")),
                )?;
                continue;
            }
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let response = match method {
            "initialize" => {
                notify_status(&socket, &token, true);
                let protocol = message
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2024-11-05");
                rpc_result(
                    id,
                    json!({
                        "protocolVersion": protocol,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": {
                            "name": "agency",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }),
                )
            }
            "ping" => rpc_result(id, json!({})),
            "tools/list" => rpc_result(id, json!({ "tools": tools() })),
            "tools/call" => call_tool(id, &message, &socket, &token),
            _ => rpc_error(id, -32601, format!("Method not found: {method}")),
        };
        write_json(&mut stdout, &response)?;
    }
    notify_status(&socket, &token, false);
    Ok(())
}

fn notify_status(socket: &std::path::Path, token: &str, connected: bool) {
    let _ = agency_rpc::call(
        socket,
        &Request {
            token: token.to_owned(),
            method: "mcp.status".to_owned(),
            params: json!({ "connected": connected }),
        },
    );
}

fn tools() -> Value {
    json!([
        {
            "name": "list_worktrees",
            "description": "List Git worktrees in the caller's Agency workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        },
        {
            "name": "create_worktree",
            "description": "Create a Git worktree and branch in the caller's Agency workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch": {
                        "type": "string",
                        "description": "New local branch name."
                    },
                    "base": {
                        "type": "string",
                        "description": "Existing revision from which to create the branch."
                    }
                },
                "required": ["branch"],
                "additionalProperties": false
            }
        }
    ])
}

fn call_tool(id: Value, message: &Value, socket: &std::path::Path, token: &str) -> Value {
    let Some(name) = message.pointer("/params/name").and_then(Value::as_str) else {
        return rpc_error(id, -32602, "Missing tool name");
    };
    let method = match name {
        "list_worktrees" => "worktree.list",
        "create_worktree" => "worktree.create",
        _ => return rpc_error(id, -32602, format!("Unknown Agency tool: {name}")),
    };
    let params = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let response = agency_rpc::call(
        socket,
        &Request {
            token: token.to_owned(),
            method: method.to_owned(),
            params,
        },
    );
    match response {
        Ok(response) => match (response.result, response.error) {
            (Some(result), _) => rpc_result(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string_pretty(&result).unwrap_or_default()
                    }],
                    "structuredContent": result
                }),
            ),
            (_, Some(error)) => tool_error(id, error),
            _ => tool_error(id, "Agency returned an empty response"),
        },
        Err(error) => tool_error(id, error),
    }
}

fn tool_error(id: Value, error: impl Into<String>) -> Value {
    rpc_result(
        id,
        json!({
            "content": [{ "type": "text", "text": error.into() }],
            "isError": true
        }),
    )
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

fn write_json(writer: &mut impl Write, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| format!("Could not encode MCP response: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("Could not write MCP response: {error}"))
}
