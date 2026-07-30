mod config;
mod diffs;
mod keybindings;
mod sessions;
mod terminal;
mod ui_theme;
mod worktrees;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use iced::widget::{
    button, column, container, image, markdown, opaque, row, rule, scrollable, stack, svg, text,
};
use iced::{
    Border, Color, Element, Fill, Font, Length, Point, Size, Subscription, Task, Theme, time,
    window,
};
use iced::{event, keyboard};

use agency_agents::{
    Event as AgentEvent, Image as AgentImage, Provider, QuestionRequest, Session as AgentSession,
};
use agency_mux::{Multiplexer, Program};
use agency_translator_api::{
    ClientId, ContentBlock, Conversation, ConversationEvent, ConversationUpdate, EventPayload,
    MessageRole,
};
use config::{DefaultAgent, GlobalConfig, ModeColors, WindowState};
use diffs::{DiffLineKind, DiffSessionState, file_changes, renderable_diff_lines};
use keybindings::{Action, Activity, Keybindings, ModeIndicator};
use sessions::{SessionRegistry, name_from_prompt, new_conversation_id};
use terminal::TerminalSession;
use worktrees::Worktree;

const AGENT_TRANSCRIPT_ID: &str = "agent-transcript";
const DIFF_VIEW_ID: &str = "diff-view";
const PANEL_RIGHT_CLOSE_ICON: &[u8] = include_bytes!("../assets/icons/panel-right-close.svg");
const MESSAGE_SQUARE_ICON: &[u8] = include_bytes!("../assets/icons/message-square.svg");
const ARROW_RIGHT_ICON: &[u8] = include_bytes!("../assets/icons/arrow-right.svg");
const TRASH_ICON: &[u8] = include_bytes!("../assets/icons/trash-2.svg");
const FILE_ICON: &[u8] = include_bytes!("../assets/icons/file.svg");
const FOLDER_ICON: &[u8] = include_bytes!("../assets/icons/folder.svg");
const TERMINAL_ICON: &[u8] = include_bytes!("../assets/icons/terminal.svg");
const CHEVRON_RIGHT_ICON: &[u8] = include_bytes!("../assets/icons/chevron-right.svg");
const CHEVRON_DOWN_ICON: &[u8] = include_bytes!("../assets/icons/chevron-down.svg");
const MOUSE_CHASTISEMENT: &str = "Easy there, clicky—this is a keybindings establishment.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarTool {
    Sessions,
    Explorer,
}

#[derive(Debug, Clone)]
struct ExplorerEntry {
    path: PathBuf,
    depth: usize,
    directory: bool,
}

pub fn main() -> iced::Result {
    let window_state = WindowState::load().unwrap_or_default();
    let position = match (window_state.x, window_state.y) {
        (Some(x), Some(y)) => window::Position::Specific(Point::new(x, y)),
        _ => window::Position::Default,
    };

    iced::application(Agency::default, Agency::update, Agency::view)
        .title("Agency")
        .theme(Agency::theme)
        .subscription(Agency::subscription)
        .window(window::Settings {
            size: Size::new(window_state.width, window_state.height),
            position,
            maximized: window_state.maximized,
            fullscreen: window_state.fullscreen,
            min_size: Some(Size::new(800.0, 520.0)),
            exit_on_close_request: false,
            ..window::Settings::default()
        })
        .run()
}

struct Agency {
    keybindings: Keybindings,
    multiplexer: Multiplexer,
    terminals: Vec<TerminalSession>,
    active_terminal: Option<usize>,
    terminal_visible: bool,
    agents: Vec<AgentView>,
    active_agent: Option<usize>,
    sessions: SessionRegistry,
    toolbar_visible: bool,
    sidebar_tool: SidebarTool,
    explorer_selected: usize,
    explorer_expanded: HashSet<PathBuf>,
    selected_session: usize,
    pending_session_trash: Option<usize>,
    worktrees: Vec<Worktree>,
    active_worktree: usize,
    cwd: PathBuf,
    notice: Option<String>,
    mouse_notice_until: Option<Instant>,
    mode_colors: ModeColors,
    selected_agent: Provider,
    default_agent: Provider,
    cursor_visible: bool,
    cursor_blinked_at: Instant,
    transcript_scroll_target: Option<f32>,
}

struct AgentView {
    conversation_id: String,
    session: AgentSession,
    transcript: Vec<TranscriptEntry>,
    conversation: Conversation,
    prompt: String,
    prompt_selected: bool,
    images: Vec<AgentImage>,
    pending_question: Option<PendingQuestion>,
    status: String,
    session_id: Option<String>,
    pending_session_name: Option<String>,
    pending_conversation_id: Option<String>,
    completed_turns: u64,
    thinking_since: Option<Instant>,
    awaiting_agent_content: bool,
    queued_messages: VecDeque<QueuedMessage>,
    image_cache: HashMap<String, TranscriptImage>,
    image_cache_directory: PathBuf,
    diff_state: DiffSessionState,
    session_directory: PathBuf,
    last_changed_at_millis: u64,
}

enum TranscriptEntry {
    User {
        message: String,
        attachments: usize,
        images: Vec<TranscriptImage>,
    },
    Assistant {
        source: String,
        content: markdown::Content,
    },
    CommandExecution {
        source: String,
        content: markdown::Content,
    },
    Activity(String),
}

#[derive(Clone)]
struct TranscriptImage {
    data: Vec<u8>,
    handle: image::Handle,
}

impl TranscriptImage {
    fn new(data: Vec<u8>) -> Self {
        let handle =
            transcript_thumbnail(&data).unwrap_or_else(|| image::Handle::from_bytes(data.clone()));
        Self { data, handle }
    }
}

fn transcript_thumbnail(data: &[u8]) -> Option<image::Handle> {
    const MAX_WIDTH: u32 = 1_600;
    const MAX_HEIGHT: u32 = 480;

    let decoded = ::image::load_from_memory(data).ok()?;
    let thumbnail = decoded.thumbnail(MAX_WIDTH, MAX_HEIGHT).into_rgba8();
    let (width, height) = thumbnail.dimensions();

    Some(image::Handle::from_rgba(
        width,
        height,
        thumbnail.into_raw(),
    ))
}

struct PendingQuestion {
    request: QuestionRequest,
    current: usize,
    answers: Vec<usize>,
}

struct QueuedMessage {
    prompt: String,
    images: Vec<AgentImage>,
}

#[derive(Debug, Clone)]
enum Message {
    OpenRepository,
    LinkClicked(markdown::Uri),
    ToggleToolbar,
    ToggleActivity(SidebarTool),
    ToggleExplorerEntry(usize),
    SelectWorktree(usize),
    ResumeSession(usize),
    RequestSessionTrash(usize),
    CancelSessionTrash,
    ConfirmSessionTrash,
    AnswerChoice(usize),
    ToggleTerminalActivity,
    ToggleDiffActivity,
    SelectDiff(usize),
    ToggleFullscreen(window::Id, window::Mode),
    WindowEvent(window::Id, window::Event),
    WindowGeometryReady(window::Id, Size, Option<Point>, bool, window::Mode, bool),
    Keyboard(keyboard::Event),
    MouseClick,
    Tick(Instant),
}

