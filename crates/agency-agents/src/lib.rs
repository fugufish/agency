use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::thread;

use agency_translator_api::{
    Conversation, ConversationUpdate, LiveEventTranslator, NativeArtifact, SessionTranslator,
};
use agency_translators::{claude::ClaudeTranslator, codex::CodexTranslator};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Codex,
    Claude,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    SessionStarted { id: String, name: Option<String> },
    SessionNameChanged { id: String, name: String },
    Ready,
    ConversationReset(Conversation),
    Conversation(Box<ConversationUpdate>),
    Status(String),
    Question(QuestionRequest),
    Error(String),
    TurnCompleted,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub media_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct QuestionRequest {
    pub id: u64,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone)]
pub struct Question {
    pub header: String,
    pub text: String,
    pub choices: Vec<Choice>,
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub label: String,
    pub description: String,
}

#[derive(Debug)]
enum PendingResponse {
    CodexApproval {
        request_id: Value,
    },
    CodexUserInput {
        request_id: Value,
        question_ids: Vec<String>,
        option_labels: Vec<Vec<String>>,
    },
    ClaudePermission {
        request_id: String,
        input: Value,
    },
}

pub struct Session {
    provider: Provider,
    writer: Arc<Mutex<ChildStdin>>,
    events: mpsc::Receiver<Event>,
    codex_thread_id: Arc<Mutex<Option<String>>>,
    next_request_id: Arc<AtomicU64>,
    pending_codex_turns: Arc<Mutex<Vec<Vec<Value>>>>,
    pending_responses: Arc<Mutex<HashMap<u64, PendingResponse>>>,
}

impl Session {
    pub fn spawn(provider: Provider, cwd: &Path) -> Result<Self, String> {
        match provider {
            Provider::Codex => Self::spawn_codex(cwd, None, None),
            Provider::Claude => Self::spawn_claude(cwd, None),
        }
    }

    pub fn spawn_with_history(
        provider: Provider,
        cwd: &Path,
        conversation: &Conversation,
    ) -> Result<Self, String> {
        match provider {
            Provider::Codex => Self::spawn_codex(cwd, None, Some(conversation)),
            Provider::Claude => Err(
                "Claude history must be installed into its native store before resuming".to_owned(),
            ),
        }
    }

