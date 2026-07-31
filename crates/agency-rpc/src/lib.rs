use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ENV_RPC_SOCKET: &str = "AGENCY_RPC_SOCKET";
pub const ENV_SESSION_TOKEN: &str = "AGENCY_SESSION_TOKEN";
pub const ENV_MCP_COMMAND: &str = "AGENCY_MCP_COMMAND";
pub const ENV_CONVERSATION_ID: &str = "AGENCY_CONVERSATION_ID";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub token: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn success(result: Value) -> Self {
        Self {
            result: Some(result),
            error: None,
        }
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self {
            result: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    pub conversation_id: String,
    pub workspace: PathBuf,
    pub provider: String,
    pub provider_session_id: Option<String>,
    pub generation: u64,
}

#[derive(Clone, Default)]
pub struct SessionCapabilities {
    sessions: Arc<Mutex<HashMap<String, SessionContext>>>,
}

impl SessionCapabilities {
    pub fn issue(&self, context: SessionContext) -> Result<String, String> {
        let token = random_token()?;
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(token.clone(), context);
        Ok(token)
    }

    pub fn resolve(&self, token: &str) -> Option<SessionContext> {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(token)
            .cloned()
    }

    pub fn bind_provider_session(&self, token: &str, id: String) {
        if let Some(context) = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(token)
        {
            context.provider_session_id = Some(id);
        }
    }

    pub fn revoke(&self, token: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(token);
    }
}

pub struct Call {
    pub context: SessionContext,
    pub method: String,
    pub params: Value,
    pub reply: mpsc::Sender<Response>,
}

pub struct Server {
    socket_path: PathBuf,
    calls: mpsc::Receiver<Call>,
}

impl Server {
    pub fn start(capabilities: SessionCapabilities) -> Result<Self, String> {
        let socket_path = std::env::temp_dir().join(format!("agency-{}.sock", std::process::id()));
        if socket_path.exists() {
            fs::remove_file(&socket_path)
                .map_err(|error| format!("Could not replace {}: {error}", socket_path.display()))?;
        }
        let listener = UnixListener::bind(&socket_path)
            .map_err(|error| format!("Could not bind {}: {error}", socket_path.display()))?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Could not secure {}: {error}", socket_path.display()))?;
        let (calls_tx, calls) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let calls = calls_tx.clone();
                        let capabilities = capabilities.clone();
                        thread::spawn(move || serve_connection(stream, capabilities, calls));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self { socket_path, calls })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn try_calls(&self) -> impl Iterator<Item = Call> + '_ {
        self.calls.try_iter()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub fn call(socket: &Path, request: &Request) -> Result<Response, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| format!("Could not connect to Agency: {error}"))?;
    serde_json::to_writer(&mut stream, request)
        .map_err(|error| format!("Could not encode Agency RPC request: {error}"))?;
    stream
        .write_all(b"\n")
        .map_err(|error| format!("Could not send Agency RPC request: {error}"))?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| format!("Could not read Agency RPC response: {error}"))?;
    serde_json::from_str(&line)
        .map_err(|error| format!("Could not decode Agency RPC response: {error}"))
}

fn serve_connection(
    stream: UnixStream,
    capabilities: SessionCapabilities,
    calls: mpsc::Sender<Call>,
) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    for line in BufReader::new(reader_stream).lines() {
        let response = match line
            .map_err(|error| error.to_string())
            .and_then(|line| serde_json::from_str::<Request>(&line).map_err(|e| e.to_string()))
        {
            Ok(request) => match capabilities.resolve(&request.token) {
                Some(context) => {
                    let (reply, response) = mpsc::channel();
                    if calls
                        .send(Call {
                            context,
                            method: request.method,
                            params: request.params,
                            reply,
                        })
                        .is_err()
                    {
                        Response::error("Agency RPC service stopped")
                    } else {
                        response
                            .recv()
                            .unwrap_or_else(|_| Response::error("Agency dropped the RPC call"))
                    }
                }
                None => Response::error("Invalid or expired Agency session capability"),
            },
            Err(error) => Response::error(format!("Invalid Agency RPC request: {error}")),
        };
        if serde_json::to_writer(&mut writer, &response).is_err()
            || writer.write_all(b"\n").is_err()
        {
            break;
        }
    }
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("Could not generate an Agency capability: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_resolve_and_revoke_session_context() {
        let capabilities = SessionCapabilities::default();
        let context = SessionContext {
            conversation_id: "conversation-1".to_owned(),
            workspace: PathBuf::from("/repo"),
            provider: "claude".to_owned(),
            provider_session_id: None,
            generation: 1,
        };
        let token = capabilities.issue(context.clone()).unwrap();
        assert_eq!(capabilities.resolve(&token), Some(context));
        capabilities.bind_provider_session(&token, "claude-1".to_owned());
        assert_eq!(
            capabilities
                .resolve(&token)
                .unwrap()
                .provider_session_id
                .as_deref(),
            Some("claude-1")
        );
        capabilities.revoke(&token);
        assert_eq!(capabilities.resolve(&token), None);
    }
}