impl Default for Agency {
    fn default() -> Self {
        let (config, notice) = match GlobalConfig::load() {
            Ok(config) => (config, None),
            Err(error) => (GlobalConfig::default(), Some(error)),
        };
        let mode_colors = ModeColors::from_config(&config.mode_colors);
        let default_agent = match config.default_agent {
            DefaultAgent::Codex => Provider::Codex,
            DefaultAgent::Claude => Provider::Claude,
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (worktrees, worktree_notice) = match worktrees::discover(&cwd) {
            Ok(worktrees) => (worktrees, None),
            Err(error) => (
                vec![Worktree {
                    label: cwd.file_name().map_or_else(
                        || cwd.display().to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    ),
                    path: cwd.clone(),
                    branch: None,
                }],
                Some(error),
            ),
        };
        let active_worktree = worktrees
            .iter()
            .position(|worktree| worktree.path == cwd)
            .unwrap_or(0);
        let cwd = worktrees[active_worktree].path.clone();
        let (sessions, session_notice) = match SessionRegistry::load(&cwd) {
            Ok(sessions) => (sessions, None),
            Err(error) => (SessionRegistry::empty(&cwd), Some(error)),
        };

        let mut agency = Self {
            keybindings: Keybindings::from_config(config.keybindings),
            multiplexer: Multiplexer::default(),
            terminals: Vec::new(),
            active_terminal: None,
            terminal_visible: false,
            agents: Vec::new(),
            active_agent: None,
            sessions,
            toolbar_visible: false,
            sidebar_tool: SidebarTool::Sessions,
            explorer_selected: 0,
            explorer_expanded: HashSet::new(),
            selected_session: 0,
            pending_session_trash: None,
            worktrees,
            active_worktree,
            cwd,
            notice: notice.or(worktree_notice).or(session_notice),
            mouse_notice_until: None,
            mode_colors,
            selected_agent: default_agent,
            default_agent,
            cursor_visible: true,
            cursor_blinked_at: Instant::now(),
            transcript_scroll_target: None,
        };
        let startup_notice = agency.notice.take();
        agency.start_agent(default_agent);
        if agency.notice.is_none() {
            agency.notice = startup_notice;
        }
        agency
    }
}

impl Agency {
    fn theme(&self) -> Theme {
        ui_theme::theme()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let transcript_len = self.active_agent().map_or(0, AgentView::transcript_len);

        match message {
            Message::OpenRepository => {}
            Message::LinkClicked(uri) => self.notice = Some(format!("Link: {uri}")),
            Message::ToggleToolbar => self.toolbar_visible = !self.toolbar_visible,
            Message::ToggleActivity(tool) => self.toggle_activity(tool),
            Message::ToggleExplorerEntry(index) => self.toggle_explorer_entry(index),
            Message::SelectWorktree(index) => self.select_worktree(index),
            Message::ResumeSession(index) => self.resume_session(index),
            Message::RequestSessionTrash(index) => self.request_session_trash(index),
            Message::CancelSessionTrash => self.pending_session_trash = None,
            Message::ConfirmSessionTrash => self.confirm_session_trash(),
            Message::AnswerChoice(choice) => {
                if let Some(agent) = self.active_agent_mut() {
                    agent.answer_choice(choice);
                }
            }
            Message::ToggleTerminalActivity => {
                self.keybindings.toggle_terminal_mode(self.terminal_visible);
                self.toggle_terminal();
            }
            Message::ToggleDiffActivity => self.activate_diff_activity(),
            Message::SelectDiff(index) => self.select_diff(index),
            Message::ToggleFullscreen(id, mode) => {
                let mode = match mode {
                    window::Mode::Fullscreen => window::Mode::Windowed,
                    window::Mode::Windowed | window::Mode::Hidden => window::Mode::Fullscreen,
                };
                return window::set_mode(id, mode);
            }
            Message::WindowEvent(id, event) => match event {
                window::Event::CloseRequested => return window_geometry(id, true),
                window::Event::Moved(_) | window::Event::Resized(_) => {
                    return window_geometry(id, false);
                }
                _ => {}
            },
            Message::WindowGeometryReady(id, size, position, maximized, mode, close_after) => {
                let state = WindowState {
                    width: size.width,
                    height: size.height,
                    x: position.map(|position| position.x),
                    y: position.map(|position| position.y),
                    maximized,
                    fullscreen: mode == window::Mode::Fullscreen,
                };
                if let Err(error) = state.save() {
                    self.notice = Some(error);
                }
                if close_after {
                    return window::close(id);
                }
            }
            Message::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                text,
                ..
            }) => {
                self.cursor_visible = true;
                self.cursor_blinked_at = Instant::now();
                if is_fullscreen_shortcut(&key, modifiers) {
                    return window::latest().then(|id| {
                        id.map_or_else(Task::none, |id| {
                            window::mode(id).map(move |mode| Message::ToggleFullscreen(id, mode))
                        })
                    });
                }
                if self.pending_session_trash.is_some() {
                    match key.as_ref() {
                        keyboard::Key::Named(keyboard::key::Named::Enter)
                            if modifiers.is_empty() =>
                        {
                            self.confirm_session_trash();
                        }
                        keyboard::Key::Named(keyboard::key::Named::Escape)
                            if modifiers.is_empty() =>
                        {
                            self.pending_session_trash = None;
                        }
                        _ => {}
                    }
                    return Task::none();
                }
                if self
                    .active_agent()
                    .is_some_and(|agent| agent.pending_question.is_some())
                    && let Some(choice) = numeric_choice(&key, physical_key, modifiers)
                {
                    if let Some(agent) = self.active_agent_mut() {
                        agent.answer_choice(choice);
                    }
                    return Task::none();
                }
                let composer_was_active = self.keybindings.is_composer_active();
                let action = self.keybindings.handle_with_diff(
                    &key,
                    physical_key,
                    modifiers,
                    text.as_deref(),
                    self.terminal_visible,
                    self.active_agent.is_some(),
                    self.toolbar_visible,
                    self.sidebar_tool == SidebarTool::Explorer,
                    self.active_agent()
                        .is_some_and(|agent| agent.diff_state.activity_visible),
                    self.active_agent()
                        .is_some_and(|agent| agent.diff_state.viewer_visible),
                );
                self.apply(action);
                if !composer_was_active && self.keybindings.is_composer_active() {
                    self.toolbar_visible = false;
                }
            }
            Message::Keyboard(_) => {}
            Message::MouseClick => {
                self.mouse_notice_until = Some(Instant::now() + Duration::from_secs(30));
            }
            Message::Tick(now) => {
                if self
                    .mouse_notice_until
                    .is_some_and(|deadline| now >= deadline)
                {
                    self.mouse_notice_until = None;
                }
                if now.duration_since(self.cursor_blinked_at) >= Duration::from_millis(500) {
                    self.cursor_visible = !self.cursor_visible;
                    self.cursor_blinked_at = now;
                }
                for terminal in &mut self.terminals {
                    terminal.poll();
                }
                let session_updates = self
                    .agents
                    .iter_mut()
                    .flat_map(AgentView::poll)
                    .collect::<Vec<_>>();
                for (provider, id, name, conversation_id) in session_updates {
                    let result = if let Some(conversation_id) = conversation_id {
                        self.sessions
                            .record_binding(conversation_id, provider, id, name)
                    } else {
                        self.sessions.record(provider, id, name)
                    };
                    if let Err(error) = result {
                        self.notice = Some(error);
                    }
                }
            }
        }

        let updated_transcript_len = self.active_agent().map_or(0, AgentView::transcript_len);