    pub fn resume(provider: Provider, session_id: &str, cwd: &Path) -> Result<Self, String> {
        if session_id.trim().is_empty() {
            return Err("Cannot resume an agent session without an ID".to_owned());
        }
        match provider {
            Provider::Codex => Self::spawn_codex(cwd, Some(session_id), None),
            Provider::Claude => Self::spawn_claude(cwd, Some(session_id)),
        }
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    pub fn send(&mut self, prompt: String, images: Vec<Image>) -> Result<(), String> {
        let encoded_images = images
            .into_iter()
            .map(|image| (image.media_type, STANDARD.encode(image.data)))
            .collect::<Vec<_>>();
        let message = match self.provider {
            Provider::Codex => {
                let input = {
                    let mut input = vec![json!({ "type": "text", "text": prompt })];
                    input.extend(encoded_images.into_iter().map(|(media_type, data)| {
                        json!({
                            "type": "image",
                            "url": format!("data:{media_type};base64,{data}")
                        })
                    }));
                    input
                };
                let thread_id = self
                    .codex_thread_id
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                let Some(thread_id) = thread_id else {
                    self.pending_codex_turns
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .push(input);
                    let initialized = self
                        .codex_thread_id
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone();
                    if let Some(thread_id) = initialized {
                        let queued = std::mem::take(
                            &mut *self
                                .pending_codex_turns
                                .lock()
                                .unwrap_or_else(|error| error.into_inner()),
                        );
                        for input in queued {
                            let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
                            write_json(
                                &self.writer,
                                &codex_turn_message(&thread_id, input, request_id),
                            )?;
                        }
                    }
                    return Ok(());
                };
                codex_turn_message(
                    &thread_id,
                    input,
                    self.next_request_id.fetch_add(1, Ordering::Relaxed),
                )
            }
            Provider::Claude => {
                let mut content = vec![json!({ "type": "text", "text": prompt })];
                content.extend(encoded_images.into_iter().map(|(media_type, data)| {
                    json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data
                        }
                    })
                }));
                json!({
                    "type": "user",
                    "message": {
                        "role": "user",
                        "content": content
                    }
                })
            }
        };

        write_json(&self.writer, &message)
    }

    pub fn try_events(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }

    pub fn answer(&self, request_id: u64, answers: Vec<usize>) -> Result<(), String> {
        let pending = self
            .pending_responses
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&request_id)
            .ok_or_else(|| "That question is no longer pending".to_owned())?;

        let message = match pending {
            PendingResponse::CodexApproval { request_id } => {
                let decision = match answers.first().copied() {
                    Some(0) => "accept",
                    Some(1) => "acceptForSession",
                    _ => "decline",
                };
                json!({ "id": request_id, "result": { "decision": decision } })
            }
            PendingResponse::CodexUserInput {
                request_id,
                question_ids,
                option_labels,
            } => {
                if answers.len() != question_ids.len() {
                    return Err("Every question needs an answer".to_owned());
                }
                let answers = question_ids
                    .into_iter()
                    .zip(option_labels)
                    .zip(answers)
                    .map(|((id, labels), answer)| {
                        let answer = labels
                            .get(answer)
                            .cloned()
                            .unwrap_or_else(|| answer.to_string());
                        (id, json!({ "answers": [answer] }))
                    })
                    .collect::<serde_json::Map<_, _>>();
                json!({ "id": request_id, "result": { "answers": answers } })
            }
            PendingResponse::ClaudePermission { request_id, input } => {
                let response = match answers.first().copied() {
                    Some(0) => json!({ "behavior": "allow", "updatedInput": input }),
                    _ => json!({
                        "behavior": "deny",
                        "message": "The user denied this permission request."
                    }),
                };
                json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": request_id,
                        "response": response
                    }
                })
            }
        };

        write_json(&self.writer, &message)
    }

    fn spawn_codex(
        cwd: &Path,
        resume_id: Option<&str>,
        imported_history: Option<&Conversation>,
    ) -> Result<Self, String> {
        let mut child = Command::new("codex")
            .args(["app-server", "--stdio"])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start Codex app-server: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Codex stdin was unavailable".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex stdout was unavailable".to_owned())?;
        let stderr = child.stderr.take();
        let writer = Arc::new(Mutex::new(stdin));
        let thread_id = Arc::new(Mutex::new(None));
        let pending_responses = Arc::new(Mutex::new(HashMap::new()));
        let next_question_id = Arc::new(AtomicU64::new(1));
        let next_request_id = Arc::new(AtomicU64::new(2));
        let pending_codex_turns = Arc::new(Mutex::new(Vec::new()));
        let imported_history = imported_history
            .map(|conversation| {
                CodexTranslator
                    .export(conversation)
                    .map_err(|error| format!("Could not translate history for Codex: {error}"))
            })
            .transpose()?
            .and_then(|result| match result.artifact {
                NativeArtifact::Json(Value::Array(items)) => Some(items),
                _ => None,
            });
        let (events_tx, events_rx) = mpsc::channel();

        write_json(
            &writer,
            &json!({
                "method": "initialize",
                "id": 0,
                "params": {
                    "clientInfo": {
                        "name": "agency",
                        "title": "Agency",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
        )?;
        write_json(&writer, &json!({ "method": "initialized", "params": {} }))?;
        let thread_request = match resume_id {
            Some(thread_id) => json!({
                "method": "thread/resume",
                "id": 1,
                "params": { "threadId": thread_id, "cwd": cwd }
            }),
            None => json!({
                "method": "thread/start",
                "id": 1,
                "params": { "cwd": cwd }
            }),
        };
        write_json(&writer, &thread_request)?;

        let reader_thread_id = Arc::clone(&thread_id);
        let reader_pending = Arc::clone(&pending_responses);
        let reader_next_question_id = Arc::clone(&next_question_id);
        let reader_next_request_id = Arc::clone(&next_request_id);
        let reader_pending_turns = Arc::clone(&pending_codex_turns);
        let reader_writer = Arc::clone(&writer);
        let reader_events = events_tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => match serde_json::from_str::<Value>(&line) {
                        Ok(value) => {
                            if value.get("id").and_then(Value::as_u64) == Some(1)
                                && let Some(id) =
                                    value.pointer("/result/thread/id").and_then(Value::as_str)
                            {
                                *reader_thread_id
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner()) =
                                    Some(id.to_owned());
                                let queued = std::mem::take(
                                    &mut *reader_pending_turns
                                        .lock()
                                        .unwrap_or_else(|error| error.into_inner()),
                                );
                                for input in queued {
                                    let request_id =
                                        reader_next_request_id.fetch_add(1, Ordering::Relaxed);
                                    if let Err(error) = write_json(
                                        &reader_writer,
                                        &codex_turn_message(id, input, request_id),
                                    ) {
                                        let _ = reader_events.send(Event::Error(error));
                                    }
                                }
                                let name = value
                                    .pointer("/result/thread/name")
                                    .and_then(Value::as_str)
                                    .or_else(|| {
                                        value
                                            .pointer("/result/thread/preview")
                                            .and_then(Value::as_str)
                                            .filter(|preview| !preview.trim().is_empty())
                                    })
                                    .map(str::to_owned);
                                let _ = reader_events.send(Event::SessionStarted {
                                    id: id.to_owned(),
                                    name,
                                });
                                if let Some(items) = imported_history.as_ref()
                                    && !items.is_empty()
                                {
                                    let request_id =
                                        reader_next_request_id.fetch_add(1, Ordering::Relaxed);
                                    if let Err(error) = write_json(
                                        &reader_writer,
                                        &json!({
                                            "method": "thread/inject_items",
                                            "id": request_id,
                                            "params": {
                                                "threadId": id,
                                                "items": items,
                                            }
                                        }),
                                    ) {
                                        let _ = reader_events.send(Event::Error(error));
                                    }
                                }
                                if let Ok(imported) =
                                    CodexTranslator.import(&NativeArtifact::Json(value.clone()))
                                    && !imported.conversation.events.is_empty()
                                {
                                    let _ = reader_events
                                        .send(Event::ConversationReset(imported.conversation));
                                }
                                let _ = reader_events.send(Event::Ready);
                            }
                            normalize_codex(
                                &value,
                                &reader_events,
                                &reader_pending,
                                &reader_next_question_id,
                            );
                        }
                        Err(error) => {
                            let _ = reader_events
                                .send(Event::Error(format!("Invalid Codex event: {error}")));
                        }
                    },
                    Err(error) => {
                        let _ = reader_events
                            .send(Event::Error(format!("Codex stream failed: {error}")));
                        break;
                    }
                }
            }
            let _ = reader_events.send(Event::Status("Codex stopped".to_owned()));
        });
        pipe_stderr(stderr, events_tx, Provider::Codex);
        thread::spawn(move || {
            let _ = child.wait();
        });

        Ok(Self {
            provider: Provider::Codex,
            writer,
            events: events_rx,
            codex_thread_id: thread_id,
            next_request_id,
            pending_codex_turns,
            pending_responses,
        })
    }

    fn spawn_claude(cwd: &Path, resume_id: Option<&str>) -> Result<Self, String> {
        let mut command = Command::new("claude");
        command.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
        ]);
        if let Some(session_id) = resume_id {
            command.args(["--resume", session_id]);
        }
        let mut child = command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start Claude Code: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Claude stdin was unavailable".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Claude stdout was unavailable".to_owned())?;
        let stderr = child.stderr.take();
        let writer = Arc::new(Mutex::new(stdin));
        let thread_id = Arc::new(Mutex::new(None));
        let pending_responses = Arc::new(Mutex::new(HashMap::new()));
        let next_question_id = Arc::new(AtomicU64::new(1));
        let (events_tx, events_rx) = mpsc::channel();
        if let Some(session_id) = resume_id {
            match claude_history(cwd, session_id) {
                Ok(conversation) if !conversation.events.is_empty() => {
                    let _ = events_tx.send(Event::ConversationReset(conversation));
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = events_tx.send(Event::Error(error));
                }
            }
        }

        let reader_pending = Arc::clone(&pending_responses);
        let reader_next_question_id = Arc::clone(&next_question_id);
        let reader_events = events_tx.clone();
        let reader_thread_id = Arc::clone(&thread_id);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => match serde_json::from_str::<Value>(&line) {
                        Ok(value) => {
                            if value.get("type").and_then(Value::as_str) == Some("system")
                                && value.get("subtype").and_then(Value::as_str) == Some("init")
                                && let Some(id) = value.get("session_id").and_then(Value::as_str)
                            {
                                *reader_thread_id
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner()) =
                                    Some(id.to_owned());
                                let name = value
                                    .get("session_name")
                                    .or_else(|| value.get("name"))
                                    .and_then(Value::as_str)
                                    .filter(|name| !name.trim().is_empty())
                                    .map(str::to_owned);
                                let _ = reader_events.send(Event::SessionStarted {
                                    id: id.to_owned(),
                                    name,
                                });
                            }
                            normalize_claude(
                                &value,
                                &reader_events,
                                &reader_pending,
                                &reader_next_question_id,
                            )
                        }
                        Err(error) => {
                            let _ = reader_events
                                .send(Event::Error(format!("Invalid Claude event: {error}")));
                        }
                    },
                    Err(error) => {
                        let _ = reader_events
                            .send(Event::Error(format!("Claude stream failed: {error}")));
                        break;
                    }
                }
            }
            let _ = reader_events.send(Event::Status("Claude Code stopped".to_owned()));
        });
        pipe_stderr(stderr, events_tx, Provider::Claude);
        thread::spawn(move || {
            let _ = child.wait();
        });

        Ok(Self {
            provider: Provider::Claude,
            writer,
            events: events_rx,
            codex_thread_id: thread_id,
            next_request_id: Arc::new(AtomicU64::new(2)),
            pending_codex_turns: Arc::new(Mutex::new(Vec::new())),
            pending_responses,
        })
    }
}