        if let Some(y) = self.transcript_scroll_target.take() {
            iced::widget::operation::snap_to(
                AGENT_TRANSCRIPT_ID,
                iced::widget::operation::RelativeOffset {
                    x: None,
                    y: Some(y),
                },
            )
        } else if updated_transcript_len > transcript_len {
            iced::widget::operation::snap_to_end(AGENT_TRANSCRIPT_ID)
        } else {
            Task::none()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            keyboard::listen().map(Message::Keyboard),
            event::listen_with(|event, _status, _window| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_)) => {
                    Some(Message::MouseClick)
                }
                _ => None,
            }),
            window::events().map(|(id, event)| Message::WindowEvent(id, event)),
            time::every(Duration::from_millis(16)).map(Message::Tick),
        ])
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::WorktreePrevious => {
                self.select_worktree(self.active_worktree.saturating_sub(1));
            }
            Action::WorktreeNext => {
                self.select_worktree(
                    self.active_worktree
                        .saturating_add(1)
                        .min(self.worktrees.len().saturating_sub(1)),
                );
            }
            Action::WorktreeSelect(index) => {
                if index < self.worktrees.len() {
                    self.select_worktree(index);
                }
            }
            Action::ToggleActivity(activity) => self.toggle_activity(match activity {
                Activity::Sessions => SidebarTool::Sessions,
                Activity::Explorer => SidebarTool::Explorer,
                Activity::Diffs => {
                    self.toggle_diff_activity();
                    return;
                }
            }),
            Action::NewSession => {
                self.selected_agent = self.default_agent;
                self.start_agent(self.default_agent);
            }
            Action::ToolbarPrevious => {
                let ordered = self.ordered_session_indices();
                let position = ordered
                    .iter()
                    .position(|index| *index == self.selected_session)
                    .unwrap_or_default();
                self.selected_session = ordered
                    .get(position.saturating_sub(1))
                    .copied()
                    .unwrap_or_default();
            }
            Action::ToolbarNext => {
                let ordered = self.ordered_session_indices();
                let position = ordered
                    .iter()
                    .position(|index| *index == self.selected_session)
                    .unwrap_or_default();
                self.selected_session = ordered
                    .get(position.saturating_add(1))
                    .copied()
                    .unwrap_or(self.selected_session);
            }
            Action::ToolbarFirst => {
                self.selected_session = self
                    .ordered_session_indices()
                    .first()
                    .copied()
                    .unwrap_or_default();
            }
            Action::ToolbarLast => {
                self.selected_session = self
                    .ordered_session_indices()
                    .last()
                    .copied()
                    .unwrap_or_default();
            }
            Action::ToolbarOpen => {
                if !self.sessions.records().is_empty() {
                    self.resume_session(self.selected_session);
                }
            }
            Action::ToolbarTrash => {
                if !self.sessions.records().is_empty() {
                    self.request_session_trash(self.selected_session);
                }
            }
            Action::ExplorerPrevious => {
                self.explorer_selected = self.explorer_selected.saturating_sub(1);
            }
            Action::ExplorerNext => {
                self.explorer_selected = self
                    .explorer_selected
                    .saturating_add(1)
                    .min(self.explorer_entries().len().saturating_sub(1));
            }
            Action::ExplorerCollapse => self.collapse_explorer_entry(),
            Action::ExplorerExpand => self.expand_explorer_entry(),
            Action::ExplorerOpen => self.toggle_selected_explorer_entry(),
            Action::DiffPrevious => self.update_diff_state(|state| {
                state.selected = state.selected.saturating_sub(1);
            }),
            Action::DiffNext => self.update_diff_state(|state| {
                state.selected = state
                    .selected
                    .saturating_add(1)
                    .min(state.artifacts.len().saturating_sub(1));
            }),
            Action::DiffFirst => self.update_diff_state(|state| state.selected = 0),
            Action::DiffLast => self.update_diff_state(|state| {
                state.selected = state.artifacts.len().saturating_sub(1);
            }),
            Action::DiffOpen => self.update_diff_state(|state| {
                if !state.artifacts.is_empty() {
                    state.viewer_visible = true;
                    state.viewer_scroll = 0;
                }
            }),
            Action::DiffScrollUp => self.scroll_diff(false),
            Action::DiffScrollDown => self.scroll_diff(true),
            Action::DiffClose => {
                self.update_diff_state(|state| state.viewer_visible = false);
            }
            Action::DiffJumpToTool => {
                if let Some(agent) = self.active_agent()
                    && let Some(artifact) =
                        agent.diff_state.artifacts.get(agent.diff_state.selected)
                {
                    self.transcript_scroll_target = Some(
                        artifact.transcript_index as f32
                            / agent.transcript.len().saturating_sub(1).max(1) as f32,
                    );
                }
            }
            Action::ToggleTerminal => {
                self.toggle_terminal();
            }
            Action::AgentAppend(text) => {
                if let Some(agent) = self.active_agent_mut() {
                    if agent.prompt_selected {
                        agent.prompt.clear();
                        agent.prompt_selected = false;
                    }
                    agent.prompt.push_str(&text);
                }
            }
            Action::AgentBackspace => {
                if let Some(agent) = self.active_agent_mut() {
                    if agent.prompt_selected {
                        agent.prompt.clear();
                        agent.prompt_selected = false;
                    } else {
                        agent.prompt.pop();
                    }
                }
            }
            Action::AgentPaste => self.paste_into_agent(),
            Action::AgentSelectAll => {
                if let Some(agent) = self.active_agent_mut() {
                    agent.prompt_selected = !agent.prompt.is_empty();
                }
            }
            Action::AgentSubmit => {
                let submitted = self.active_agent_mut().and_then(AgentView::submit);
                if let Some((provider, id, name)) = submitted
                    && let Err(error) = self.sessions.name_if_missing(provider, &id, name)
                {
                    self.notice = Some(error);
                }
            }
            Action::TerminalInput(bytes) => {
                if let Some(terminal) = self.active_terminal() {
                    terminal.send(bytes);
                }
            }
        }
    }

    fn toggle_activity(&mut self, tool: SidebarTool) {
        (self.sidebar_tool, self.toolbar_visible) =
            toggled_activity(self.sidebar_tool, self.toolbar_visible, tool);
    }

    fn toggle_terminal(&mut self) {
        if self.terminal_visible {
            self.terminal_visible = false;
        } else if self.active_terminal.is_some() {
            self.terminal_visible = true;
        } else {
            self.start_terminal(Program::Shell);
        }
    }

    fn update_diff_state(&mut self, update: impl FnOnce(&mut DiffSessionState)) {
        let Some(agent) = self.active_agent_mut() else {
            return;
        };
        update(&mut agent.diff_state);
        if let Err(error) = agent.diff_state.save(&agent.session_directory) {
            self.notice = Some(error);
        }
    }

    fn toggle_diff_activity(&mut self) {
        self.update_diff_state(|state| {
            state.activity_visible = !state.activity_visible;
        });
    }

    fn activate_diff_activity(&mut self) {
        if self.terminal_visible {
            self.keybindings.toggle_terminal_mode(true);
            self.terminal_visible = false;
            if self
                .active_agent()
                .is_some_and(|agent| !agent.diff_state.activity_visible)
            {
                self.toggle_diff_activity();
            }
        } else {
            self.toggle_diff_activity();
        }
    }

    fn select_diff(&mut self, index: usize) {
        self.update_diff_state(|state| {
            if index < state.artifacts.len() {
                state.selected = index;
            }
        });
    }

    fn scroll_diff(&mut self, down: bool) {
        self.update_diff_state(|state| {
            state.viewer_scroll = if down {
                state.viewer_scroll.saturating_add(80)
            } else {
                state.viewer_scroll.saturating_sub(80)
            };
        });
    }

    fn select_worktree(&mut self, index: usize) {
        let Some(worktree) = self.worktrees.get(index) else {
            return;
        };
        if index == self.active_worktree {
            return;
        }

        let cwd = worktree.path.clone();
        let sessions = match SessionRegistry::load(&cwd) {
            Ok(sessions) => sessions,
            Err(error) => {
                self.notice = Some(error);
                return;
            }
        };

        self.active_worktree = index;
        self.cwd = cwd;
        self.sessions = sessions;
        self.agents.clear();
        self.active_agent = None;
        self.terminal_visible = false;
        self.active_terminal = self
            .terminals
            .iter()
            .position(|terminal| terminal.cwd() == self.cwd);
        self.explorer_selected = 0;
        self.explorer_expanded.clear();
        self.selected_session = 0;
        self.pending_session_trash = None;
        self.notice = Some(format!("Switched to {}", self.cwd.display()));
    }

    fn explorer_entries(&self) -> Vec<ExplorerEntry> {
        let mut entries = Vec::new();
        collect_explorer_entries(&self.cwd, 0, &self.explorer_expanded, &mut entries);
        entries
    }

    fn toggle_explorer_entry(&mut self, index: usize) {
        self.explorer_selected = index;
        self.toggle_selected_explorer_entry();
    }

    fn expand_explorer_entry(&mut self) {
        let Some(entry) = self.explorer_entries().get(self.explorer_selected).cloned() else {
            return;
        };
        if entry.directory {
            self.explorer_expanded.insert(entry.path);
        }
    }

    fn toggle_selected_explorer_entry(&mut self) {
        let Some(entry) = self.explorer_entries().get(self.explorer_selected).cloned() else {
            return;
        };
        if entry.directory && !self.explorer_expanded.remove(&entry.path) {
            self.explorer_expanded.insert(entry.path);
        }
    }

    fn collapse_explorer_entry(&mut self) {
        let entries = self.explorer_entries();
        let Some(entry) = entries.get(self.explorer_selected) else {
            return;
        };
        if entry.directory && self.explorer_expanded.remove(&entry.path) {
            return;
        }
        if let Some(parent) = entry.path.parent()
            && let Some(index) = entries
                .iter()
                .position(|candidate| candidate.path == parent)
        {
            self.explorer_selected = index;
        }
    }

    fn start_terminal(&mut self, program: Program) {
        match TerminalSession::spawn(&mut self.multiplexer, program, &self.cwd) {
            Ok(terminal) => {
                self.terminals.push(terminal);
                self.active_terminal = Some(self.terminals.len() - 1);
                self.terminal_visible = true;
                self.notice = None;
            }
            Err(error) => {
                self.terminal_visible = false;
                self.notice = Some(error);
            }
        }
    }

    fn start_agent(&mut self, provider: Provider) {
        match AgentSession::spawn(provider, &self.cwd) {
            Ok(session) => {
                let conversation_id = new_conversation_id();
                let session_directory = self.sessions.session_directory(&conversation_id);
                let diff_state = DiffSessionState::load(&session_directory).unwrap_or_default();
                self.agents.push(AgentView {
                    conversation_id: conversation_id.clone(),
                    session,
                    transcript: Vec::new(),
                    conversation: Conversation::default(),
                    prompt: String::new(),
                    prompt_selected: false,
                    images: Vec::new(),
                    pending_question: None,
                    status: "Initializing".to_owned(),
                    session_id: None,
                    pending_session_name: None,
                    pending_conversation_id: Some(conversation_id.clone()),
                    completed_turns: 0,
                    thinking_since: None,
                    awaiting_agent_content: false,
                    queued_messages: VecDeque::new(),
                    image_cache: HashMap::new(),
                    image_cache_directory: self
                        .sessions
                        .session_directory(&conversation_id)
                        .join("images"),
                    diff_state,
                    session_directory,
                    last_changed_at_millis: unix_time_millis(),
                });
                self.active_agent = Some(self.agents.len() - 1);
                self.terminal_visible = false;
                self.notice = None;
            }
            Err(error) => {
                self.notice = Some(error);
            }
        }
    }

    fn resume_session(&mut self, index: usize) {
        let Some(record) = self.sessions.records().get(index).cloned() else {
            return;
        };
        if let Some(running_index) = self
            .agents
            .iter()
            .position(|agent| agent.conversation_id == record.conversation_id)
        {
            self.selected_session = index;
            self.selected_agent = self.agents[running_index].session.provider();
            self.active_agent = Some(running_index);
            self.terminal_visible = false;
            self.notice = None;
            return;
        }
        let (provider, id) = if let Some(id) = record.binding(self.selected_agent) {
            (self.selected_agent, id.to_owned())
        } else if let Some(id) = record.binding(next_provider(self.selected_agent)) {
            (next_provider(self.selected_agent), id.to_owned())
        } else {
            self.notice = Some("This Agency session has no agent bindings".to_owned());
            return;
        };
        self.selected_agent = provider;
        match AgentSession::resume(provider, &id, &self.cwd) {
            Ok(session) => {
                self.selected_session = index;
                let session_directory = self.sessions.session_directory(record.conversation_id());
                let diff_state = match DiffSessionState::load(&session_directory) {
                    Ok(state) => state,
                    Err(error) => {
                        self.notice = Some(error);
                        DiffSessionState::default()
                    }
                };
                self.agents.push(AgentView {
                    conversation_id: record.conversation_id.clone(),
                    session,
                    transcript: Vec::new(),
                    conversation: Conversation::default(),
                    prompt: String::new(),
                    prompt_selected: false,
                    images: Vec::new(),
                    pending_question: None,
                    status: "Resuming".to_owned(),
                    session_id: Some(id),
                    pending_session_name: None,
                    pending_conversation_id: None,
                    completed_turns: 0,
                    thinking_since: None,
                    awaiting_agent_content: false,
                    queued_messages: VecDeque::new(),
                    image_cache: HashMap::new(),
                    image_cache_directory: self
                        .sessions
                        .session_directory(record.conversation_id())
                        .join("images"),
                    diff_state,
                    session_directory,
                    last_changed_at_millis: unix_time_millis(),
                });
                self.active_agent = Some(self.agents.len() - 1);
                self.terminal_visible = false;
                self.notice = None;
            }
            Err(error) => self.notice = Some(error),
        }
    }

    fn request_session_trash(&mut self, index: usize) {
        if index < self.sessions.records().len() {
            self.selected_session = index;
            self.pending_session_trash = Some(index);
        }
    }

    fn confirm_session_trash(&mut self) {
        let Some(index) = self.pending_session_trash.take() else {
            return;
        };
        match self.sessions.remove(index) {
            Ok(record) => {
                if let Some(running_index) = self
                    .agents
                    .iter()
                    .position(|agent| agent.conversation_id == record.conversation_id)
                {
                    self.agents.remove(running_index);
                    self.active_agent = match self.active_agent {
                        Some(active) if active == running_index => {
                            (!self.agents.is_empty()).then_some(active.min(self.agents.len() - 1))
                        }
                        Some(active) if active > running_index => Some(active - 1),
                        active => active,
                    };
                }
                self.selected_session = index.min(self.sessions.records().len().saturating_sub(1));
                self.notice = Some(format!(
                    "Trashed session {}",
                    record.name.as_deref().unwrap_or(record.conversation_id())
                ));
            }
            Err(error) => {
                self.pending_session_trash = Some(index);
                self.notice = Some(error);
            }
        }
    }

    fn active_terminal(&self) -> Option<&TerminalSession> {
        self.active_terminal
            .and_then(|index| self.terminals.get(index))
    }

    fn active_agent(&self) -> Option<&AgentView> {
        self.active_agent.and_then(|index| self.agents.get(index))
    }

    fn active_agent_mut(&mut self) -> Option<&mut AgentView> {
        self.active_agent
            .and_then(|index| self.agents.get_mut(index))
    }

    fn ordered_session_indices(&self) -> Vec<usize> {
        let active_conversation_id = self
            .active_agent()
            .map(|agent| agent.conversation_id.as_str());
        let mut indices = (0..self.sessions.records().len()).collect::<Vec<_>>();
        indices.sort_by_key(|index| {
            let session = &self.sessions.records()[*index];
            let agent = self
                .agents
                .iter()
                .find(|agent| agent.conversation_id == session.conversation_id);
            let rank = if agent.is_some_and(|agent| agent.pending_question.is_some()) {
                0
            } else if active_conversation_id == Some(session.conversation_id()) {
                1
            } else if agent.is_some() {
                2
            } else {
                3
            };
            let changed = agent
                .map(|agent| agent.last_changed_at_millis)
                .unwrap_or(session.updated_at_millis);
            (rank, std::cmp::Reverse(changed))
        });
        indices
    }

    fn paste_into_agent(&mut self) {
        let Some(agent) = self.active_agent_mut() else {
            return;
        };

        let mut clipboard = match arboard::Clipboard::new() {
            Ok(clipboard) => clipboard,
            Err(error) => {
                self.notice = Some(format!("Could not access clipboard: {error}"));
                return;
            }
        };

        match clipboard.get_image() {
            Ok(image) => match encode_png(image.width as u32, image.height as u32, &image.bytes) {
                Ok(data) => {
                    agent.images.push(AgentImage {
                        media_type: "image/png".to_owned(),
                        data,
                    });
                    self.notice = Some(format!(
                        "Attached clipboard image ({} × {})",
                        image.width, image.height
                    ));
                }
                Err(error) => self.notice = Some(error),
            },
            Err(_) => match clipboard.get_text() {
                Ok(text) => {
                    if agent.prompt_selected {
                        agent.prompt.clear();
                        agent.prompt_selected = false;
                    }
                    agent.prompt.push_str(&text);
                }
                Err(error) => {
                    self.notice = Some(format!("Clipboard has no pasteable content: {error}"))
                }
            },
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let leader_pending = self.keybindings.is_leader_pending();
        let worktree_tabs =
            self.worktrees
                .iter()
                .enumerate()
                .fold(row![].spacing(4), |tabs, (index, worktree)| {
                    let selected = index == self.active_worktree;
                    let label: Element<'_, Message> = if leader_pending && index < 10 {
                        let shortcut = if index == 9 {
                            "0".to_owned()
                        } else {
                            (index + 1).to_string()
                        };
                        row![shortcut_badge(shortcut), text(&worktree.label).size(12),]
                            .align_y(iced::Alignment::Center)
                            .spacing(6)
                            .into()
                    } else {
                        text(&worktree.label).size(12).into()
                    };
                    tabs.push(
                        button(label)
                            .padding([5, 10])
                            .style(move |_theme: &Theme, status| {
                                ui_theme::worktree_tab(selected, status)
                            })
                            .on_press_maybe((!selected).then_some(Message::SelectWorktree(index))),
                    )
                });
        let tab_bar = container(
            row![
                text("Worktrees:").size(12),
                scrollable(worktree_tabs).direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new()
                )),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(10),
        )
        .width(Fill)
        .padding([5, 10])
        .style(|_theme: &Theme| ui_theme::tab_bar());
        let active_conversation_id = self
            .active_agent()
            .map(|agent| agent.conversation_id.as_str());
        let session_buttons = self.ordered_session_indices().into_iter().fold(
            column![].spacing(8),
            |sessions, index| {
                let session = &self.sessions.records()[index];
                let display_id = session
                    .binding(self.default_agent)
                    .or_else(|| session.binding(next_provider(self.default_agent)))
                    .unwrap_or(session.conversation_id());
                let short_id = display_id.chars().take(12).collect::<String>();
                let id = if display_id.chars().count() > 12 {
                    format!("{short_id}…")
                } else {
                    short_id
                };
                let is_active = active_conversation_id == Some(session.conversation_id());
                let is_running = self
                    .agents
                    .iter()
                    .any(|agent| agent.conversation_id == session.conversation_id);
                let waiting = self.agents.iter().any(|agent| {
                    agent.conversation_id == session.conversation_id
                        && agent.pending_question.is_some()
                });
                let (state, badge_status) = if waiting {
                    ("WAITING FOR INPUT", ui_theme::SessionStatus::Waiting)
                } else if is_active {
                    ("ACTIVE", ui_theme::SessionStatus::Active)
                } else if is_running {
                    ("RUNNING", ui_theme::SessionStatus::Running)
                } else {
                    ("RESUME", ui_theme::SessionStatus::Resume)
                };
                let name = session.name.as_deref().unwrap_or("Untitled session");
                let badge = container(text(state).size(10))
                    .padding([2, 5])
                    .style(move |_theme: &Theme| ui_theme::session_status_badge(badge_status));
                let card =
                    button(column![badge, text(name).size(13), text(id).size(10),].spacing(3))
                        .width(Fill)
                        .padding(iced::Padding::from([8, 10]).right(38))
                        .style(move |_theme: &Theme, status| {
                            ui_theme::session_button(index == self.selected_session, status)
                        });
                let card = if is_active {
                    card
                } else {
                    card.on_press(Message::ResumeSession(index))
                };
                let trash = container(
                    button(
                        svg(svg::Handle::from_memory(TRASH_ICON))
                            .width(Length::Fixed(14.0))
                            .height(Length::Fixed(14.0))
                            .style(|_theme: &Theme, _status| ui_theme::danger_icon()),
                    )
                    .padding(5)
                    .style(|_theme: &Theme, status| ui_theme::trash_button(status))
                    .on_press(Message::RequestSessionTrash(index)),
                )
                .padding(6)
                .align_right(Fill)
                .align_top(Fill);
                sessions.push(stack![card, trash])
            },
        );
        let tool_button = |tool, icon: &'static [u8], hint: String| {
            let selected = self.toolbar_visible && self.sidebar_tool == tool;
            let control = button(
                svg(svg::Handle::from_memory(icon))
                    .width(Length::Fixed(19.0))
                    .height(Length::Fixed(19.0))
                    .style(move |_theme: &Theme, _status| ui_theme::tool_icon(selected)),
            )
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(36.0))
            .padding(8)
            .style(move |_theme: &Theme, status| ui_theme::tool_button(selected, status))
            .on_press(Message::ToggleActivity(tool));
            let control: Element<'_, Message> = control.into();
            if leader_pending {
                stack![
                    control,
                    container(shortcut_badge(hint))
                        .padding([2, 3])
                        .align_right(Fill)
                        .align_bottom(Fill),
                ]
                .into()
            } else {
                control
            }
        };
        let terminal_selected = self.terminal_visible;
        let terminal_control = button(
            svg(svg::Handle::from_memory(TERMINAL_ICON))
                .width(Length::Fixed(19.0))
                .height(Length::Fixed(19.0))
                .style(move |_theme: &Theme, _status| ui_theme::tool_icon(terminal_selected)),
        )
        .width(Length::Fixed(36.0))
        .height(Length::Fixed(36.0))
        .padding(8)
        .style(move |_theme: &Theme, status| ui_theme::tool_button(terminal_selected, status))
        .on_press(Message::ToggleTerminalActivity);
        let terminal_control: Element<'_, Message> = if leader_pending {
            stack![
                terminal_control,
                container(shortcut_badge(
                    self.keybindings.toggle_terminal_hint().to_owned()
                ))
                .padding([2, 3])
                .align_right(Fill)
                .align_bottom(Fill),
            ]
            .into()
        } else {
            terminal_control.into()
        };

        // The left activity bar contains worktree-scoped tools only.
        let worktree_activity_bar = container(
            column![
                tool_button(
                    SidebarTool::Sessions,
                    MESSAGE_SQUARE_ICON,
                    self.keybindings.show_sessions_hint().to_owned(),
                ),
                tool_button(
                    SidebarTool::Explorer,
                    FOLDER_ICON,
                    self.keybindings.show_explorer_hint().to_owned(),
                ),
                terminal_control,
            ]
            .spacing(4)
            .align_x(iced::Alignment::Center),
        )
        .width(Length::Fixed(48.0))
        .height(Fill)
        .padding([8, 6])
        .style(|_theme: &Theme| ui_theme::activity_bar());

        let panel: Element<'_, Message> = match (self.toolbar_visible, self.sidebar_tool) {
            (false, _) => iced::widget::Space::new().width(Length::Shrink).into(),
            (true, SidebarTool::Sessions) => container(
                column![
                    sidebar_header("SESSIONS", MESSAGE_SQUARE_ICON),
                    rule::horizontal(1),
                    scrollable(session_buttons).height(Fill),
                    text("↑/↓ or j/k select · Enter open · d trash · <leader>s").size(10),
                ]
                .spacing(14),
            )
            .width(Length::Fixed(260.0))
            .height(Fill)
            .padding(16)
            .style(|_theme: &Theme| ui_theme::rail())
            .into(),
            (true, SidebarTool::Explorer) => {
                let entries = self.explorer_entries();
                let project_name = self.cwd.file_name().map_or_else(
                    || self.cwd.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                let tree = entries.iter().enumerate().fold(
                    column![].spacing(0),
                    |tree, (index, entry)| {
                        let is_directory = entry.directory;
                        let name = entry.path.file_name().map_or_else(
                            || entry.path.display().to_string(),
                            |name| name.to_string_lossy().into_owned(),
                        );
                        let indent = (0..entry.depth).fold(row![].spacing(0), |indent, _| {
                            indent.push(
                                container(rule::vertical(1))
                                    .width(Length::Fixed(13.0))
                                    .height(Length::Fixed(26.0))
                                    .center_x(Length::Fixed(13.0)),
                            )
                        });
                        let disclosure: Element<'_, Message> = if is_directory {
                            let icon = if self.explorer_expanded.contains(&entry.path) {
                                CHEVRON_DOWN_ICON
                            } else {
                                CHEVRON_RIGHT_ICON
                            };
                            svg(svg::Handle::from_memory(icon))
                                .width(Length::Fixed(13.0))
                                .height(Length::Fixed(13.0))
                                .style(|_theme: &Theme, _status| ui_theme::disclosure_icon())
                                .into()
                        } else {
                            iced::widget::Space::new()
                                .width(Length::Fixed(13.0))
                                .height(Length::Fixed(13.0))
                                .into()
                        };
                        let icon = if is_directory { FOLDER_ICON } else { FILE_ICON };
                        tree.push(
                            button(
                                row![
                                    indent,
                                    disclosure,
                                    svg(svg::Handle::from_memory(icon))
                                        .width(Length::Fixed(15.0))
                                        .height(Length::Fixed(15.0))
                                        .style(move |_theme: &Theme, _status| {
                                            ui_theme::tree_item_icon(is_directory)
                                        }),
                                    text(name).size(12).width(Fill),
                                ]
                                .align_y(iced::Alignment::Center)
                                .spacing(6),
                            )
                            .width(Fill)
                            .padding([5, 7])
                            .style(move |_theme: &Theme, status| {
                                ui_theme::file_entry(index == self.explorer_selected, status)
                            })
                            .on_press(Message::ToggleExplorerEntry(index)),
                        )
                    },
                );
                container(
                    column![
                        sidebar_header("EXPLORER", FOLDER_ICON),
                        rule::horizontal(1),
                        container(
                            row![
                                svg(svg::Handle::from_memory(FOLDER_ICON))
                                    .width(Length::Fixed(15.0))
                                    .height(Length::Fixed(15.0))
                                    .style(|_theme: &Theme, _status| {
                                        ui_theme::tree_item_icon(true)
                                    }),
                                text(project_name).size(11).width(Fill),
                                text(entries.len().to_string()).size(10),
                            ]
                            .spacing(7)
                            .align_y(iced::Alignment::Center),
                        )
                        .padding([7, 8])
                        .style(|_theme: &Theme| ui_theme::tree_root()),
                        scrollable(tree).height(Fill),
                        text("j/k navigate  ·  h/l fold  ·  Enter open").size(10),
                    ]
                    .spacing(9),
                )
                .width(Length::Fixed(280.0))
                .height(Fill)
                .padding([14, 12])
                .style(|_theme: &Theme| ui_theme::rail())
                .into()
            }
        };
        let rail_divider: Element<'_, Message> = if self.toolbar_visible {
            rule::vertical(1).into()
        } else {
            iced::widget::Space::new().width(Length::Shrink).into()
        };
        let toolbar: Element<'_, Message> = row![worktree_activity_bar, rail_divider, panel]
            .height(Fill)
            .into();

        let content: Element<'_, Message> = if let Some(agent) = self.active_agent() {
            let input_active = self.keybindings.is_composer_active();
            let header = row![
                text(&agent.status).size(12),
                text(
                    agent
                        .session_id
                        .as_deref()
                        .unwrap_or("discovering session ID")
                )
                .size(12),
                text(self.cwd.display().to_string()).size(12),
                iced::widget::Space::new().width(Fill),
            ]
            .align_y(iced::Alignment::Center)
            .spacing(16);
            let transcript = if agent.transcript.is_empty() {
                column![text("Waiting for agent…").size(15)].width(Fill)
            } else {
                agent
                    .transcript
                    .iter()
                    .fold(column![].spacing(12), |transcript, entry| match entry {
                        TranscriptEntry::User {
                            message,
                            attachments,
                            images,
                        } => {
                            let mut quoted = message.to_owned();
                            for _ in 0..*attachments {
                                if !quoted.is_empty() {
                                    quoted.push('\n');
                                }
                                quoted.push_str("   [attachment]");
                            }
                            let message_content = images.iter().fold(
                                column![
                                    text(quoted)
                                        .size(15)
                                        .width(Fill)
                                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                                ]
                                .spacing(10),
                                |content, transcript_image| {
                                    content.push(
                                        image(transcript_image.handle.clone())
                                            .width(Fill)
                                            .height(Length::Fixed(240.0))
                                            .content_fit(iced::ContentFit::Contain),
                                    )
                                },
                            );
                            transcript.push(
                                row![
                                    container(
                                        svg(svg::Handle::from_memory(ARROW_RIGHT_ICON))
                                            .width(Length::Fixed(19.0))
                                            .height(Length::Fixed(19.0))
                                            .style(|_theme: &Theme, _status| {
                                                ui_theme::user_arrow()
                                            })
                                    )
                                    .padding([11, 0]),
                                    container(message_content)
                                        .padding([10, 12])
                                        .width(Fill)
                                        .style(|_theme: &Theme| ui_theme::user_message()),
                                ]
                                .spacing(14)
                                .align_y(iced::Alignment::Start)
                                .width(Fill),
                            )
                        }
                        TranscriptEntry::Assistant { content, .. } => transcript.push(
                            container(
                                row![
                                    text("•")
                                        .size(21)
                                        .color(ui_theme::SUCCESS)
                                        .width(Length::Fixed(18.0)),
                                    markdown::view(content.items(), ui_theme::markdown_settings(),)
                                        .map(Message::LinkClicked),
                                ]
                                .spacing(10)
                                .align_y(iced::Alignment::Start),
                            )
                            .padding([4, 4]),
                        ),
                        TranscriptEntry::CommandExecution { content, .. } => transcript.push(
                            container(
                                column![
                                    text("Executing command:").size(13),
                                    markdown::view(content.items(), ui_theme::markdown_settings(),)
                                        .map(Message::LinkClicked),
                                ]
                                .spacing(6),
                            )
                            .padding([4, 32]),
                        ),
                        TranscriptEntry::Activity(message) => transcript
                            .push(container(text(message).size(12).width(Fill)).padding([2, 4])),
                    })
                    .width(Fill)
            };
            let cursor_visible = input_active
                && self.cursor_visible
                && (agent.prompt.is_empty() || !agent.prompt_selected);
            let cursor = container(text(" ").font(Font::MONOSPACE).size(14))
                .width(Length::Fixed(8.0))
                .height(Length::Fixed(17.0))
                .style(move |_theme: &Theme| {
                    if cursor_visible {
                        ui_theme::block_cursor()
                    } else {
                        container::Style::default()
                    }
                });
            let prompt_text = container(text(&agent.prompt).font(Font::MONOSPACE).size(14)).style(
                move |_theme: &Theme| {
                    if agent.prompt_selected {
                        ui_theme::text_selection()
                    } else {
                        container::Style::default()
                    }
                },
            );
            let prompt = if agent.prompt.is_empty() {
                row![
                    cursor,
                    text("Type a message and press Enter")
                        .font(Font::MONOSPACE)
                        .size(14),
                ]
                .spacing(2)
            } else {
                row![prompt_text, cursor].spacing(0)
            };
            let input_indicator: Element<'_, Message> = if let Some(thinking_since) = agent
                .thinking_since
                .filter(|_| agent.awaiting_agent_content)
            {
                let dots =
                    ".".repeat((thinking_since.elapsed().as_millis() / 350 % 3 + 1) as usize);
                row![
                    text("ACTIVE").size(11).color(ui_theme::SUCCESS),
                    text("•").size(11),
                    container(text(format!("THINKING{dots}")).size(11))
                        .padding([3, 8])
                        .style(|_theme: &Theme| ui_theme::thinking_badge()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into()
            } else if agent.awaiting_agent_content {
                text("ACTIVE").size(11).color(ui_theme::SUCCESS).into()
            } else {
                text("IDLE").size(11).into()
            };
            let mut input_details = Vec::new();
            if !agent.images.is_empty() {
                input_details.push(format!(
                    "{} image{} attached",
                    agent.images.len(),
                    if agent.images.len() == 1 { "" } else { "s" }
                ));
            }
            if !agent.queued_messages.is_empty() {
                input_details.push(format!(
                    "{} message{} queued",
                    agent.queued_messages.len(),
                    if agent.queued_messages.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
            }
            let input_details = input_details.join("  •  ");
            let question = agent.pending_question.as_ref().and_then(|pending| {
                pending
                    .request
                    .questions
                    .get(pending.current)
                    .map(|question| {
                        let choices = question.choices.iter().enumerate().fold(
                            column![].spacing(6),
                            |choices, (index, choice)| {
                                choices.push(
                                    button(
                                        column![
                                            text(format!("{}. {}", index + 1, choice.label))
                                                .size(14),
                                            text(&choice.description).size(12),
                                        ]
                                        .spacing(2),
                                    )
                                    .on_press(Message::AnswerChoice(index))
                                    .width(Fill),
                                )
                            },
                        );
                        container(
                            column![
                                text(format!(
                                    "{}  •  QUESTION {} OF {}",
                                    question.header.to_uppercase(),
                                    pending.current + 1,
                                    pending.request.questions.len()
                                ))
                                .size(11),
                                text(&question.text).size(15),
                                choices,
                                text("Press 1–9 or select an option").size(11),
                            ]
                            .spacing(8),
                        )
                        .padding(16)
                    })
            });

            let mut content = column![
                container(header).padding([10, 16]),
                rule::horizontal(1),
                container(
                    scrollable(transcript)
                        .id(AGENT_TRANSCRIPT_ID)
                        .width(Fill)
                        .height(Fill),
                )
                .width(Fill)
                .padding(24)
                .height(Fill),
                rule::horizontal(1),
            ];
            if let Some(question) = question {
                content = content.push(question).push(rule::horizontal(1));
            }
            let composer: Element<'_, Message> = container(
                column![input_indicator, text(input_details).size(12), prompt,].spacing(8),
            )
            .padding(16)
            .width(Fill)
            .style(move |theme: &Theme| {
                let palette = theme.extended_palette();
                let border_color = if input_active {
                    palette.primary.strong.color
                } else {
                    palette.background.strong.color
                };

                container::Style::default()
                    .background(palette.background.weak.color)
                    .border(Border {
                        width: if input_active { 2.0 } else { 1.0 },
                        radius: 0.0.into(),
                        color: border_color,
                    })
            })
            .into();
            let composer = if leader_pending {
                stack![
                    composer,
                    container(shortcut_badge(
                        self.keybindings.enter_active_view_hint().to_owned()
                    ))
                    .padding(8)
                    .align_right(Fill)
                    .align_top(Fill),
                ]
                .into()
            } else {
                composer
            };
            content.push(composer).width(Fill).height(Fill).into()
        } else {
            container(
                column![
                    text("Start with a repository").size(28),
                    text("Open a local Git repository to create and manage agent workspaces.")
                        .size(15),
                    button("Open repository").on_press(Message::OpenRepository),
                    text("Press <Space> t to open a terminal here.").size(13),
                    text("<Space> n starts a new session").size(13),
                ]
                .spacing(16),
            )
            .center_x(Fill)
            .center_y(Fill)
            .width(Fill)
            .height(Fill)
            .into()
        };

        let terminal_view: Element<'_, Message> = if self.terminal_visible {
            let terminal = self
                .active_terminal()
                .expect("a visible terminal must have a session");
            let header = row![
                text(terminal.program().label()).size(14),
                text(terminal.cwd().display().to_string()).size(12),
                text(terminal.status()).size(12),
            ]
            .spacing(16);

            column![
                container(header).padding([10, 16]),
                rule::horizontal(1),
                container(
                    scrollable(
                        text(terminal.screen())
                            .font(Font::MONOSPACE)
                            .size(14)
                            .width(Fill),
                    )
                    .height(Fill),
                )
                .padding(16)
                .height(Fill),
            ]
            .width(Fill)
            .height(Fill)
            .into()
        } else {
            iced::widget::Space::new().width(Length::Shrink).into()
        };
        let (diff_viewer, diff_activity, diff_control): (
            Element<'_, Message>,
            Element<'_, Message>,
            Element<'_, Message>,
        ) = if let Some(agent) = self.active_agent() {
            let state = &agent.diff_state;
            let viewer: Element<'_, Message> = if state.viewer_visible && !self.terminal_visible {
                let artifact = state.artifacts.get(state.selected);
                let title = artifact
                    .map(|artifact| artifact.title.clone())
                    .unwrap_or_else(|| "DIFF".to_owned());
                let description = artifact
                    .map(|artifact| artifact.description.clone())
                    .unwrap_or_else(|| "No diff selected".to_owned());
                let lines = artifact
                    .map(|artifact| rich_diff(&artifact.diff, (state.viewer_scroll / 20) as usize))
                    .unwrap_or_else(|| text("No diff selected").into());
                column![
                    container(
                        row![
                            svg(svg::Handle::from_memory(FILE_ICON))
                                .width(Length::Fixed(16.0))
                                .height(Length::Fixed(16.0))
                                .style(|_theme: &Theme, _status| ui_theme::icon()),
                            column![text(title).size(13), text(description).size(11)]
                                .spacing(2)
                                .width(Fill),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                    )
                    .padding([10, 12])
                    .width(Fill)
                    .style(|_theme: &Theme| ui_theme::status_bar()),
                    rule::horizontal(1),
                    scrollable(lines).id(DIFF_VIEW_ID).height(Fill).direction(
                        iced::widget::scrollable::Direction::Both {
                            vertical: iced::widget::scrollable::Scrollbar::default(),
                            horizontal: iced::widget::scrollable::Scrollbar::default(),
                        }
                    ),
                    rule::horizontal(1),
                    container(text("j/k scroll  ·  Enter jump  ·  Ctrl+C close").size(10))
                        .padding([7, 12])
                        .width(Fill)
                        .style(|_theme: &Theme| ui_theme::status_bar()),
                ]
                .width(Fill)
                .height(Fill)
                .into()
            } else {
                iced::widget::Space::new().width(Length::Shrink).into()
            };
            let artifacts = state.artifacts.iter().enumerate().fold(
                column![].spacing(8),
                |items, (index, artifact)| {
                    items.push(
                        button(
                            column![
                                text(&artifact.title).size(12),
                                text(&artifact.description).size(10),
                            ]
                            .spacing(3),
                        )
                        .width(Fill)
                        .padding([8, 10])
                        .style(move |_theme: &Theme, status| {
                            ui_theme::session_button(index == state.selected, status)
                        })
                        .on_press(Message::SelectDiff(index)),
                    )
                },
            );
            let activity: Element<'_, Message> = if state.activity_visible && !self.terminal_visible
            {
                container(
                    column![
                        diff_sidebar_header(),
                        rule::horizontal(1),
                        scrollable(artifacts).height(Fill),
                        text("j/k select  ·  Enter open  ·  <leader>d").size(10),
                    ]
                    .spacing(12),
                )
                .width(Length::Fixed(260.0))
                .height(Fill)
                .padding(14)
                .style(|_theme: &Theme| ui_theme::rail())
                .into()
            } else {
                iced::widget::Space::new().width(Length::Shrink).into()
            };
            let selected = state.activity_visible && !self.terminal_visible;
            let control = button(
                svg(svg::Handle::from_memory(FILE_ICON))
                    .width(Length::Fixed(19.0))
                    .height(Length::Fixed(19.0))
                    .style(move |_theme: &Theme, _status| ui_theme::tool_icon(selected)),
            )
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(36.0))
            .padding(8)
            .style(move |_theme: &Theme, status| ui_theme::tool_button(selected, status))
            .on_press(Message::ToggleDiffActivity);
            let count_badge = container(diff_count_badge(state.artifacts.len()))
                .padding([1, 2])
                .align_right(Fill)
                .align_top(Fill);
            let control: Element<'_, Message> = if leader_pending {
                stack![
                    control,
                    count_badge,
                    container(shortcut_badge(
                        self.keybindings.show_diffs_hint().to_owned()
                    ))
                    .padding([2, 3])
                    .align_right(Fill)
                    .align_bottom(Fill),
                ]
                .into()
            } else {
                stack![control, count_badge].into()
            };
            (viewer, activity, control)
        } else {
            (
                iced::widget::Space::new().width(Length::Shrink).into(),
                iced::widget::Space::new().width(Length::Shrink).into(),
                iced::widget::Space::new().width(Length::Shrink).into(),
            )
        };
        // The right activity bar contains active-session-scoped tools only.
        let session_activity_bar = container(
            column![diff_control]
                .spacing(4)
                .align_x(iced::Alignment::Center),
        )
        .width(Length::Fixed(48.0))
        .height(Fill)
        .padding([8, 6])
        .style(|_theme: &Theme| ui_theme::rail());

        let diff_viewer_visible = self
            .active_agent()
            .is_some_and(|agent| agent.diff_state.viewer_visible)
            && !self.terminal_visible;
        let diff_activity_visible = self
            .active_agent()
            .is_some_and(|agent| agent.diff_state.activity_visible)
            && !self.terminal_visible;
        let terminal_pane = right_pane(terminal_view, self.terminal_visible, Fill);
        let diff_viewer_pane = right_pane(diff_viewer, diff_viewer_visible, Fill);
        let diff_activity_pane =
            right_pane(diff_activity, diff_activity_visible, Length::Fixed(261.0));
        let right_activity_pane =
            right_pane(session_activity_bar.into(), true, Length::Fixed(49.0));

        let indicator = self.keybindings.mode_indicator();
        let indicator_color = match indicator {
            ModeIndicator::Normal => self.mode_colors.normal,
            ModeIndicator::Terminal => self.mode_colors.terminal,
            ModeIndicator::Composer => self.mode_colors.agent,
            ModeIndicator::Leader => self.mode_colors.leader,
        };
        let indicator_text_color = contrasting_text(indicator_color);
        let mode_indicator = container(text(indicator.label()).size(12))
            .padding([3, 8])
            .style(move |_theme: &Theme| {
                container::Style::default()
                    .color(indicator_text_color)
                    .background(indicator_color)
                    .border(Border {
                        radius: 4.0.into(),
                        ..Border::default()
                    })
            });
        let agent_indicator =
            container(text(format!("AGENT {}", self.selected_agent.label())).size(12))
                .padding([3, 8])
                .style(|_theme: &Theme| ui_theme::agent_badge());

        let mouse_notice_active = self.mouse_notice_until.is_some();
        let status_notice = if mouse_notice_active {
            MOUSE_CHASTISEMENT
        } else {
            self.notice.as_deref().unwrap_or("Esc returns to NORMAL")
        };
        let status = row![
            mode_indicator,
            agent_indicator,
            text(self.cwd.display().to_string()).size(12),
            text(status_notice)
                .size(12)
                .color_maybe(mouse_notice_active.then_some(ui_theme::DANGER)),
        ]
        .spacing(24);

        let application: Element<'_, Message> = column![
            tab_bar,
            rule::horizontal(1),
            row![
                toolbar,
                rule::vertical(1),
                content,
                terminal_pane,
                diff_viewer_pane,
                diff_activity_pane,
                right_activity_pane
            ]
            .width(Fill)
            .height(Fill),
            rule::horizontal(1),
            container(status)
                .width(Fill)
                .padding([7, 12])
                .style(|_theme: &Theme| ui_theme::status_bar()),
        ]
        .into();

        let Some(index) = self.pending_session_trash else {
            return application;
        };
        let Some(session) = self.sessions.records().get(index) else {
            return application;
        };
        let name = session.name.as_deref().unwrap_or("Untitled session");
        let dialog = container(
            column![
                text("Trash session?").size(20),
                text(format!(
                    "“{name}” will be removed from this workspace’s session list."
                ))
                .size(14),
                text("Provider-owned session history will not be deleted.").size(12),
                row![
                    button("Cancel")
                        .style(|_theme: &Theme, status| { ui_theme::dialog_button(false, status) })
                        .on_press(Message::CancelSessionTrash),
                    button("Trash session")
                        .style(|_theme: &Theme, status| { ui_theme::dialog_button(true, status) })
                        .on_press(Message::ConfirmSessionTrash),
                ]
                .spacing(10),
                text("Enter confirms  ·  Esc cancels").size(11),
            ]
            .spacing(14),
        )
        .width(Length::Fixed(440.0))
        .padding(22)
        .style(|_theme: &Theme| ui_theme::modal());
        let overlay = opaque(
            container(dialog)
                .center_x(Fill)
                .center_y(Fill)
                .width(Fill)
                .height(Fill)
                .style(|_theme: &Theme| ui_theme::modal_backdrop()),
        );

        stack![application, overlay].into()
    }
}

fn contrasting_text(background: Color) -> Color {
    let luminance = 0.299 * background.r + 0.587 * background.g + 0.114 * background.b;
    if luminance > 0.6 {
        ui_theme::DARK_TEXT
    } else {
        ui_theme::TEXT
    }
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("Could not encode clipboard image: {error}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| format!("Could not encode clipboard image: {error}"))?;
    }
    Ok(output)
}

fn toggled_activity(
    current: SidebarTool,
    visible: bool,
    requested: SidebarTool,
) -> (SidebarTool, bool) {
    if visible && current == requested {
        (current, false)
    } else {
        (requested, true)
    }
}

fn numeric_choice(
    key: &keyboard::Key,
    physical_key: keyboard::key::Physical,
    modifiers: keyboard::Modifiers,
) -> Option<usize> {
    if !modifiers.is_empty() {
        return None;
    }
    let character = match key.as_ref() {
        keyboard::Key::Character(character) => character,
        _ => return None,
    };
    character
        .parse::<usize>()
        .ok()
        .filter(|choice| (1..=9).contains(choice))
        .map(|choice| choice - 1)
        .or(match physical_key {
            keyboard::key::Physical::Code(code) => match code {
                keyboard::key::Code::Digit1 => Some(0),
                keyboard::key::Code::Digit2 => Some(1),
                keyboard::key::Code::Digit3 => Some(2),
                keyboard::key::Code::Digit4 => Some(3),
                keyboard::key::Code::Digit5 => Some(4),
                keyboard::key::Code::Digit6 => Some(5),
                keyboard::key::Code::Digit7 => Some(6),
                keyboard::key::Code::Digit8 => Some(7),
                keyboard::key::Code::Digit9 => Some(8),
                _ => None,
            },
            _ => None,
        })
}