fn claude_history(cwd: &Path, session_id: &str) -> Result<Conversation, String> {
    if !session_id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err("Claude session ID contains unsupported characters".to_owned());
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "Could not locate Claude's session store".to_owned())?;
    let projects = home.join(".claude/projects");
    let encoded_cwd = cwd
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
    let direct = projects
        .join(encoded_cwd)
        .join(format!("{session_id}.jsonl"));
    let path = if direct.exists() {
        direct
    } else {
        find_session_file(&projects, &format!("{session_id}.jsonl")).ok_or_else(|| {
            "Claude resumed the session, but its message history was not found".to_owned()
        })?
    };
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("Could not read Claude session history: {error}"))?;

    ClaudeTranslator
        .import(&NativeArtifact::JsonLines(source))
        .map(|result| result.conversation)
        .map_err(|error| format!("Could not translate Claude history: {error}"))
}

#[cfg(test)]
fn parse_claude_history(source: &str) -> Conversation {
    ClaudeTranslator
        .import(&NativeArtifact::JsonLines(source.to_owned()))
        .map(|result| result.conversation)
        .unwrap_or_default()
}

fn find_session_file(directory: &Path, filename: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(directory).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_session_file(&path, filename) {
                return Some(found);
            }
        } else if path.file_name().and_then(|name| name.to_str()) == Some(filename) {
            return Some(path);
        }
    }
    None
}