#[cfg(test)]
mod activity_tests {
    use super::{SidebarTool, toggled_activity};

    #[test]
    fn requesting_the_open_activity_closes_it() {
        assert_eq!(
            toggled_activity(SidebarTool::Sessions, true, SidebarTool::Sessions),
            (SidebarTool::Sessions, false)
        );
    }

    #[test]
    fn requesting_another_activity_switches_to_it() {
        assert_eq!(
            toggled_activity(SidebarTool::Sessions, true, SidebarTool::Explorer),
            (SidebarTool::Explorer, true)
        );
    }
}

fn sidebar_header(label: &'static str, icon: &'static [u8]) -> Element<'static, Message> {
    row![
        svg(svg::Handle::from_memory(icon))
            .width(Length::Fixed(17.0))
            .height(Length::Fixed(17.0))
            .style(|_theme: &Theme, _status| ui_theme::icon()),
        text(label).size(11),
        iced::widget::Space::new().width(Fill),
        button(
            svg(svg::Handle::from_memory(PANEL_RIGHT_CLOSE_ICON))
                .width(Length::Fixed(16.0))
                .height(Length::Fixed(16.0))
                .style(|_theme: &Theme, _status| ui_theme::icon())
        )
        .padding(5)
        .style(|_theme: &Theme, status| ui_theme::icon_button(status))
        .on_press(Message::ToggleToolbar),
    ]
    .align_y(iced::Alignment::Center)
    .spacing(8)
    .into()
}