fn codex_turn_message(thread_id: &str, input: Vec<Value>, request_id: u64) -> Value {
    json!({
        "method": "turn/start",
        "id": request_id,
        "params": {
            "threadId": thread_id,
            "input": input
        }
    })
}

fn write_json(writer: &Mutex<ChildStdin>, value: &Value) -> Result<(), String> {
    let mut writer = writer.lock().unwrap_or_else(|error| error.into_inner());
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| format!("Could not encode agent message: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("Could not send agent message: {error}"))
}

fn normalize_codex(
    value: &Value,
    events: &mpsc::Sender<Event>,
    pending: &Mutex<HashMap<u64, PendingResponse>>,
    next_question_id: &AtomicU64,
) {
    match CodexTranslator.translate_live(value) {
        Ok(updates) => {
            for update in updates {
                let _ = events.send(Event::Conversation(Box::new(update)));
            }
        }
        Err(error) => {
            let _ = events.send(Event::Error(format!(
                "Could not translate Codex event: {error}"
            )));
        }
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        if let Some(error) = value.pointer("/error/message").and_then(Value::as_str) {
            let _ = events.send(Event::Error(error.to_owned()));
        }
        return;
    };

    match method {
        "thread/name/updated" => {
            if let (Some(id), Some(name)) = (
                value.pointer("/params/threadId").and_then(Value::as_str),
                value.pointer("/params/threadName").and_then(Value::as_str),
            ) {
                let _ = events.send(Event::SessionNameChanged {
                    id: id.to_owned(),
                    name: name.to_owned(),
                });
            }
        }
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            let Some(request_id) = value.get("id").cloned() else {
                return;
            };
            let id = next_question_id.fetch_add(1, Ordering::Relaxed);
            let params = &value["params"];
            let subject = params
                .get("command")
                .or_else(|| params.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("Allow the requested operation?");
            pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(id, PendingResponse::CodexApproval { request_id });
            let _ = events.send(Event::Question(QuestionRequest {
                id,
                questions: vec![Question {
                    header: "Permission".to_owned(),
                    text: subject.to_owned(),
                    choices: approval_choices(true),
                }],
            }));
            let _ = events.send(Event::Status("Waiting for answer".to_owned()));
        }
        "item/tool/requestUserInput" => {
            let Some(request_id) = value.get("id").cloned() else {
                return;
            };
            let questions = value
                .pointer("/params/questions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|question| Question {
                    header: string_at(question, "header", "Question"),
                    text: string_at(question, "question", "Choose an option"),
                    choices: question
                        .get("options")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|option| Choice {
                            label: string_at(option, "label", "Option"),
                            description: string_at(option, "description", ""),
                        })
                        .collect(),
                })
                .collect::<Vec<_>>();
            let question_ids = value
                .pointer("/params/questions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|question| string_at(question, "id", "question"))
                .collect();
            let option_labels = value
                .pointer("/params/questions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|question| {
                    question
                        .get("options")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|option| string_at(option, "label", "Option"))
                        .collect()
                })
                .collect();
            let id = next_question_id.fetch_add(1, Ordering::Relaxed);
            pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    id,
                    PendingResponse::CodexUserInput {
                        request_id,
                        question_ids,
                        option_labels,
                    },
                );
            let _ = events.send(Event::Question(QuestionRequest { id, questions }));
            let _ = events.send(Event::Status("Waiting for answer".to_owned()));
        }
        "item/agentMessage/delta" | "item/started" => {}
        "turn/started" => {
            let _ = events.send(Event::Status("Working".to_owned()));
        }
        "turn/completed" => {
            let _ = events.send(Event::TurnCompleted);
        }
        _ if method.contains("requestApproval") => {
            let _ = events.send(Event::Status("Approval required".to_owned()));
        }
        _ => {}
    }
}