fn diff_sidebar_header() -> Element<'static, Message> {
    row![
        svg(svg::Handle::from_memory(FILE_ICON))
            .width(Length::Fixed(17.0))
            .height(Length::Fixed(17.0))
            .style(|_theme: &Theme, _status| ui_theme::icon()),
        text("DIFFS").size(11),
        iced::widget::Space::new().width(Fill),
        button(
            svg(svg::Handle::from_memory(PANEL_RIGHT_CLOSE_ICON))
                .width(Length::Fixed(16.0))
                .height(Length::Fixed(16.0))
                .style(|_theme: &Theme, _status| ui_theme::icon())
        )
        .padding(5)
        .style(|_theme: &Theme, status| ui_theme::icon_button(status))
        .on_press(Message::ToggleDiffActivity),
    ]
    .align_y(iced::Alignment::Center)
    .spacing(8)
    .into()
}

fn rich_diff(diff: &str, skipped: usize) -> Element<'static, Message> {
    let lines = renderable_diff_lines(diff)
        .into_iter()
        .filter(|line| line.kind != DiffLineKind::Metadata)
        .collect::<Vec<_>>();
    let width = (lines
        .iter()
        .map(|line| line.content.chars().count())
        .max()
        .unwrap_or_default() as f32
        * 7.5
        + 110.0)
        .max(900.0);

    lines
        .into_iter()
        .skip(skipped)
        .fold(column![].spacing(0), |lines, line| {
            let marker = match line.kind {
                DiffLineKind::Addition => "+",
                DiffLineKind::Deletion => "−",
                DiffLineKind::Hunk => "◆",
                DiffLineKind::Metadata => "·",
                DiffLineKind::Context => " ",
            };
            let number = |number: Option<usize>| {
                container(
                    text(number.map_or_else(String::new, |number| number.to_string()))
                        .font(Font::MONOSPACE)
                        .size(11),
                )
                .width(Length::Fixed(40.0))
                .padding([3, 6])
                .align_x(iced::alignment::Horizontal::Right)
                .style(|_theme: &Theme| ui_theme::diff_gutter())
            };
            let content = row![
                number(line.old_number),
                number(line.new_number),
                container(text(marker).font(Font::MONOSPACE).size(12))
                    .width(Length::Fixed(22.0))
                    .padding([3, 6]),
                container(
                    text(line.content)
                        .font(Font::MONOSPACE)
                        .size(12)
                        .wrapping(iced::widget::text::Wrapping::None),
                )
                .padding([3, 6])
                .width(Fill),
            ]
            .align_y(iced::Alignment::Center);

            lines.push(
                container(content)
                    .width(Length::Fixed(width))
                    .style(move |_theme: &Theme| ui_theme::diff_line(line.kind)),
            )
        })
        .width(Length::Fixed(width))
        .into()
}