fn normalize_claude(
    value: &Value,
    events: &mpsc::Sender<Event>,
    pending: &Mutex<HashMap<u64, PendingResponse>>,
    next_question_id: &AtomicU64,
) {
    match ClaudeTranslator.translate_live(value) {
        Ok(updates) => {
            for update in updates {
                let _ = events.send(Event::Conversation(Box::new(update)));
            }
        }
        Err(error) => {
            let _ = events.send(Event::Error(format!(
                "Could not translate Claude event: {error}"
            )));
        }
    }
    match value.get("type").and_then(Value::as_str) {
        Some("control_request")
            if value.pointer("/request/subtype").and_then(Value::as_str)
                == Some("can_use_tool") =>
        {
            let request = &value["request"];
            let Some(request_id) = value
                .get("request_id")
                .or_else(|| request.get("request_id"))
                .and_then(Value::as_str)
            else {
                return;
            };
            let tool = string_at(request, "tool_name", "tool");
            let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
            let id = next_question_id.fetch_add(1, Ordering::Relaxed);
            pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    id,
                    PendingResponse::ClaudePermission {
                        request_id: request_id.to_owned(),
                        input,
                    },
                );
            let _ = events.send(Event::Question(QuestionRequest {
                id,
                questions: vec![Question {
                    header: "Permission".to_owned(),
                    text: format!("Allow Claude Code to use {tool}?"),
                    choices: approval_choices(false),
                }],
            }));
            let _ = events.send(Event::Status("Waiting for answer".to_owned()));
        }
        Some("system") if value.get("subtype").and_then(Value::as_str) == Some("init") => {
            let _ = events.send(Event::Ready);
        }
        Some("stream_event") => {}
        Some("result") => {
            let _ = events.send(Event::TurnCompleted);
        }
        _ => {}
    }
}

fn string_at(value: &Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

fn approval_choices(include_session: bool) -> Vec<Choice> {
    let mut choices = vec![Choice {
        label: "Allow".to_owned(),
        description: "Allow this operation once.".to_owned(),
    }];
    if include_session {
        choices.push(Choice {
            label: "Always allow".to_owned(),
            description: "Allow similar operations for this session.".to_owned(),
        });
    }
    choices.push(Choice {
        label: "Deny".to_owned(),
        description: "Do not allow this operation.".to_owned(),
    });
    choices
}

fn pipe_stderr(
    stderr: Option<impl std::io::Read + Send + 'static>,
    events: mpsc::Sender<Event>,
    provider: Provider,
) {
    if let Some(stderr) = stderr {
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = events.send(Event::Error(format!("{}: {line}", provider.label())));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalization_state() -> (Mutex<HashMap<u64, PendingResponse>>, AtomicU64) {
        (Mutex::new(HashMap::new()), AtomicU64::new(1))
    }

    #[test]
    fn codex_deltas_normalize_to_assistant_text() {
        let (sender, receiver) = mpsc::channel();
        let (pending, next_id) = normalization_state();
        normalize_codex(
            &json!({
                "method": "item/agentMessage/delta",
                "params": { "delta": "hello" }
            }),
            &sender,
            &pending,
            &next_id,
        );

        assert!(matches!(
            receiver.recv().unwrap(),
            Event::Conversation(update)
                if matches!(*update, ConversationUpdate::AppendText { ref delta, .. } if delta == "hello")
        ));
    }

    #[test]
    fn codex_thread_name_updates_are_normalized() {
        let (sender, receiver) = mpsc::channel();
        let (pending, next_id) = normalization_state();
        normalize_codex(
            &json!({
                "method": "thread/name/updated",
                "params": {
                    "threadId": "thread-123",
                    "threadName": "Fix terminal rendering"
                }
            }),
            &sender,
            &pending,
            &next_id,
        );

        assert!(matches!(
            receiver.recv().unwrap(),
            Event::SessionNameChanged { id, name }
                if id == "thread-123" && name == "Fix terminal rendering"
        ));
    }

    #[test]
    fn claude_deltas_normalize_to_assistant_text() {
        let (sender, receiver) = mpsc::channel();
        let (pending, next_id) = normalization_state();
        normalize_claude(
            &json!({
                "type": "stream_event",
                "event": {
                    "type": "content_block_delta",
                    "delta": { "type": "text_delta", "text": "hello" }
                }
            }),
            &sender,
            &pending,
            &next_id,
        );

        assert!(matches!(
            receiver.recv().unwrap(),
            Event::Conversation(update)
                if matches!(*update, ConversationUpdate::AppendText { ref delta, .. } if delta == "hello")
        ));
    }

    #[test]
    fn codex_user_input_becomes_a_multiple_choice_question() {
        let (sender, receiver) = mpsc::channel();
        let (pending, next_id) = normalization_state();
        normalize_codex(
            &json!({
                "id": 42,
                "method": "item/tool/requestUserInput",
                "params": {
                    "questions": [{
                        "id": "strategy",
                        "header": "Approach",
                        "question": "Which approach?",
                        "options": [
                            { "label": "Safe", "description": "Prefer safety." },
                            { "label": "Fast", "description": "Prefer speed." }
                        ]
                    }]
                }
            }),
            &sender,
            &pending,
            &next_id,
        );

        let Event::Question(request) = receiver.recv().unwrap() else {
            panic!("expected a question event");
        };
        assert_eq!(request.questions[0].header, "Approach");
        assert_eq!(request.questions[0].choices[1].label, "Fast");
        assert!(pending.lock().unwrap().contains_key(&request.id));
    }

    #[test]
    fn codex_approval_becomes_allow_once_session_or_deny() {
        let (sender, receiver) = mpsc::channel();
        let (pending, next_id) = normalization_state();
        normalize_codex(
            &json!({
                "id": "approval-1",
                "method": "item/commandExecution/requestApproval",
                "params": { "command": "cargo publish" }
            }),
            &sender,
            &pending,
            &next_id,
        );

        let Event::Question(request) = receiver.recv().unwrap() else {
            panic!("expected a question event");
        };
        assert_eq!(request.questions[0].text, "cargo publish");
        assert_eq!(
            request.questions[0]
                .choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["Allow", "Always allow", "Deny"]
        );
    }

    #[test]
    fn claude_control_request_becomes_a_permission_question() {
        let (sender, receiver) = mpsc::channel();
        let (pending, next_id) = normalization_state();
        normalize_claude(
            &json!({
                "type": "control_request",
                "request_id": "permission-1",
                "request": {
                    "subtype": "can_use_tool",
                    "tool_name": "Bash",
                    "input": { "command": "cargo test" }
                }
            }),
            &sender,
            &pending,
            &next_id,
        );

        let Event::Question(request) = receiver.recv().unwrap() else {
            panic!("expected a question event");
        };
        assert!(request.questions[0].text.contains("Bash"));
        assert_eq!(request.questions[0].choices.len(), 2);
    }

    #[test]
    fn codex_resume_snapshot_becomes_ordered_message_history() {
        let history = CodexTranslator
            .import(&NativeArtifact::Json(json!({
                "result": {
                    "thread": {
                        "turns": [{
                            "items": [
                                {
                                    "type": "userMessage",
                                    "content": [
                                        { "type": "text", "text": "Fix the toolbar" },
                                        { "type": "image", "url": "data:image/png;base64,..." }
                                    ]
                                },
                                { "type": "agentMessage", "text": "I’ll inspect it." },
                                { "type": "commandExecution", "command": "cargo test" }
                            ]
                        }]
                    }
                }
            })))
            .unwrap()
            .conversation;

        assert_eq!(history.events.len(), 3);
        assert!(matches!(
            &history.events[0].payload,
            agency_translator_api::EventPayload::Message {
                role: agency_translator_api::MessageRole::User,
                ..
            }
        ));
        assert!(matches!(
            &history.events[1].payload,
            agency_translator_api::EventPayload::Message {
                role: agency_translator_api::MessageRole::Assistant,
                ..
            }
        ));
    }

    #[test]
    fn claude_native_jsonl_becomes_main_conversation_history() {
        let history = parse_claude_history(
            r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"Hello"}}
{"type":"assistant","isSidechain":false,"message":{"role":"assistant","content":[{"type":"text","text":"Hi there"},{"type":"tool_use","name":"Bash"}]}}
{"type":"assistant","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"hidden subagent"}]}}"#,
        );

        assert_eq!(history.events.len(), 3);
        assert!(matches!(
            &history.events[0].payload,
            agency_translator_api::EventPayload::Message {
                role: agency_translator_api::MessageRole::User,
                ..
            }
        ));
        assert!(matches!(
            &history.events[1].payload,
            agency_translator_api::EventPayload::Message {
                role: agency_translator_api::MessageRole::Assistant,
                ..
            }
        ));
    }
}