fn right_pane<'a>(
    content: Element<'a, Message>,
    visible: bool,
    width: impl Into<Length>,
) -> Element<'a, Message> {
    if visible {
        row![rule::vertical(1), content]
            .width(width)
            .height(Fill)
            .into()
    } else {
        iced::widget::Space::new().width(Length::Shrink).into()
    }
}

fn shortcut_badge(key: String) -> Element<'static, Message> {
    container(text(key).font(Font::MONOSPACE).size(11))
        .padding([2, 5])
        .style(|_theme: &Theme| ui_theme::shortcut_badge())
        .into()
}

fn diff_count_badge(count: usize) -> Element<'static, Message> {
    container(text(count.to_string()).font(Font::MONOSPACE).size(10))
        .padding([1, 4])
        .style(|_theme: &Theme| ui_theme::counter_badge())
        .into()
}

fn collect_explorer_entries(
    directory: &std::path::Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    entries: &mut Vec<ExplorerEntry>,
) {
    let Ok(children) = fs::read_dir(directory) else {
        return;
    };
    let mut children = children.filter_map(Result::ok).collect::<Vec<_>>();
    children.sort_by(|left, right| {
        let left_directory = left.file_type().is_ok_and(|kind| kind.is_dir());
        let right_directory = right.file_type().is_ok_and(|kind| kind.is_dir());
        right_directory.cmp(&left_directory).then_with(|| {
            left.file_name()
                .to_string_lossy()
                .to_lowercase()
                .cmp(&right.file_name().to_string_lossy().to_lowercase())
        })
    });
    for child in children {
        let path = child.path();
        let directory = child.file_type().is_ok_and(|kind| kind.is_dir());
        entries.push(ExplorerEntry {
            path: path.clone(),
            depth,
            directory,
        });
        if directory && expanded.contains(&path) {
            collect_explorer_entries(&path, depth + 1, expanded, entries);
        }
    }
}

fn window_geometry(id: window::Id, close_after: bool) -> Task<Message> {
    window::size(id).then(move |size| {
        window::position(id).then(move |position| {
            window::is_maximized(id).then(move |maximized| {
                window::mode(id).map(move |mode| {
                    Message::WindowGeometryReady(id, size, position, maximized, mode, close_after)
                })
            })
        })
    })
}

fn is_fullscreen_shortcut(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    modifiers == keyboard::Modifiers::ALT
        && matches!(
            key.as_ref(),
            keyboard::Key::Named(keyboard::key::Named::Enter)
        )
}

fn next_provider(provider: Provider) -> Provider {
    match provider {
        Provider::Codex => Provider::Claude,
        Provider::Claude => Provider::Codex,
    }
}

impl AgentView {
    fn poll(&mut self) -> Vec<(Provider, String, Option<String>, Option<String>)> {
        let mut updates = Vec::new();
        let events = self.session.try_events().collect::<Vec<_>>();
        if !events.is_empty() {
            self.last_changed_at_millis = unix_time_millis();
        }
        for event in events {
            match event {
                AgentEvent::SessionStarted { id, .. } => {
                    self.session_id = Some(id.clone());
                    let name = self.pending_session_name.take();
                    let conversation_id = self.pending_conversation_id.take();
                    if name.is_some() || conversation_id.is_some() {
                        updates.push((self.session.provider(), id, name, conversation_id));
                    }
                }
                AgentEvent::SessionNameChanged { id, name } => {
                    updates.push((self.session.provider(), id, Some(name), None));
                }
                AgentEvent::Ready => {
                    self.awaiting_agent_content = false;
                    self.status = "Ready".to_owned();
                }
                AgentEvent::ConversationReset(conversation) => {
                    self.thinking_since = None;
                    self.awaiting_agent_content = false;
                    self.conversation = conversation;
                    self.rebuild_transcript();
                }
                AgentEvent::Conversation(update) => self.apply_conversation_update(*update),
                AgentEvent::Status(status) => self.status = status,
                AgentEvent::Question(request) => {
                    self.awaiting_agent_content = false;
                    self.status = "Waiting for answer".to_owned();
                    self.pending_question = Some(PendingQuestion {
                        request,
                        current: 0,
                        answers: Vec::new(),
                    });
                }
                AgentEvent::Error(error) => {
                    self.thinking_since = None;
                    self.awaiting_agent_content = false;
                    self.status = "Error".to_owned();
                    self.transcript
                        .push(TranscriptEntry::Activity(format!("[error] {error}")));
                }
                AgentEvent::TurnCompleted => {
                    self.thinking_since = None;
                    self.awaiting_agent_content = false;
                    self.completed_turns = self.completed_turns.saturating_add(1);
                    self.status = "Ready".to_owned();
                }
            }
        }
        self.flush_queued_message();
        updates
    }

    fn apply_conversation_update(&mut self, update: ConversationUpdate) {
        match update {
            ConversationUpdate::Append { mut event } => {
                if let EventPayload::ToolCall { name, input, .. } = &event.payload {
                    self.thinking_since = if name.eq_ignore_ascii_case("reasoning") {
                        Some(Instant::now())
                    } else {
                        None
                    };
                    if self
                        .diff_state
                        .capture(&event.id, input, self.transcript.len())
                    {
                        let _ = self.diff_state.save(&self.session_directory);
                    }
                }
                if event.parent_id.is_none() {
                    event.parent_id = self
                        .conversation
                        .events
                        .last()
                        .map(|event| event.id.clone());
                }
                self.conversation.events.push(event);
            }
            ConversationUpdate::AppendText {
                mut event_id,
                source,
                delta,
                native,
            } => {
                if event_id.ends_with("live-assistant") {
                    event_id = format!("{event_id}-turn-{}", self.completed_turns);
                }
                if let Some(event) = self
                    .conversation
                    .events
                    .iter_mut()
                    .find(|event| event.id == event_id)
                    && let EventPayload::Message { content, .. } = &mut event.payload
                    && let Some(ContentBlock::Text { text }) = content.first_mut()
                {
                    text.push_str(&delta);
                } else {
                    self.conversation.events.push(ConversationEvent {
                        id: event_id,
                        parent_id: self
                            .conversation
                            .events
                            .last()
                            .map(|event| event.id.clone()),
                        turn_id: None,
                        source,
                        payload: EventPayload::Message {
                            role: MessageRole::Assistant,
                            content: vec![ContentBlock::Text { text: delta }],
                        },
                        native,
                    });
                }
            }
        }
        self.rebuild_transcript();
    }

    fn rebuild_transcript(&mut self) {
        let mut transcript_index = 0;
        let mut captured = false;
        for event in &self.conversation.events {
            if let EventPayload::ToolCall { input, .. } = &event.payload {
                captured |= self.diff_state.capture(&event.id, input, transcript_index);
            }
            if !matches!(
                &event.payload,
                EventPayload::ToolCall { name, .. } if name.eq_ignore_ascii_case("reasoning")
            ) {
                transcript_index += 1;
            }
        }
        if captured {
            let _ = self.diff_state.save(&self.session_directory);
        }
        let cache = &mut self.image_cache;
        let cache_directory = &self.image_cache_directory;
        self.transcript = self
            .conversation
            .events
            .iter()
            .filter_map(|event| transcript_entry(event, cache, cache_directory))
            .collect();
    }

    fn answer_choice(&mut self, choice: usize) {
        let Some(pending) = &mut self.pending_question else {
            return;
        };
        let Some(question) = pending.request.questions.get(pending.current) else {
            return;
        };
        let Some(selected) = question.choices.get(choice) else {
            return;
        };

        self.transcript.push(TranscriptEntry::Activity(format!(
            "[answer] {}: {}",
            question.header, selected.label
        )));
        pending.answers.push(choice);
        pending.current += 1;

        if pending.current < pending.request.questions.len() {
            return;
        }

        let request_id = pending.request.id;
        let answers = std::mem::take(&mut pending.answers);
        match self.session.answer(request_id, answers) {
            Ok(()) => {
                self.pending_question = None;
                self.awaiting_agent_content = true;
                self.status = "Working".to_owned();
            }
            Err(error) => {
                self.awaiting_agent_content = false;
                self.status = "Error".to_owned();
                self.transcript
                    .push(TranscriptEntry::Activity(format!("[error] {error}")));
            }
        }
    }

    fn submit(&mut self) -> Option<(Provider, String, String)> {
        self.prompt_selected = false;
        let prompt = self.prompt.trim().to_owned();
        if prompt.is_empty() && self.images.is_empty() {
            return None;
        }
        let suggested_name = self.session_id.as_ref().and_then(|id| {
            name_from_prompt(&prompt).map(|name| (self.session.provider(), id.clone(), name))
        });
        if self.session_id.is_none() {
            self.pending_session_name = name_from_prompt(&prompt);
        }

        let images = std::mem::take(&mut self.images);
        self.prompt.clear();
        self.queued_messages
            .push_back(QueuedMessage { prompt, images });
        self.flush_queued_message();
        suggested_name
    }

    fn flush_queued_message(&mut self) {
        if self.awaiting_agent_content || self.pending_question.is_some() {
            return;
        }
        let Some(queued) = self.queued_messages.pop_front() else {
            return;
        };
        let mut content = vec![ContentBlock::Text {
            text: queued.prompt.clone(),
        }];
        content.extend(queued.images.iter().map(|image| ContentBlock::Image {
            media_type: Some(image.media_type.clone()),
            data: STANDARD.encode(&image.data),
        }));
        self.conversation.events.push(ConversationEvent {
            id: format!("agency-user-{}", self.conversation.events.len() + 1),
            parent_id: self
                .conversation
                .events
                .last()
                .map(|event| event.id.clone()),
            turn_id: None,
            source: match self.session.provider() {
                Provider::Codex => ClientId::new("codex"),
                Provider::Claude => ClientId::new("claude-code"),
            },
            payload: EventPayload::Message {
                role: MessageRole::User,
                content,
            },
            native: None,
        });
        self.rebuild_transcript();
        match self.session.send(queued.prompt, queued.images) {
            Ok(()) => {
                self.awaiting_agent_content = true;
                self.status = "Working".to_owned();
            }
            Err(error) => {
                self.awaiting_agent_content = false;
                self.status = "Error".to_owned();
                self.transcript
                    .push(TranscriptEntry::Activity(format!("[error] {error}")));
            }
        }
    }

    fn transcript_len(&self) -> usize {
        self.transcript
            .iter()
            .map(|entry| match entry {
                TranscriptEntry::User {
                    message,
                    attachments,
                    images,
                } => {
                    message.len()
                        + attachments
                        + images.iter().map(|image| image.data.len()).sum::<usize>()
                }
                TranscriptEntry::Assistant { source, .. } => source.len(),
                TranscriptEntry::CommandExecution { source, .. } => source.len(),
                TranscriptEntry::Activity(message) => message.len(),
            })
            .sum()
    }
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

impl TranscriptEntry {
    fn assistant(source: String) -> Self {
        let content = markdown::Content::parse(&source);
        Self::Assistant { source, content }
    }

    fn command_execution(command: String) -> Self {
        let source = fenced_command(&command, command_language());
        let content = markdown::Content::parse(&source);
        Self::CommandExecution { source, content }
    }
}

fn transcript_entry(
    event: &ConversationEvent,
    image_cache: &mut HashMap<String, TranscriptImage>,
    image_cache_directory: &std::path::Path,
) -> Option<TranscriptEntry> {
    match &event.payload {
        EventPayload::Message { role, content } => {
            let text = content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let images = content
                .iter()
                .enumerate()
                .filter_map(|(index, block)| match block {
                    ContentBlock::Image { data, .. } => cached_transcript_image(
                        image_cache,
                        image_cache_directory,
                        &event.id,
                        index,
                        data,
                    ),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let attachments = content
                .iter()
                .filter(|block| matches!(block, ContentBlock::Attachment { .. }))
                .count();
            match role {
                MessageRole::User => Some(TranscriptEntry::User {
                    message: text,
                    attachments,
                    images,
                }),
                MessageRole::Assistant => Some(TranscriptEntry::assistant(text)),
                MessageRole::System | MessageRole::Developer => {
                    Some(TranscriptEntry::Activity(format!("[context] {text}")))
                }
            }
        }
        EventPayload::ToolCall { name, input, .. } => {
            if name.eq_ignore_ascii_case("reasoning") {
                return None;
            }
            if input.get("type").and_then(serde_json::Value::as_str) == Some("fileChange") {
                let status = input
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("completed");
                let changes = file_changes(input);
                let message = if changes.is_empty() {
                    format!("[file change {status}] No file changes were applied")
                } else {
                    changes
                        .iter()
                        .map(|change| format!("[file] {} — {}", change.path, change.description))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                return Some(TranscriptEntry::Activity(message));
            }
            let kind = event
                .native
                .as_ref()
                .and_then(|native| native.raw.pointer("/params/item/type"))
                .and_then(serde_json::Value::as_str);
            if kind == Some("commandExecution") {
                Some(TranscriptEntry::command_execution(name.clone()))
            } else {
                Some(TranscriptEntry::Activity(format!(
                    "[tool] {name} {}",
                    compact_json(input)
                )))
            }
        }
        EventPayload::ToolResult {
            content, is_error, ..
        } => {
            let text = content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            Some(TranscriptEntry::Activity(format!(
                "[tool {}] {text}",
                if *is_error { "error" } else { "result" }
            )))
        }
        EventPayload::Summary { text } => {
            Some(TranscriptEntry::Activity(format!("[summary] {text}")))
        }
    }
}

fn cached_transcript_image(
    cache: &mut HashMap<String, TranscriptImage>,
    cache_directory: &std::path::Path,
    event_id: &str,
    index: usize,
    encoded: &str,
) -> Option<TranscriptImage> {
    let key = format!("{event_id}:{index}");
    if let Some(image) = cache.get(&key) {
        return Some(image.clone());
    }

    let filename = format!("{}-{index}.image", safe_filename(event_id));
    let path = cache_directory.join(filename);
    let data = fs::read(&path)
        .ok()
        .or_else(|| STANDARD.decode(encoded).ok())?;
    let image = TranscriptImage::new(data.clone());
    cache.insert(key, image.clone());

    if !path.exists() && fs::create_dir_all(cache_directory).is_ok() {
        let _ = fs::write(path, data);
    }
    Some(image)
}

fn safe_filename(value: &str) -> String {
    let encoded = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if encoded.is_empty() {
        "event".to_owned()
    } else {
        encoded
    }
}

fn compact_json(value: &serde_json::Value) -> String {
    let value = value.to_string();
    const MAX_CHARS: usize = 160;
    if value.chars().count() <= MAX_CHARS {
        value
    } else {
        format!("{}…", value.chars().take(MAX_CHARS - 1).collect::<String>())
    }
}

fn command_language() -> &'static str {
    if cfg!(windows) { "powershell" } else { "bash" }
}

fn fenced_command(command: &str, language: &str) -> String {
    let longest_run = command
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(3.max(longest_run + 1));
    format!("{fence}{language}\n{command}\n{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_rgba_is_encoded_as_png() {
        let encoded = encode_png(1, 1, &[0xff, 0x00, 0x00, 0xff]).unwrap();

        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn assistant_content_parses_github_flavored_markdown() {
        let TranscriptEntry::Assistant { content, .. } = TranscriptEntry::assistant(
            "## Heading\n\n- [x] done\n- [ ] todo\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```"
                .to_owned(),
        ) else {
            unreachable!();
        };

        assert!(
            content
                .items()
                .iter()
                .any(|item| matches!(item, markdown::Item::List { .. }))
        );
        assert!(
            content
                .items()
                .iter()
                .any(|item| matches!(item, markdown::Item::Table { .. }))
        );
        assert!(
            content
                .items()
                .iter()
                .any(|item| matches!(item, markdown::Item::CodeBlock { .. }))
        );
    }

    #[test]
    fn command_execution_uses_a_safe_platform_fence() {
        let rendered = fenced_command("echo ```nested```", "bash");

        assert!(rendered.starts_with("````bash\n"));
        assert!(rendered.ends_with("\n````"));
        assert!(rendered.contains("echo ```nested```"));
    }

    #[test]
    fn reasoning_tool_calls_are_hidden_from_the_transcript() {
        let event = ConversationEvent {
            id: "reasoning-1".to_owned(),
            parent_id: None,
            turn_id: None,
            source: ClientId::new("codex"),
            payload: EventPayload::ToolCall {
                id: Some("reasoning-1".to_owned()),
                name: "reasoning".to_owned(),
                input: serde_json::json!({}),
            },
            native: None,
        };

        assert!(transcript_entry(&event, &mut HashMap::new(), &std::env::temp_dir()).is_none());
    }

    #[test]
    fn user_images_are_retained_for_transcript_rendering() {
        let png = encode_png(1, 1, &[0xff, 0x00, 0x00, 0xff]).unwrap();
        let event = ConversationEvent {
            id: "user-image-1".to_owned(),
            parent_id: None,
            turn_id: None,
            source: ClientId::new("agency"),
            payload: EventPayload::Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Image {
                    media_type: Some("image/png".to_owned()),
                    data: STANDARD.encode(&png),
                }],
            },
            native: None,
        };

        let cache_directory =
            std::env::temp_dir().join(format!("agency-image-cache-{}", std::process::id()));
        let mut cache = HashMap::new();
        let Some(TranscriptEntry::User { images, .. }) =
            transcript_entry(&event, &mut cache, &cache_directory)
        else {
            unreachable!();
        };
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].data, png);
        assert_eq!(cache.len(), 1);
        assert!(cache_directory.join("user-image-1-0.image").is_file());
        std::fs::remove_dir_all(cache_directory).unwrap();
    }

    #[test]
    fn alt_enter_is_the_fullscreen_shortcut() {
        let enter = keyboard::Key::Named(keyboard::key::Named::Enter);

        assert!(is_fullscreen_shortcut(&enter, keyboard::Modifiers::ALT));
        assert!(!is_fullscreen_shortcut(
            &enter,
            keyboard::Modifiers::ALT | keyboard::Modifiers::SHIFT
        ));
        assert!(!is_fullscreen_shortcut(
            &enter,
            keyboard::Modifiers::empty()
        ));
    }

    #[test]
    fn provider_fallback_cycles_between_supported_backends() {
        assert_eq!(next_provider(Provider::Codex), Provider::Claude);
        assert_eq!(next_provider(Provider::Claude), Provider::Codex);
    }
}
