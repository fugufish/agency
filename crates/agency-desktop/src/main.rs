mod config;
mod diffs;
mod file_viewer;
mod keybindings;
mod plugins;
mod sessions;
mod slash_commands;
mod terminal;
mod ui_theme;
mod workspaces;
mod worktrees;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use iced::widget::{
    button, column, container, image, markdown, opaque, rich_text, row, rule, scrollable, span,
    stack, svg, text,
};
use iced::{
    Border, Color, Element, Fill, Font, Length, Padding, Point, Size, Subscription, Task, Theme,
    time, window,
};

// Runtime and terminal events are currently polled. Keeping this below the display refresh
// rate leaves enough main-thread time for input and expensive panes (such as the file viewer)
// while still presenting streamed output smoothly.
const RUNTIME_POLL_INTERVAL: Duration = Duration::from_millis(33);
use iced::{event, keyboard};

use agency_agents::{
    Event as AgentEvent, Image as AgentImage, McpServer, McpTransport, Provider, QuestionRequest,
    Session as AgentSession,
};
use agency_mux::{Multiplexer, Program};
use agency_rpc::{
    ENV_CONVERSATION_ID, ENV_MCP_COMMAND, ENV_RPC_SOCKET, ENV_SESSION_TOKEN,
    Response as RpcResponse, Server as RpcServer, SessionCapabilities, SessionContext,
};
use agency_translator_api::commands::AgentCommand;
use agency_translator_api::{
    ClientId, ContentBlock, Conversation, ConversationEvent, ConversationUpdate, EventPayload,
    MessageRole, tools,
};
use config::{DefaultAgent, GlobalConfig, ModeColors, WindowState};
use diffs::{DiffLineKind, DiffSessionState, file_changes, renderable_diff_lines};
use keybindings::{
    Action, Activity, DispatchContext, ElementModeRegistry, FocusId, FocusTracker,
    KeybindingContext, Keybindings, Mode, ModeIndicator,
};
use plugins::{PluginInstallEntry, PluginInstallEvent, PluginInstalls, TranscriptInstalls};
use sessions::{SessionRegistry, name_from_prompt, new_conversation_id};
use slash_commands::{
    ComposerState, INIT_AGENT_PROMPT, SlashCommand, SlashCommandCompletion, SlashCompletionState,
    Submission, TabCompletion, agency_commands, completion_count, discover_agent_commands,
    initialize_workspace, load_codex_mcp, merge_catalog, resolve_submission,
    slash_command_completions, tab_completion,
};
use terminal::TerminalSession;
use worktrees::Worktree;

const AGENT_TRANSCRIPT_ID: &str = "agent-transcript";
/// Shared by both places `submit_agent_input` rejects an image alongside a
/// command already bound to an agent: a slash command typed and resolved
/// against the catalog, and one accepted from the overlay or Tab-filled, so
/// `command_provider` was already set before Enter was pressed.
const AGENT_COMMAND_IMAGE_ATTACHMENT_NOTICE: &str =
    "Agent slash commands and skills cannot include image attachments";
const FOCUS_TOOLBAR: FocusId = FocusId(0);
const FOCUS_EXPLORER: FocusId = FocusId(1);
const FOCUS_WORKSPACE: FocusId = FocusId(2);
const FOCUS_COMPOSER: FocusId = FocusId(3);
const FOCUS_TERMINAL: FocusId = FocusId(4);
const FOCUS_DIFF_VIEWER: FocusId = FocusId(5);
const FOCUS_DIFF_ACTIVITY: FocusId = FocusId(6);
const FOCUS_SLASH_COMPLETION: FocusId = FocusId(7);
const FOCUS_CONFIRMATION: FocusId = FocusId(8);
const FOCUS_AGENT_MENU: FocusId = FocusId(9);
/// Overlays that take focus from the surface they float over rather than owning
/// a place in the focus cycle.
const BORROWING_OVERLAYS: [FocusId; 3] =
    [FOCUS_SLASH_COMPLETION, FOCUS_CONFIRMATION, FOCUS_AGENT_MENU];

/// Height reserved for the status bar. The agent menu floats directly above it,
/// so the two have to agree on where the bar starts.
const STATUS_BAR_HEIGHT: f32 = 30.0;
const AGENT_MENU_GAP: f32 = 7.0;

fn ui_focus_tracker() -> FocusTracker<KeybindingContext> {
    let mut focus = FocusTracker::new(FOCUS_WORKSPACE, KeybindingContext::Workspace);
    for (element, context) in [
        (FOCUS_TOOLBAR, KeybindingContext::Toolbar),
        (FOCUS_EXPLORER, KeybindingContext::Explorer),
        (FOCUS_DIFF_ACTIVITY, KeybindingContext::DiffActivity),
        (FOCUS_DIFF_VIEWER, KeybindingContext::DiffViewer),
        (FOCUS_COMPOSER, KeybindingContext::Composer),
        (FOCUS_TERMINAL, KeybindingContext::Terminal),
        // The slash completion list is an inline composer overlay. It borrows
        // focus so it can render as the active surface, but keys still resolve
        // against the composer keymap so typing continues to filter it.
        (FOCUS_SLASH_COMPLETION, KeybindingContext::Composer),
        (FOCUS_CONFIRMATION, KeybindingContext::Confirmation),
        // The agent menu floats over the status bar's agent chip and owns
        // input while it is open, so it binds its own vim-style keymap.
        (FOCUS_AGENT_MENU, KeybindingContext::AgentMenu),
    ] {
        focus.attach(element, context);
    }
    focus
}

/// Whether an action rewrites the composer's prompt text. The match is
/// exhaustive on purpose: a new action has to declare itself rather than
/// silently skipping the prompt-changed event the completion list reduces.
fn edits_prompt(action: &Action) -> bool {
    match action {
        Action::AgentAppend(_)
        | Action::AgentBackspace
        | Action::AgentPaste
        | Action::AgentDeleteChar
        | Action::AgentOperate(..)
        | Action::AgentOperateSelection(_)
        | Action::AgentSubmit => true,
        // Cursor, selection, and mode moves leave the text alone.
        Action::AgentSelectAll
        | Action::AgentMove(_)
        | Action::AgentInsertAtLineStart
        | Action::AgentAppendAtCursor
        | Action::AgentAppendAtLineEnd
        | Action::None
        | Action::FocusRight
        | Action::FocusLeft
        | Action::WorktreePrevious
        | Action::WorktreeNext
        | Action::WorktreeSelect(_)
        | Action::ToggleActivity(_)
        | Action::ToggleSettings
        | Action::NewSession
        | Action::ToolbarPrevious
        | Action::ToolbarNext
        | Action::ToolbarFirst
        | Action::ToolbarLast
        | Action::ToolbarOpen
        | Action::ToolbarTrash
        | Action::ExplorerPrevious
        | Action::ExplorerNext
        | Action::ExplorerCollapse
        | Action::ExplorerExpand
        | Action::ExplorerOpen
        | Action::DiffPrevious
        | Action::DiffNext
        | Action::DiffFirst
        | Action::DiffLast
        | Action::DiffOpen
        | Action::DiffScrollUp
        | Action::DiffScrollDown
        | Action::DiffJumpToTool
        | Action::DiffClose
        | Action::ToggleTerminal
        | Action::ToggleAgentMenu
        | Action::AgentMenuPrevious
        | Action::AgentMenuNext
        | Action::AgentMenuFirst
        | Action::AgentMenuLast
        | Action::AgentMenuConfirm
        | Action::AgentMenuClose
        | Action::EnterComposer
        | Action::EnterTerminal
        | Action::TerminalInput(_) => false,
    }
}

fn ui_element_modes() -> ElementModeRegistry<Mode> {
    let mut modes = ElementModeRegistry::default();
    modes.attach(FOCUS_COMPOSER, [Mode::Normal, Mode::Insert, Mode::Visual]);
    modes.attach(FOCUS_DIFF_VIEWER, [Mode::Normal, Mode::Visual]);
    modes.attach(FOCUS_EXPLORER, [Mode::Normal]);
    modes.attach(FOCUS_DIFF_ACTIVITY, [Mode::Normal]);
    modes.attach(FOCUS_TOOLBAR, [Mode::Normal]);
    modes.attach(FOCUS_WORKSPACE, [Mode::Normal]);
    modes.attach(FOCUS_TERMINAL, [Mode::Terminal]);
    // Overlays borrow focus from the surface that spawned them: the slash
    // completion list is a composer affordance, and the confirmation modal is
    // navigated from NORMAL.
    modes.attach(
        FOCUS_SLASH_COMPLETION,
        [Mode::Normal, Mode::Insert, Mode::Visual],
    );
    modes.attach(FOCUS_CONFIRMATION, [Mode::Normal]);
    modes.attach(FOCUS_AGENT_MENU, [Mode::Normal]);
    modes
}
const DIFF_VIEW_ID: &str = "diff-view";
const PANEL_RIGHT_CLOSE_ICON: &[u8] = include_bytes!("../assets/icons/panel-right-close.svg");
const MESSAGE_SQUARE_ICON: &[u8] = include_bytes!("../assets/icons/message-square.svg");
const ARROW_RIGHT_ICON: &[u8] = include_bytes!("../assets/icons/arrow-right.svg");
const TRASH_ICON: &[u8] = include_bytes!("../assets/icons/trash-2.svg");
const FILE_ICON: &[u8] = include_bytes!("../assets/icons/file.svg");
const FOLDER_ICON: &[u8] = include_bytes!("../assets/icons/folder.svg");
const TERMINAL_ICON: &[u8] = include_bytes!("../assets/icons/terminal.svg");
const NETWORK_ICON: &[u8] = include_bytes!("../assets/icons/network.svg");
const CHEVRON_RIGHT_ICON: &[u8] = include_bytes!("../assets/icons/chevron-right.svg");
const CHEVRON_DOWN_ICON: &[u8] = include_bytes!("../assets/icons/chevron-down.svg");
const SETTINGS_ICON: &[u8] = include_bytes!("../assets/icons/settings.svg");
const MOUSE_CHASTISEMENT: &str = "Easy there, clicky—this is a keybindings establishment.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarTool {
    Sessions,
    Explorer,
    Mcp,
}

struct LayoutState {
    sidebar_tool: SidebarTool,
    toolbar_visible: bool,
    terminal_visible: bool,
    settings_open: bool,
}

#[derive(Default)]
struct ToolbarState {
    selected_session: usize,
}

#[derive(Default)]
struct ExplorerState {
    selected: usize,
    expanded: HashSet<PathBuf>,
}

#[derive(Default)]
struct OverlayState {
    pending_session_trash: Option<usize>,
    slash: SlashCompletionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentMenuMotion {
    Previous,
    Next,
    First,
    Last,
}

/// Floating agent switcher anchored to the status bar's agent chip. It owns its
/// own visibility and selection so no view has to coordinate the other.
#[derive(Default)]
struct AgentMenuState {
    open: bool,
    selected: usize,
    /// The element that owned focus when the menu opened. The menu borrows
    /// focus while it is up and hands it back when it closes.
    return_focus: Option<FocusId>,
}

#[derive(Debug, Clone, Copy)]
struct AgentMenuContext<'a> {
    agents: &'a [Provider],
    selected_agent: Provider,
    focused: FocusId,
}

impl AgentMenuState {
    fn on_event(&mut self, event: &AppEvent, context: AgentMenuContext<'_>) {
        match event {
            AppEvent::ToggleAgentMenu => {
                if self.open {
                    self.close();
                } else {
                    self.open = true;
                    self.return_focus = Some(context.focused);
                    self.selected = context
                        .agents
                        .iter()
                        .position(|agent| *agent == context.selected_agent)
                        .unwrap_or_default();
                }
            }
            // Choosing an agent, starting a session, or leaving the workspace
            // all resolve the menu: it never survives the interaction it began.
            AppEvent::CloseAgentMenu
            | AppEvent::SelectAgent(_)
            | AppEvent::StartAgent(_)
            | AppEvent::ToggleSettings => self.close(),
            AppEvent::MoveAgentMenu(motion) => {
                let count = context.agents.len();
                if count == 0 {
                    self.selected = 0;
                    return;
                }
                self.selected = match motion {
                    AgentMenuMotion::Previous => self
                        .selected
                        .min(count - 1)
                        .checked_sub(1)
                        .unwrap_or(count - 1),
                    AgentMenuMotion::Next => (self.selected + 1) % count,
                    AgentMenuMotion::First => 0,
                    AgentMenuMotion::Last => count - 1,
                };
            }
            _ => {}
        }
    }

    fn close(&mut self) {
        self.open = false;
        self.selected = 0;
    }

    fn highlighted(&self, agents: &[Provider]) -> Option<Provider> {
        agents.get(self.selected).copied()
    }
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            sidebar_tool: SidebarTool::Sessions,
            toolbar_visible: false,
            terminal_visible: false,
            settings_open: false,
        }
    }
}

impl LayoutState {
    fn on_event(&mut self, event: &AppEvent) {
        match event {
            AppEvent::ToggleToolbar => self.toolbar_visible = !self.toolbar_visible,
            AppEvent::ToggleActivity(tool) => {
                (self.sidebar_tool, self.toolbar_visible) =
                    toggled_activity(self.sidebar_tool, self.toolbar_visible, *tool);
                self.settings_open = false;
            }
            AppEvent::ToggleSettings => {
                self.settings_open = !self.settings_open;
                if self.settings_open {
                    self.terminal_visible = false;
                }
            }
            AppEvent::EnterComposer => {
                // The terminal covers the composer, so entering the composer
                // has to uncover it: a hidden composer cannot take focus, and
                // the mode would otherwise be stranded away from its element.
                self.toolbar_visible = false;
                self.terminal_visible = false;
            }
            AppEvent::TerminalVisibilityChanged(visible) => {
                self.terminal_visible = *visible;
                if *visible {
                    self.settings_open = false;
                }
            }
            _ => {}
        }
    }
}

struct InteractionState {
    focus: FocusTracker<KeybindingContext>,
    element_modes: ElementModeRegistry<Mode>,
    input_mode: InputModeState,
    /// The surface an overlay borrowed focus from, so closing the overlay hands
    /// focus back instead of dropping it on whichever element happens to sort
    /// first.
    borrowed_focus: Option<FocusId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InputModeState {
    mode: Mode,
}

impl Default for InputModeState {
    fn default() -> Self {
        Self { mode: Mode::Normal }
    }
}

impl InputModeState {
    fn on_event(&mut self, event: &AppEvent) {
        if let AppEvent::InputModeChanged { mode } = event {
            self.mode = *mode;
        }
    }

    fn composer_needs_insert_hint(self) -> bool {
        self.mode == Mode::Normal
    }
}

struct EventEnvelope<E> {
    sequence: u64,
    event: E,
}

struct EventBus<E> {
    next_sequence: u64,
    pending: VecDeque<EventEnvelope<E>>,
}

impl<E> Default for EventBus<E> {
    fn default() -> Self {
        Self {
            next_sequence: 0,
            pending: VecDeque::new(),
        }
    }
}

impl<E> EventBus<E> {
    fn publish(&mut self, event: E) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.pending.push_back(EventEnvelope { sequence, event });
        sequence
    }

    fn next(&mut self) -> Option<EventEnvelope<E>> {
        self.pending.pop_front()
    }
}

impl Default for InteractionState {
    fn default() -> Self {
        Self {
            focus: ui_focus_tracker(),
            element_modes: ui_element_modes(),
            input_mode: InputModeState::default(),
            borrowed_focus: None,
        }
    }
}

#[derive(Default)]
struct FocusVisibility {
    toolbar: bool,
    explorer: bool,
    workspace: bool,
    composer: bool,
    terminal: bool,
    diff_viewer: bool,
    diff_activity: bool,
    slash_completion: bool,
    confirmation: bool,
    agent_menu: bool,
}

impl InteractionState {
    /// Modes are application-global while focus is not, so a mode the focused
    /// element cannot bind would swallow every key. Such a pairing resolves
    /// back to NORMAL.
    fn reconciled_mode(&self, mode: Mode) -> Mode {
        if self.element_modes.supports(self.focus.focused(), mode) {
            mode
        } else {
            Mode::Normal
        }
    }

    fn sync_visibility(&mut self, visibility: FocusVisibility) {
        for (element, visible) in [
            (FOCUS_TOOLBAR, visibility.toolbar),
            (FOCUS_EXPLORER, visibility.explorer),
            (FOCUS_WORKSPACE, visibility.workspace),
            (FOCUS_COMPOSER, visibility.composer),
            (FOCUS_TERMINAL, visibility.terminal),
            (FOCUS_DIFF_VIEWER, visibility.diff_viewer),
            (FOCUS_DIFF_ACTIVITY, visibility.diff_activity),
            (FOCUS_SLASH_COMPLETION, visibility.slash_completion),
            (FOCUS_CONFIRMATION, visibility.confirmation),
            (FOCUS_AGENT_MENU, visibility.agent_menu),
        ] {
            debug_assert!(self.focus.set_visible(element, visible));
        }

        let forced = if visibility.confirmation {
            Some(FOCUS_CONFIRMATION)
        } else if visibility.agent_menu {
            Some(FOCUS_AGENT_MENU)
        } else if visibility.slash_completion {
            Some(FOCUS_SLASH_COMPLETION)
        } else {
            None
        };
        if let Some(forced) = forced {
            // Overlays stack, so only the first one to take focus records where
            // it came from: closing them all returns to the original surface.
            if self.focus.focused() != forced && !BORROWING_OVERLAYS.contains(&self.focus.focused())
            {
                self.borrowed_focus = Some(self.focus.focused());
            }
            debug_assert!(self.focus.focus(forced));
        } else {
            let borrowed = self.borrowed_focus.take();
            if !self.focus.is_visible(self.focus.focused())
                && !borrowed.is_some_and(|element| self.focus.focus(element))
            {
                self.focus.focus_right();
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpStatus {
    Waiting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpServerState {
    Connected,
    Error,
    RequiresAuthentication,
}

impl McpServerState {
    fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Error => "Error",
            Self::RequiresAuthentication => "Require authentication",
        }
    }
}

#[derive(Debug, Clone)]
struct ExplorerEntry {
    path: PathBuf,
    depth: usize,
    directory: bool,
}

fn main() {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "--mcp") {
        if let Err(error) = agency_mcp::run() {
            eprintln!("agency --mcp: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run_desktop() {
        eprintln!("agency: {error}");
        std::process::exit(1);
    }
}

fn run_desktop() -> iced::Result {
    let window_state = WindowState::load().unwrap_or_default();
    let position = match (window_state.x, window_state.y) {
        (Some(x), Some(y)) => window::Position::Specific(Point::new(x, y)),
        _ => window::Position::Default,
    };

    iced::application(Agency::boot, Agency::update, Agency::view)
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
    rpc_capabilities: SessionCapabilities,
    rpc_server: Option<RpcServer>,
    keybindings: Keybindings,
    interaction: InteractionState,
    layout: LayoutState,
    event_bus: EventBus<AppEvent>,
    multiplexer: Multiplexer,
    terminals: Vec<TerminalSession>,
    active_terminal: Option<usize>,
    agents: Vec<AgentView>,
    active_agent: Option<usize>,
    workspaces: workspaces::Workspaces,
    toolbar: ToolbarState,
    explorer: ExplorerState,
    overlays: OverlayState,
    agent_menu: AgentMenuState,
    worktrees: Vec<Worktree>,
    active_worktree: usize,
    cwd: PathBuf,
    notice: Option<String>,
    mouse_notice_until: Option<Instant>,
    mode_colors: ModeColors,
    selected_agent: Provider,
    default_agent: Provider,
    configured_agents: Vec<Provider>,
    cursor_visible: bool,
    cursor_blinked_at: Instant,
    animation_started_at: Instant,
    transcript_scroll_target: Option<f32>,
    file_viewer: file_viewer::State,
    agent_installations: Vec<AgentInstallation>,
    slash_command_catalog: Vec<SlashCommandCompletion>,
    plugin_installs: PluginInstalls,
}

#[derive(Debug, Clone)]
struct AgentInstallation {
    provider: Provider,
    path: Option<PathBuf>,
    version: Option<String>,
}

struct AgentView {
    workspace: PathBuf,
    conversation_id: String,
    rpc_token: String,
    session: AgentSession,
    transcript: Vec<TranscriptEntry>,
    transcript_dirty: bool,
    conversation: Conversation,
    prompt: String,
    prompt_selected: bool,
    prompt_cursor: usize,
    prompt_selection_anchor: Option<usize>,
    command_provider: Option<Provider>,
    images: Vec<AgentImage>,
    pending_question: Option<PendingQuestion>,
    status: String,
    session_id: Option<String>,
    pending_session_name: Option<String>,
    pending_conversation_id: Option<String>,
    completed_turns: u64,
    activity: AgentActivity,
    queued_messages: VecDeque<QueuedMessage>,
    image_cache: HashMap<String, TranscriptImage>,
    image_cache_directory: PathBuf,
    diff_state: DiffSessionState,
    session_directory: PathBuf,
    last_changed_at_millis: u64,
    mcp_status: McpStatus,
    plugin_installs: TranscriptInstalls,
}

type SessionUpdate = (Provider, String, Option<String>, Option<String>, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentActivity {
    Starting,
    Idle,
    Active,
    Thinking,
    WaitingForInput,
    Error,
}

impl AgentActivity {
    fn is_busy(self) -> bool {
        matches!(self, Self::Active | Self::Thinking | Self::WaitingForInput)
    }

    fn badge(self) -> ui_theme::AgentStatus {
        match self {
            Self::Starting | Self::Active => ui_theme::AgentStatus::Active,
            Self::Thinking => ui_theme::AgentStatus::Thinking,
            Self::WaitingForInput => ui_theme::AgentStatus::Waiting,
            Self::Idle | Self::Error => ui_theme::AgentStatus::Idle,
        }
    }
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
        output: String,
        status: String,
        exit_code: Option<i64>,
    },
    FileChanges {
        status: String,
        changes: Vec<diffs::FileChange>,
    },
    FileRead {
        path: String,
        status: String,
        lines: Option<u64>,
    },
    WebSearch {
        queries: Vec<String>,
    },
    PluginInstall(PluginInstallEntry),
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

fn mcp_server_state(server: &McpServer, agents: &[AgentView]) -> McpServerState {
    if mcp_server_requires_auth(server) {
        McpServerState::RequiresAuthentication
    } else if agents
        .iter()
        .any(|agent| agent.mcp_status == McpStatus::Disconnected)
    {
        McpServerState::Error
    } else {
        McpServerState::Connected
    }
}

fn agency_mcp_server_state(statuses: impl IntoIterator<Item = McpStatus>) -> McpServerState {
    for status in statuses {
        match status {
            McpStatus::Disconnected => return McpServerState::Error,
            McpStatus::Waiting | McpStatus::Connected => {}
        }
    }
    McpServerState::Connected
}

fn mcp_server_requires_auth(server: &McpServer) -> bool {
    let McpTransport::StreamableHttp {
        bearer_token_env_var,
        env_http_headers,
        ..
    } = &server.transport
    else {
        return false;
    };

    bearer_token_env_var
        .iter()
        .chain(env_http_headers.iter().flat_map(|headers| headers.values()))
        .any(|variable| std::env::var_os(variable).is_none())
}

fn configured_agents(path: Option<&std::ffi::OsStr>) -> Vec<Provider> {
    let Some(path) = path else {
        return Vec::new();
    };

    [Provider::Codex, Provider::Claude]
        .into_iter()
        .filter(|provider| {
            let command = provider.command();
            std::env::split_paths(path).any(|directory| {
                let candidate = directory.join(command);
                candidate.is_file()
                    || (cfg!(windows) && directory.join(format!("{command}.exe")).is_file())
            })
        })
        .collect()
}

fn detect_agent_installations(path: Option<&std::ffi::OsStr>) -> Vec<AgentInstallation> {
    [Provider::Codex, Provider::Claude]
        .into_iter()
        .map(|provider| {
            let command = provider.command();
            let path = path.and_then(|path| {
                std::env::split_paths(path).find_map(|directory| {
                    let candidate = directory.join(command);
                    candidate.is_file().then_some(candidate)
                })
            });
            let version = path.as_ref().and_then(|executable| {
                Command::new(executable)
                    .arg("--version")
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| String::from_utf8(output.stdout).ok())
                    .map(|output| output.trim().to_owned())
                    .filter(|output| !output.is_empty())
            });
            AgentInstallation {
                provider,
                path,
                version,
            }
        })
        .collect()
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
enum AppEvent {
    Action(Action),
    AgentRuntime {
        conversation_id: String,
        events: Vec<AgentEvent>,
    },
    TranscriptChanged {
        conversation_id: String,
    },
    RefreshVisibleTranscript,
    OpenRepository,
    EnterComposer,
    InputModeChanged {
        mode: Mode,
    },
    FocusNext,
    FocusPrevious,
    FocusRequested(FocusId),
    LinkClicked(markdown::Uri),
    ToggleToolbar,
    ToggleActivity(SidebarTool),
    ToggleExplorerEntry(usize),
    SelectWorktree(usize),
    /// The tab strip as git reports it. Published at startup and after any
    /// change to the worktree set, so all three paths land in one reducer.
    WorktreesDiscovered {
        worktrees: Vec<Worktree>,
    },
    WorktreeCreated {
        worktree: Worktree,
    },
    WorktreeRemoved {
        worktree: Worktree,
    },
    StartAgent(Provider),
    ResumeSession(usize),
    RequestSessionTrash(usize),
    CancelSessionTrash,
    ConfirmSessionTrash,
    AnswerChoice(usize),
    CompleteSlashCommand(String, Option<Provider>),
    TabCompleteSlashCommand,
    /// The composer's prompt text changed. Published *after* the change lands so
    /// every prompt-derived affordance re-reads the text as it stands now.
    ComposerPromptChanged,
    /// Ask the configured agents what they can run here. Published at startup,
    /// on worktree switch, and after an install changes what is on disk.
    SlashCatalogRequested,
    SlashCatalogLoaded(Vec<(Provider, AgentCommand)>),
    SlashCatalogFailed(String),
    PluginInstallRequested {
        conversation_id: String,
        source: String,
        targets: Vec<Provider>,
    },
    PluginInstall(PluginInstallEvent),
    ToggleTerminalActivity,
    TerminalVisibilityChanged(bool),
    ToggleDiffActivity,
    ToggleFileViewer,
    ToggleFileViewerMode,
    SetFileViewerMode(file_viewer::Mode),
    OpenFileViewer(PathBuf),
    SelectDiff(usize),
    ToggleFullscreen(window::Id, window::Mode),
    WindowEvent(window::Id, window::Event),
    WindowGeometryReady(window::Id, Size, Option<Point>, bool, window::Mode, bool),
    Keyboard(keyboard::Event),
    MouseClick,
    Tick(Instant),
    ToggleSettings,
    RefreshAgents,
    SetDefaultAgent(Provider),
    ToggleAgentMenu,
    CloseAgentMenu,
    MoveAgentMenu(AgentMenuMotion),
    SelectAgent(Provider),
}

impl Agency {
    /// Shared by `Default` and the test-only constructor below. `spawn_agent_and_rpc`
    /// gates the two effects a unit test must never trigger: binding the RPC unix
    /// socket and spawning a real `codex`/`claude` child process. Everything else
    /// (config load, worktree discovery, session registry) is read-only and safe
    /// to run in either path.
    fn build(spawn_agent_and_rpc: bool) -> Self {
        let (config, notice) = match GlobalConfig::load() {
            Ok(config) => (config, None),
            Err(error) => (GlobalConfig::default(), Some(error)),
        };
        let mode_colors = ModeColors::from_config(&config.mode_colors);
        let configured_agents = configured_agents(std::env::var_os("PATH").as_deref());
        let agent_installations = detect_agent_installations(std::env::var_os("PATH").as_deref());
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
        // Keyed to the whole list, never to `cwd`: the legacy layout lived under
        // the primary, so a launch from a linked worktree must still migrate the
        // primary's history rather than search the worktree it started in.
        sessions::migrate_legacy_sessions(&worktrees);
        let _ = worktrees::ensure_agency_ignored(&cwd);
        let mut workspaces = workspaces::Workspaces::new();
        let session_notice = workspaces.ensure(&cwd).err();
        let rpc_capabilities = SessionCapabilities::default();
        let slash_command_catalog = agency_commands();
        let (rpc_server, rpc_notice) = if spawn_agent_and_rpc {
            match RpcServer::start(rpc_capabilities.clone()) {
                Ok(server) => (Some(server), None),
                Err(error) => (None, Some(error)),
            }
        } else {
            (None, None)
        };

        let mut agency = Self {
            rpc_capabilities,
            rpc_server,
            keybindings: Keybindings::from_config(config.keybindings),
            interaction: InteractionState::default(),
            layout: LayoutState::default(),
            event_bus: EventBus::default(),
            toolbar: ToolbarState::default(),
            explorer: ExplorerState::default(),
            overlays: OverlayState::default(),
            agent_menu: AgentMenuState::default(),
            multiplexer: Multiplexer::default(),
            terminals: Vec::new(),
            active_terminal: None,
            agents: Vec::new(),
            active_agent: None,
            workspaces,
            worktrees,
            active_worktree,
            cwd,
            notice: notice.or(worktree_notice).or(session_notice).or(rpc_notice),
            mouse_notice_until: None,
            mode_colors,
            selected_agent: default_agent,
            default_agent,
            configured_agents,
            cursor_visible: true,
            cursor_blinked_at: Instant::now(),
            animation_started_at: Instant::now(),
            transcript_scroll_target: None,
            file_viewer: file_viewer::State::default(),
            agent_installations,
            slash_command_catalog,
            plugin_installs: PluginInstalls::default(),
        };
        let startup_notice = agency.notice.take();
        let discovered = agency.worktrees.clone();
        agency.emit(AppEvent::WorktreesDiscovered {
            worktrees: discovered,
        });
        if spawn_agent_and_rpc {
            agency.start_agent(default_agent);
        }
        if agency.notice.is_none() {
            agency.notice = startup_notice;
        }
        agency
    }
}

impl Default for Agency {
    fn default() -> Self {
        Self::build(true)
    }
}

#[cfg(test)]
impl Agency {
    /// `Default` binds a real RPC socket and spawns a real agent process, which a
    /// unit test must never do: nothing frees either when the test ends, so a
    /// full suite run leaks a child process per test, and parallel tests race
    /// over the same pid-derived socket path. Reducer tests that only care about
    /// state transitions use this instead.
    fn for_testing() -> Self {
        Self::build(false)
    }

    /// Reducer tests call `reduce_event` directly rather than `update`, so
    /// nothing else drains the bus for them. This gives a test the same view
    /// `update`'s loop would have had, without processing the follow-up events
    /// (which would blur "did this handler publish X" into "what did X do").
    fn drain_events(&mut self) -> Vec<AppEvent> {
        let mut events = Vec::new();
        while let Some(envelope) = self.event_bus.next() {
            events.push(envelope.event);
        }
        events
    }
}

impl Agency {
    /// iced's boot hook. The catalog starts with Agency's own commands and the
    /// agent half is requested immediately, so the composer is usable while
    /// the first index runs.
    fn boot() -> (Self, Task<AppEvent>) {
        (Self::default(), Task::done(AppEvent::SlashCatalogRequested))
    }

    fn activate_focused_context(&mut self) {
        self.keybindings
            .activate_context(self.interaction.focus.context());
        self.reconcile_mode();
        self.publish_input_mode();
    }

    /// Modes are global, focus is not. An element that cannot bind keys in the
    /// active mode would swallow every key, so the mode falls back to NORMAL
    /// whenever focus and mode disagree.
    fn reconcile_mode(&mut self) {
        let reconciled = self.interaction.reconciled_mode(self.keybindings.mode());
        if reconciled != self.keybindings.mode() {
            self.keybindings.set_mode(reconciled);
            self.publish_input_mode();
        }
    }

    fn publish_input_mode(&mut self) {
        self.emit(AppEvent::InputModeChanged {
            mode: self.keybindings.mode(),
        });
    }

    fn sync_focus(&mut self) {
        let diff_viewer_visible = self
            .active_agent()
            .is_some_and(|agent| agent.diff_state.viewer_visible)
            && !self.layout.terminal_visible;
        let diff_activity_visible = self
            .active_agent()
            .is_some_and(|agent| agent.diff_state.activity_visible)
            && !self.layout.terminal_visible;
        let explorer_visible =
            self.layout.toolbar_visible && self.layout.sidebar_tool == SidebarTool::Explorer;
        let toolbar_visible = self.layout.toolbar_visible && !explorer_visible;
        let composer_visible = self.active_agent.is_some() && !self.layout.terminal_visible;
        let workspace_visible = !composer_visible && !self.layout.terminal_visible;

        self.interaction.sync_visibility(FocusVisibility {
            toolbar: toolbar_visible,
            explorer: explorer_visible,
            workspace: workspace_visible,
            composer: composer_visible,
            terminal: self.layout.terminal_visible,
            diff_viewer: diff_viewer_visible,
            diff_activity: diff_activity_visible,
            slash_completion: self.overlays.slash.is_open(),
            confirmation: self.overlays.pending_session_trash.is_some(),
            agent_menu: self.agent_menu.open,
        });
        self.reconcile_mode();
    }

    fn theme(&self) -> Theme {
        ui_theme::theme()
    }

    fn update(&mut self, event: AppEvent) -> Task<AppEvent> {
        self.event_bus.publish(event);
        let mut tasks = Vec::new();
        while let Some(envelope) = self.event_bus.next() {
            let _sequence = envelope.sequence;
            self.layout.on_event(&envelope.event);
            self.interaction.input_mode.on_event(&envelope.event);
            let agent_menu_context = AgentMenuContext {
                agents: &self.configured_agents,
                selected_agent: self.selected_agent,
                focused: self.interaction.focus.focused(),
            };
            self.agent_menu
                .on_event(&envelope.event, agent_menu_context);
            tasks.push(self.reduce_event(envelope.event));
        }
        Task::batch(tasks)
    }

    fn emit(&mut self, event: AppEvent) {
        self.event_bus.publish(event);
    }

    /// Re-derives the slash completion overlay from the prompt. This has to run
    /// after the prompt has already changed — reading it while the mutation is
    /// still queued is what left the list a keystroke behind.
    fn refresh_slash_completions(&mut self) {
        let composer = ComposerState {
            focused: self.interaction.focus.context() == KeybindingContext::Composer,
            accepting_text: self.keybindings.is_composer_active(),
        };
        let prompt = self
            .active_agent()
            .map(|agent| agent.prompt.clone())
            .unwrap_or_default();
        let was_open = self.overlays.slash.is_open();
        self.overlays
            .slash
            .refresh(&self.slash_command_catalog, &prompt, composer);
        if self.overlays.slash.is_open() != was_open {
            // Opening borrows focus from the composer and closing gives it
            // back, so focus follows the overlay in the same pass.
            self.sync_focus();
        }
    }

    fn slash_completion_count(&self) -> usize {
        self.active_agent().map_or(0, |agent| {
            completion_count(&self.slash_command_catalog, &agent.prompt)
        })
    }

    fn reduce_event(&mut self, message: AppEvent) -> Task<AppEvent> {
        let transcript_len = self.active_agent().map_or(0, AgentView::transcript_len);

        match message {
            AppEvent::Action(action) => self.apply(action),
            AppEvent::AgentRuntime {
                conversation_id,
                events,
            } => {
                let (updates, transcript_changed) = self
                    .agents
                    .iter_mut()
                    .find(|agent| agent.conversation_id == conversation_id)
                    .map(|agent| agent.on_runtime_events(events))
                    .unwrap_or_default();
                self.record_session_updates(updates);
                if transcript_changed {
                    self.emit(AppEvent::TranscriptChanged { conversation_id });
                }
            }
            AppEvent::TranscriptChanged { conversation_id } => {
                let visible = transcript_is_visible(
                    self.active_agent()
                        .map(|agent| agent.conversation_id.as_str()),
                    self.layout.terminal_visible,
                    self.layout.settings_open,
                    &conversation_id,
                );
                if let Some(agent) = self
                    .agents
                    .iter_mut()
                    .find(|agent| agent.conversation_id == conversation_id)
                {
                    agent.transcript_dirty = true;
                    if visible {
                        agent.rebuild_transcript();
                    }
                }
            }
            AppEvent::RefreshVisibleTranscript => {
                if !self.layout.terminal_visible
                    && !self.layout.settings_open
                    && let Some(agent) = self.active_agent_mut()
                    && agent.transcript_dirty
                {
                    agent.rebuild_transcript();
                }
            }
            AppEvent::OpenRepository => {}
            AppEvent::EnterComposer => {
                self.sync_focus();
                if self.interaction.focus.focus(FOCUS_COMPOSER) {
                    self.keybindings.set_mode(Mode::Insert);
                }
                self.publish_input_mode();
                // A prompt left holding a slash command gets its list back when
                // the composer is re-entered.
                self.refresh_slash_completions();
            }
            AppEvent::InputModeChanged { .. } => {}
            AppEvent::FocusNext => {
                self.interaction.focus.focus_right();
                self.activate_focused_context();
            }
            AppEvent::FocusPrevious => {
                self.interaction.focus.focus_left();
                self.activate_focused_context();
            }
            AppEvent::FocusRequested(target) => {
                self.sync_focus();
                if self.interaction.focus.focus(target) {
                    self.activate_focused_context();
                }
            }
            AppEvent::ToggleSettings => {
                self.sync_focus();
                if !self.layout.settings_open {
                    self.emit(AppEvent::RefreshVisibleTranscript);
                }
            }
            AppEvent::RefreshAgents => {
                self.agent_installations =
                    detect_agent_installations(std::env::var_os("PATH").as_deref());
                self.configured_agents = self
                    .agent_installations
                    .iter()
                    .filter(|agent| agent.path.is_some())
                    .map(|agent| agent.provider)
                    .collect();
                // A newly detected agent has its own slash commands, and a
                // newly missing one should drop its rows; either way the
                // catalog built from the old agent list is stale.
                self.emit(AppEvent::SlashCatalogRequested);
            }
            AppEvent::SetDefaultAgent(provider) => {
                let configured = match provider {
                    Provider::Codex => DefaultAgent::Codex,
                    Provider::Claude => DefaultAgent::Claude,
                };
                match GlobalConfig::save_default_agent(configured) {
                    Ok(()) => {
                        self.default_agent = provider;
                        self.selected_agent = provider;
                        self.notice =
                            Some(format!("{} is now the default agent", provider.label()));
                    }
                    Err(error) => self.notice = Some(error),
                }
            }
            AppEvent::ToggleAgentMenu | AppEvent::CloseAgentMenu | AppEvent::MoveAgentMenu(_) => {
                self.agent_menu_focus_changed();
            }
            AppEvent::SelectAgent(provider) => {
                self.select_agent(provider);
                self.agent_menu_focus_changed();
            }
            AppEvent::LinkClicked(uri) => self.notice = Some(format!("Link: {uri}")),
            AppEvent::ToggleToolbar => self.sync_focus(),
            AppEvent::ToggleActivity(tool) => {
                self.activity_did_toggle(tool);
            }
            AppEvent::ToggleExplorerEntry(index) => self.toggle_explorer_entry(index),
            AppEvent::SelectWorktree(index) => self.select_worktree(index),
            AppEvent::WorktreesDiscovered { worktrees } => self.worktrees_discovered(worktrees),
            AppEvent::WorktreeCreated { worktree } => self.worktree_created(worktree),
            AppEvent::WorktreeRemoved { worktree } => self.worktree_removed(&worktree),
            AppEvent::StartAgent(provider) => {
                self.selected_agent = provider;
                self.start_agent(provider);
            }
            AppEvent::ResumeSession(index) => self.resume_session(index),
            AppEvent::RequestSessionTrash(index) => self.request_session_trash(index),
            AppEvent::CancelSessionTrash => self.overlays.pending_session_trash = None,
            AppEvent::ConfirmSessionTrash => self.confirm_session_trash(),
            AppEvent::AnswerChoice(choice) => {
                if let Some(agent) = self.active_agent_mut() {
                    agent.answer_choice(choice);
                }
            }
            AppEvent::CompleteSlashCommand(insertion, provider) => {
                if let Some(agent) = self.active_agent_mut() {
                    agent.prompt = normalized_prompt(insertion);
                    agent.prompt_selected = false;
                    agent.prompt_cursor = agent.prompt.len();
                    agent.prompt_selection_anchor = None;
                    agent.command_provider = provider;
                }
                // An accepted command usually still matches its own catalog
                // entry, so the list is closed outright rather than refreshed.
                // The next edit reopens it.
                self.overlays.slash.close();
                self.sync_focus();
            }
            AppEvent::ComposerPromptChanged => self.refresh_slash_completions(),
            AppEvent::TabCompleteSlashCommand => {
                let Some((prompt, active)) = self
                    .active_agent()
                    .map(|agent| (agent.prompt.clone(), agent.session.provider()))
                else {
                    return Task::none();
                };
                match tab_completion(
                    &self.slash_command_catalog,
                    &prompt,
                    self.overlays.slash.selected(),
                    Some(active),
                ) {
                    Some(TabCompletion::Fill(prefix)) => {
                        if let Some(agent) = self.active_agent_mut() {
                            agent.prompt = normalized_prompt(prefix);
                            agent.prompt_selected = false;
                            agent.prompt_cursor = agent.prompt.len();
                            agent.prompt_selection_anchor = None;
                        }
                        self.refresh_slash_completions();
                    }
                    // Accepting reuses the event a click publishes, so the
                    // prompt, provider, and overlay settle in one place.
                    Some(TabCompletion::Accept(completion)) => {
                        self.emit(AppEvent::CompleteSlashCommand(
                            completion.insertion,
                            completion.provider,
                        ));
                    }
                    None => {}
                }
            }
            AppEvent::SlashCatalogRequested => {
                let workspace = self.cwd.clone();
                let providers = self.configured_agents.clone();
                return Task::perform(
                    async move {
                        // Discovery reads hundreds of directories under the
                        // plugin caches. Running it on the async runtime would
                        // stall every other effect for as long as it takes.
                        tokio::task::spawn_blocking(move || {
                            discover_agent_commands(&providers, &workspace)
                        })
                        .await
                        .map_err(|error| format!("Could not index agent commands: {error}"))
                    },
                    |result| match result {
                        Ok(commands) => AppEvent::SlashCatalogLoaded(commands),
                        Err(error) => AppEvent::SlashCatalogFailed(error),
                    },
                );
            }
            AppEvent::SlashCatalogLoaded(commands) => {
                self.slash_command_catalog = merge_catalog(commands);
            }
            AppEvent::SlashCatalogFailed(error) => {
                // A stale catalog is more useful than an empty one, so the
                // previous entries stay.
                self.notice = Some(error);
            }
            AppEvent::PluginInstallRequested {
                conversation_id,
                source,
                targets,
            } => {
                let events =
                    self.plugin_installs
                        .start(&conversation_id, &source, &targets, &self.cwd);
                self.notice = Some(format!(
                    "Installing {} with {}",
                    plugins::install_kind(&source).describe(&source),
                    targets
                        .iter()
                        .map(|provider| provider.label())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for event in events {
                    self.emit(AppEvent::PluginInstall(event));
                }
            }
            AppEvent::PluginInstall(event) => {
                if let PluginInstallEvent::Finished {
                    provider,
                    kind,
                    status,
                    detail,
                    ..
                } = &event
                {
                    self.notice = Some(match (status, detail) {
                        (_, Some(detail)) => {
                            format!(
                                "{} could not install the plugin: {detail}",
                                provider.label()
                            )
                        }
                        (plugins::InstallStatus::Installed, None) => match kind {
                            plugins::InstallKind::Marketplace => {
                                format!("{} added the marketplace source", provider.label())
                            }
                            plugins::InstallKind::Plugin => {
                                format!("{} installed the plugin", provider.label())
                            }
                        },
                        (status, None) => format!(
                            "{} plugin install {}",
                            provider.label(),
                            status.label().to_lowercase()
                        ),
                    });
                    // An install changes what is on disk, so the catalog it
                    // was built from is now stale.
                    self.emit(AppEvent::SlashCatalogRequested);
                }
                let conversation_id = event.conversation_id().to_owned();
                let changed = self
                    .agents
                    .iter_mut()
                    .find(|agent| agent.conversation_id == conversation_id)
                    .is_some_and(|agent| agent.on_plugin_install(&event));
                if changed {
                    self.emit(AppEvent::TranscriptChanged { conversation_id });
                }
            }
            AppEvent::ToggleTerminalActivity => {
                self.keybindings
                    .toggle_terminal_mode(self.layout.terminal_visible);
                self.toggle_terminal();
            }
            AppEvent::TerminalVisibilityChanged(visible) => {
                self.sync_focus();
                if visible {
                    debug_assert!(self.interaction.focus.focus(FOCUS_TERMINAL));
                    self.activate_focused_context();
                } else {
                    self.emit(AppEvent::RefreshVisibleTranscript);
                }
            }
            AppEvent::ToggleDiffActivity => self.activate_diff_activity(),
            AppEvent::ToggleFileViewer => self.toggle_file_viewer(),
            AppEvent::ToggleFileViewerMode => self.file_viewer.toggle_mode(),
            AppEvent::SetFileViewerMode(mode) => self.file_viewer.set_mode(mode),
            AppEvent::OpenFileViewer(path) => self.open_file_viewer(path),
            AppEvent::SelectDiff(index) => self.select_diff(index),
            AppEvent::ToggleFullscreen(id, mode) => {
                let mode = match mode {
                    window::Mode::Fullscreen => window::Mode::Windowed,
                    window::Mode::Windowed | window::Mode::Hidden => window::Mode::Fullscreen,
                };
                return window::set_mode(id, mode);
            }
            AppEvent::WindowEvent(id, event) => match event {
                window::Event::CloseRequested => return window_geometry(id, true),
                window::Event::Moved(_) | window::Event::Resized(_) => {
                    return window_geometry(id, false);
                }
                _ => {}
            },
            AppEvent::WindowGeometryReady(id, size, position, maximized, mode, close_after) => {
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
            AppEvent::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                text,
                ..
            }) => {
                self.sync_focus();
                self.cursor_visible = true;
                self.cursor_blinked_at = Instant::now();
                if is_fullscreen_shortcut(&key, modifiers) {
                    return window::latest().then(|id| {
                        id.map_or_else(Task::none, |id| {
                            window::mode(id).map(move |mode| AppEvent::ToggleFullscreen(id, mode))
                        })
                    });
                }
                if self.overlays.pending_session_trash.is_some() {
                    match key.as_ref() {
                        keyboard::Key::Named(keyboard::key::Named::Enter)
                            if modifiers.is_empty() =>
                        {
                            self.confirm_session_trash();
                        }
                        keyboard::Key::Named(keyboard::key::Named::Escape)
                            if modifiers.is_empty() =>
                        {
                            self.overlays.pending_session_trash = None;
                        }
                        _ => {}
                    }
                    return Task::none();
                }
                // The agent menu floats above the workspace and owns input, so
                // it dispatches before the affordances of the surfaces behind
                // it — the markdown viewer, pending questions, and the inline
                // slash completion list all keep their keys until it closes.
                if self.agent_menu.open {
                    let action = self.keybindings.handle_in_context(
                        &key,
                        physical_key,
                        modifiers,
                        text.as_deref(),
                        DispatchContext {
                            terminal_available: self.active_terminal.is_some(),
                            composer_available: self.active_agent.is_some(),
                            ..DispatchContext::focused(self.interaction.focus.context())
                        },
                    );
                    self.publish_input_mode();
                    self.emit(AppEvent::Action(action));
                    return Task::none();
                }
                if self.file_viewer.visible
                    && self.file_viewer.is_markdown()
                    && modifiers.is_empty()
                    && matches!(
                        key.as_ref(),
                        keyboard::Key::Named(keyboard::key::Named::Tab)
                    )
                {
                    self.emit(AppEvent::ToggleFileViewerMode);
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
                let completion_count = self.slash_completion_count();
                if self.overlays.slash.is_open() && completion_count > 0 && modifiers.is_empty() {
                    let normal = self.keybindings.is_normal();
                    let previous = matches!(
                        key.as_ref(),
                        keyboard::Key::Named(keyboard::key::Named::ArrowUp)
                    ) || (normal
                        && matches!(key.as_ref(), keyboard::Key::Character(character) if character == "k"));
                    let next = matches!(
                        key.as_ref(),
                        keyboard::Key::Named(keyboard::key::Named::ArrowDown)
                    ) || (normal
                        && matches!(key.as_ref(), keyboard::Key::Character(character) if character == "j"));
                    if previous {
                        self.overlays.slash.select_previous(completion_count);
                        return Task::none();
                    }
                    if next {
                        self.overlays.slash.select_next(completion_count);
                        return Task::none();
                    }
                    // Tab is claimed here in both modes so it completes the
                    // command instead of reaching the composer as a literal tab.
                    if matches!(
                        key.as_ref(),
                        keyboard::Key::Named(keyboard::key::Named::Tab)
                    ) {
                        self.emit(AppEvent::TabCompleteSlashCommand);
                        return Task::none();
                    }
                    // Enter takes the highlighted row through the same event a
                    // click publishes, so the prompt, provider, and overlay
                    // settle in one place. A row that would insert exactly what
                    // is already typed has nothing to add, so Enter falls
                    // through and submits instead of demanding a second press.
                    if matches!(
                        key.as_ref(),
                        keyboard::Key::Named(keyboard::key::Named::Enter)
                    ) {
                        let completion = self.active_agent().and_then(|agent| {
                            slash_command_completions(
                                &self.slash_command_catalog,
                                &agent.prompt,
                                Some(agent.session.provider()),
                            )
                            .into_iter()
                            .nth(self.overlays.slash.selected())
                            .filter(|completion| completion.insertion != agent.prompt)
                            .cloned()
                        });
                        match completion {
                            Some(completion) => {
                                self.emit(AppEvent::CompleteSlashCommand(
                                    completion.insertion,
                                    completion.provider,
                                ));
                                return Task::none();
                            }
                            None => {
                                self.overlays.slash.close();
                                self.sync_focus();
                            }
                        }
                    }
                    if normal
                        && matches!(
                            key.as_ref(),
                            keyboard::Key::Named(keyboard::key::Named::Escape)
                        )
                    {
                        self.overlays.slash.close();
                        self.sync_focus();
                        return Task::none();
                    }
                }
                let composer_was_active = self.interaction.focus.context()
                    == KeybindingContext::Composer
                    && self.keybindings.is_composer_active();
                let mode_before = self.keybindings.mode();
                let action = self.keybindings.handle_in_context(
                    &key,
                    physical_key,
                    modifiers,
                    text.as_deref(),
                    DispatchContext {
                        terminal_available: self.active_terminal.is_some(),
                        composer_available: self.active_agent.is_some(),
                        ..DispatchContext::focused(self.interaction.focus.context())
                    },
                );
                let mode_after = self.keybindings.mode();
                let leaving_visual_mode = mode_before == Mode::Visual && mode_after != Mode::Visual;
                self.publish_input_mode();
                if mode_before != Mode::Visual
                    && mode_after == Mode::Visual
                    && self.interaction.focus.context() == KeybindingContext::Composer
                    && let Some(agent) = self.active_agent_mut()
                {
                    agent.prompt_selection_anchor = Some(agent.prompt_cursor);
                }
                self.emit(AppEvent::Action(action));
                if leaving_visual_mode && let Some(agent) = self.active_agent_mut() {
                    agent.prompt_selection_anchor = None;
                    agent.prompt_selected = false;
                }
                // The prompt itself is edited by the queued `Action`, which
                // refreshes the list once the text has actually changed. Only
                // the mode transition is settled here.
                self.refresh_slash_completions();
                if !composer_was_active
                    && self.interaction.focus.context() == KeybindingContext::Composer
                    && self.keybindings.is_composer_active()
                {
                    self.emit(AppEvent::EnterComposer);
                }
            }
            AppEvent::Keyboard(_) => {}
            AppEvent::MouseClick => {
                self.mouse_notice_until = Some(Instant::now() + Duration::from_secs(30));
            }
            AppEvent::Tick(now) => {
                self.handle_rpc_calls();
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
                for event in self.plugin_installs.poll() {
                    self.emit(AppEvent::PluginInstall(event));
                }
                let runtime_events = self
                    .agents
                    .iter_mut()
                    .filter_map(AgentView::drain_runtime_events)
                    .collect::<Vec<_>>();
                for (conversation_id, events) in runtime_events {
                    self.emit(AppEvent::AgentRuntime {
                        conversation_id,
                        events,
                    });
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

    fn subscription(&self) -> Subscription<AppEvent> {
        Subscription::batch([
            keyboard::listen().map(AppEvent::Keyboard),
            event::listen_with(|event, _status, _window| match event {
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_)) => {
                    Some(AppEvent::MouseClick)
                }
                _ => None,
            }),
            window::events().map(|(id, event)| AppEvent::WindowEvent(id, event)),
            time::every(RUNTIME_POLL_INTERVAL).map(AppEvent::Tick),
        ])
    }

    fn apply(&mut self, action: Action) {
        // Prompt edits are effects on the composer's text, so they announce
        // themselves once the text has settled rather than leaving every
        // prompt-derived affordance to guess when to re-read it. The event is
        // published before the edit runs and reduced after it, because the bus
        // drains once this action has been reduced.
        if edits_prompt(&action) {
            self.emit(AppEvent::ComposerPromptChanged);
        }
        match action {
            Action::None => {}
            Action::FocusRight => self.emit(AppEvent::FocusNext),
            Action::FocusLeft => self.emit(AppEvent::FocusPrevious),
            Action::WorktreePrevious => {
                self.emit(AppEvent::SelectWorktree(
                    self.active_worktree.saturating_sub(1),
                ));
            }
            Action::WorktreeNext => {
                self.emit(AppEvent::SelectWorktree(
                    self.active_worktree
                        .saturating_add(1)
                        .min(self.worktrees.len().saturating_sub(1)),
                ));
            }
            Action::WorktreeSelect(index) => {
                if index < self.worktrees.len() {
                    self.emit(AppEvent::SelectWorktree(index));
                }
            }
            Action::ToggleActivity(activity) => {
                self.emit(AppEvent::ToggleActivity(match activity {
                    Activity::Sessions => SidebarTool::Sessions,
                    Activity::Explorer => SidebarTool::Explorer,
                    Activity::Mcp => SidebarTool::Mcp,
                    Activity::Diffs => {
                        self.emit(AppEvent::ToggleDiffActivity);
                        return;
                    }
                    Activity::FileViewer => {
                        self.emit(AppEvent::ToggleFileViewer);
                        return;
                    }
                }))
            }
            Action::ToggleSettings => {
                self.emit(AppEvent::ToggleSettings);
            }
            Action::NewSession => {
                self.emit(AppEvent::StartAgent(self.default_agent));
            }
            Action::ToggleAgentMenu => self.emit(AppEvent::ToggleAgentMenu),
            Action::AgentMenuClose => self.emit(AppEvent::CloseAgentMenu),
            Action::AgentMenuPrevious => {
                self.emit(AppEvent::MoveAgentMenu(AgentMenuMotion::Previous));
            }
            Action::AgentMenuNext => self.emit(AppEvent::MoveAgentMenu(AgentMenuMotion::Next)),
            Action::AgentMenuFirst => self.emit(AppEvent::MoveAgentMenu(AgentMenuMotion::First)),
            Action::AgentMenuLast => self.emit(AppEvent::MoveAgentMenu(AgentMenuMotion::Last)),
            Action::AgentMenuConfirm => {
                match self.agent_menu.highlighted(&self.configured_agents) {
                    Some(provider) => self.emit(AppEvent::SelectAgent(provider)),
                    None => self.emit(AppEvent::CloseAgentMenu),
                }
            }
            Action::EnterComposer => self.emit(AppEvent::EnterComposer),
            Action::EnterTerminal => self.emit(AppEvent::TerminalVisibilityChanged(true)),
            Action::ToolbarPrevious => {
                let ordered = self.ordered_session_indices();
                let position = ordered
                    .iter()
                    .position(|index| *index == self.toolbar.selected_session)
                    .unwrap_or_default();
                self.toolbar.selected_session = ordered
                    .get(position.saturating_sub(1))
                    .copied()
                    .unwrap_or_default();
            }
            Action::ToolbarNext => {
                let ordered = self.ordered_session_indices();
                let position = ordered
                    .iter()
                    .position(|index| *index == self.toolbar.selected_session)
                    .unwrap_or_default();
                self.toolbar.selected_session = ordered
                    .get(position.saturating_add(1))
                    .copied()
                    .unwrap_or(self.toolbar.selected_session);
            }
            Action::ToolbarFirst => {
                self.toolbar.selected_session = self
                    .ordered_session_indices()
                    .first()
                    .copied()
                    .unwrap_or_default();
            }
            Action::ToolbarLast => {
                self.toolbar.selected_session = self
                    .ordered_session_indices()
                    .last()
                    .copied()
                    .unwrap_or_default();
            }
            Action::ToolbarOpen => {
                if !self.sessions().records().is_empty() {
                    self.emit(AppEvent::ResumeSession(self.toolbar.selected_session));
                }
            }
            Action::ToolbarTrash => {
                if !self.sessions().records().is_empty() {
                    self.request_session_trash(self.toolbar.selected_session);
                }
            }
            Action::ExplorerPrevious => {
                self.explorer.selected = self.explorer.selected.saturating_sub(1);
            }
            Action::ExplorerNext => {
                self.explorer.selected = self
                    .explorer
                    .selected
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
            Action::DiffOpen => {
                let mut opened = false;
                self.update_diff_state(|state| {
                    if !state.artifacts.is_empty() {
                        opened = true;
                        state.viewer_visible = true;
                        state.viewer_scroll = 0;
                    }
                });
                if opened {
                    self.emit(AppEvent::FocusRequested(FOCUS_DIFF_VIEWER));
                }
            }
            Action::DiffScrollUp => self.scroll_diff(false),
            Action::DiffScrollDown => self.scroll_diff(true),
            Action::DiffClose => {
                self.update_diff_state(|state| state.viewer_visible = false);
                self.emit(AppEvent::FocusRequested(FOCUS_DIFF_ACTIVITY));
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
                    agent.insert_prompt_text(&text);
                }
            }
            Action::AgentBackspace => {
                if let Some(agent) = self.active_agent_mut() {
                    if !agent.delete_prompt_selection() {
                        let previous = previous_char_boundary(&agent.prompt, agent.prompt_cursor);
                        agent.prompt.drain(previous..agent.prompt_cursor);
                        agent.prompt_cursor = previous;
                    }
                    if agent.prompt.is_empty() {
                        agent.command_provider = None;
                    }
                }
            }
            Action::AgentPaste => self.paste_into_agent(),
            Action::AgentSelectAll => {
                if let Some(agent) = self.active_agent_mut() {
                    agent.prompt_selected = !agent.prompt.is_empty();
                    agent.prompt_selection_anchor = (!agent.prompt.is_empty()).then_some(0);
                    agent.prompt_cursor = agent.prompt.len();
                }
            }
            Action::AgentMove(motion) => {
                let visual = self.keybindings.mode() == Mode::Visual;
                if let Some(agent) = self.active_agent_mut() {
                    agent.prompt_cursor =
                        composer_motion_target(&agent.prompt, agent.prompt_cursor, motion);
                    agent.prompt_selected = visual
                        && agent
                            .prompt_selection_anchor
                            .is_some_and(|anchor| anchor != agent.prompt_cursor);
                }
            }
            Action::AgentOperate(operator, motion) => {
                let selected = self.active_agent().and_then(|agent| {
                    composer_operation_range(&agent.prompt, agent.prompt_cursor, motion)
                        .map(|(start, end)| (start, end, agent.prompt[start..end].to_owned()))
                });
                if let Some((start, end, text)) = selected {
                    if matches!(
                        operator,
                        keybindings::ComposerOperator::Delete
                            | keybindings::ComposerOperator::Change
                    ) && let Some(agent) = self.active_agent_mut()
                    {
                        agent.prompt.drain(start..end);
                        agent.prompt_cursor = start.min(agent.prompt.len());
                        agent.command_provider = None;
                    }
                    match arboard::Clipboard::new()
                        .and_then(|mut clipboard| clipboard.set_text(text))
                    {
                        Ok(()) => {}
                        Err(error) => {
                            self.notice = Some(format!("Could not copy prompt text: {error}"))
                        }
                    }
                }
            }
            Action::AgentOperateSelection(operator) => {
                let selected = self.active_agent().and_then(|agent| {
                    agent
                        .prompt_selection()
                        .map(|(start, end)| (start, end, agent.prompt[start..end].to_owned()))
                });
                if let Some((start, end, text)) = selected {
                    if matches!(
                        operator,
                        keybindings::ComposerOperator::Delete
                            | keybindings::ComposerOperator::Change
                    ) && let Some(agent) = self.active_agent_mut()
                    {
                        agent.prompt.drain(start..end);
                        agent.prompt_cursor = start.min(agent.prompt.len());
                        agent.command_provider = None;
                    }
                    match arboard::Clipboard::new()
                        .and_then(|mut clipboard| clipboard.set_text(text))
                    {
                        Ok(()) => {}
                        Err(error) => {
                            self.notice = Some(format!("Could not copy prompt text: {error}"))
                        }
                    }
                }
            }
            Action::AgentDeleteChar => {
                if let Some(agent) = self.active_agent_mut() {
                    let end = next_char_boundary(&agent.prompt, agent.prompt_cursor);
                    agent.prompt.drain(agent.prompt_cursor..end);
                    agent.command_provider = None;
                }
            }
            Action::AgentInsertAtLineStart => {
                if let Some(agent) = self.active_agent_mut() {
                    agent.prompt_cursor = composer_motion_target(
                        &agent.prompt,
                        agent.prompt_cursor,
                        keybindings::ComposerMotion::LineStart,
                    );
                }
            }
            Action::AgentAppendAtCursor => {
                if let Some(agent) = self.active_agent_mut() {
                    agent.prompt_cursor = next_char_boundary(&agent.prompt, agent.prompt_cursor);
                }
            }
            Action::AgentAppendAtLineEnd => {
                if let Some(agent) = self.active_agent_mut() {
                    agent.prompt_cursor = composer_motion_target(
                        &agent.prompt,
                        agent.prompt_cursor,
                        keybindings::ComposerMotion::LineEnd,
                    );
                }
            }
            Action::AgentSubmit => {
                self.submit_agent_input();
            }
            Action::TerminalInput(bytes) => {
                if let Some(terminal) = self.active_terminal() {
                    terminal.send(bytes);
                }
            }
        }
    }

    /// Sends `prompt` to `provider`, switching agents first when it belongs to
    /// the one that is not focused. An accepted completion and a resolved
    /// submission share this, because they must route identically: the only
    /// difference between them is how the provider was learned.
    fn route_agent_command(&mut self, provider: Provider, prompt: String) {
        if self
            .active_agent()
            .is_some_and(|agent| command_needs_agent_switch(agent.session.provider(), provider))
        {
            self.start_agent(provider);
            if !self
                .active_agent()
                .is_some_and(|agent| agent.session.provider() == provider)
            {
                return;
            }
        }
        // Set unconditionally: a resolved submission rewrites the token to the
        // entry's own insertion, so the composer's text is not what should be
        // sent even when no switch happened.
        if let Some(agent) = self.active_agent_mut() {
            agent.prompt = normalized_prompt(prompt);
            agent.prompt_cursor = agent.prompt.len();
            agent.prompt_selection_anchor = None;
            agent.command_provider = Some(provider);
        }
        let workspace = self.active_agent().map(|agent| agent.workspace.clone());
        let submitted = self.active_agent_mut().and_then(AgentView::submit);
        if let Some((provider, id, name)) = submitted
            && let Some(workspace) = workspace
            && let Err(error) = self
                .workspaces
                .state_mut(&workspace)
                .registry
                .name_if_missing(provider, &id, name)
        {
            self.notice = Some(error);
        }
    }

    fn submit_agent_input(&mut self) {
        let Some(agent) = self.active_agent() else {
            return;
        };
        let prompt = agent.prompt.trim().to_owned();
        let has_images = !agent.images.is_empty();
        let command_provider = agent.command_provider;
        if let Some(provider) = command_provider {
            if has_images {
                self.notice = Some(AGENT_COMMAND_IMAGE_ATTACHMENT_NOTICE.to_owned());
                return;
            }
            self.route_agent_command(provider, prompt);
            return;
        }

        let active = self.active_agent().map(|agent| agent.session.provider());
        match resolve_submission(&self.slash_command_catalog, &prompt, active) {
            Err(error) => self.notice = Some(error),
            Ok(Submission::Agency(_)) if has_images => {
                self.notice = Some("Slash commands cannot include image attachments".to_owned());
            }
            Ok(Submission::Agency(command)) => {
                if let Err(error) = self.run_slash_command(command) {
                    self.notice = Some(error);
                    return;
                }
                if let Some(agent) = self.active_agent_mut() {
                    agent.clear_prompt();
                }
            }
            Ok(Submission::Agent { provider, prompt }) => {
                if has_images {
                    self.notice = Some(AGENT_COMMAND_IMAGE_ATTACHMENT_NOTICE.to_owned());
                    return;
                }
                self.route_agent_command(provider, prompt);
            }
            Ok(Submission::Verbatim) => {
                let workspace = self.active_agent().map(|agent| agent.workspace.clone());
                let submitted = self.active_agent_mut().and_then(AgentView::submit);
                if let Some((provider, id, name)) = submitted
                    && let Some(workspace) = workspace
                    && let Err(error) = self
                        .workspaces
                        .state_mut(&workspace)
                        .registry
                        .name_if_missing(provider, &id, name)
                {
                    self.notice = Some(error);
                }
            }
        }
    }

    fn run_slash_command(&mut self, command: SlashCommand) -> Result<(), String> {
        match command {
            SlashCommand::Init => {
                let created = initialize_workspace(&self.cwd)?;
                let default_agent = self.default_agent;
                let agent_count = self.agents.len();
                self.start_agent(default_agent);
                if self.agents.len() == agent_count {
                    let detail = self
                        .notice
                        .take()
                        .unwrap_or_else(|| "unknown startup error".to_owned());
                    return Err(format!(
                        "Created workspace files, but could not start the default {} agent: {detail}",
                        default_agent.label()
                    ));
                }
                let Some(agent) = self.active_agent_mut() else {
                    return Err(format!(
                        "Created workspace files, but could not start the default {} agent",
                        default_agent.label()
                    ));
                };
                if agent.session.provider() != default_agent {
                    return Err(format!(
                        "Created workspace files, but could not start the default {} agent",
                        default_agent.label()
                    ));
                }
                agent.prompt = INIT_AGENT_PROMPT.to_owned();
                agent.prompt_cursor = agent.prompt.len();
                agent.prompt_selection_anchor = None;
                let _ = agent.submit();
                let created_summary = if created.is_empty() {
                    "workspace files already existed".to_owned()
                } else {
                    format!(
                        "created {}",
                        created
                            .iter()
                            .map(|path| {
                                path.strip_prefix(&self.cwd)
                                    .unwrap_or(path)
                                    .display()
                                    .to_string()
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                self.notice = Some(format!(
                    "Initializing with {} ({created_summary})",
                    default_agent.label()
                ));
                Ok(())
            }
            SlashCommand::McpAdd { name } => self.add_mcp_server(&name),
            SlashCommand::PluginInstall { source, agent } => self.install_plugin(source, agent),
        }
    }

    fn install_plugin(&mut self, source: String, agent: Option<Provider>) -> Result<(), String> {
        let Some(conversation_id) = self
            .active_agent()
            .map(|agent| agent.conversation_id.clone())
        else {
            return Err("Start an agent before installing a plugin".to_owned());
        };
        let targets = match agent {
            Some(provider) if !self.configured_agents.contains(&provider) => {
                return Err(format!("{} is not configured", provider.label()));
            }
            Some(provider) => vec![provider],
            None if self.configured_agents.is_empty() => {
                return Err("No configured agents were found".to_owned());
            }
            None => self.configured_agents.clone(),
        };

        self.emit(AppEvent::PluginInstallRequested {
            conversation_id,
            source,
            targets,
        });
        Ok(())
    }

    fn add_mcp_server(&mut self, name: &str) -> Result<(), String> {
        if name == "agency" {
            return Err("The MCP server name \"agency\" is reserved".to_owned());
        }
        if self.mcp_servers().iter().any(|server| server.name == name) {
            return Err(format!("MCP server {name:?} is already connected"));
        }
        let server = load_codex_mcp(name)?;
        self.connect_mcp_server(name, server)
    }

    /// Everything after Codex has confirmed the server exists, split out from
    /// `add_mcp_server` so a test can exercise the reconnect-and-refresh
    /// behavior without shelling out to a real `codex` binary.
    fn connect_mcp_server(&mut self, name: &str, server: McpServer) -> Result<(), String> {
        if !server.enabled {
            return Err(format!("MCP server {name:?} is disabled in Codex"));
        }
        if self
            .agents
            .iter()
            .any(|agent| agent.activity.is_busy() || !agent.queued_messages.is_empty())
        {
            return Err(
                "Wait for every agent to become idle before changing MCP servers".to_owned(),
            );
        }
        if self.agents.iter().any(|agent| {
            agent.workspace == self.cwd
                && agent.session.provider() == Provider::Claude
                && agent.session_id.is_none()
        }) {
            return Err(
                "Wait for every Claude Code agent to finish connecting before changing MCP servers"
                    .to_owned(),
            );
        }

        let mut servers = self.mcp_servers().to_vec();
        servers.push(server);
        let reconnect = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, agent)| {
                agent.session.provider() == Provider::Claude && agent.workspace == self.cwd
            })
            .map(|(index, agent)| {
                (
                    index,
                    agent.rpc_token.clone(),
                    agent.conversation_id.clone(),
                    agent.session_id.clone().unwrap_or_default(),
                    agent.workspace.clone(),
                )
            })
            .collect::<Vec<_>>();
        let reconnected = reconnect.len();
        for (index, rpc_token, conversation_id, session_id, workspace) in reconnect {
            let environment = self.rpc_environment(&rpc_token, &conversation_id);
            let session = AgentSession::resume_with_env_and_mcps(
                Provider::Claude,
                &session_id,
                &workspace,
                &environment,
                &servers,
            )?;
            let agent = &mut self.agents[index];
            agent.session = session;
            agent.status = "Reconnecting MCP servers".to_owned();
            agent.mcp_status = McpStatus::Waiting;
        }
        let cwd = self.cwd.clone();
        self.workspaces.state_mut(&cwd).mcp_servers = servers;
        self.emit(AppEvent::SlashCatalogRequested);
        self.notice = Some(format!(
            "Added MCP server {name:?} to {reconnected} connected agent{}",
            if reconnected == 1 { "" } else { "s" }
        ));
        Ok(())
    }

    fn activity_did_toggle(&mut self, tool: SidebarTool) {
        self.sync_focus();
        if let Some(target) = activity_focus_target(self.layout.toolbar_visible, tool) {
            self.emit(AppEvent::FocusRequested(target));
        }
    }

    fn toggle_terminal(&mut self) {
        if self.layout.terminal_visible {
            self.emit(AppEvent::TerminalVisibilityChanged(false));
        } else if self.active_terminal.is_some() {
            self.emit(AppEvent::TerminalVisibilityChanged(true));
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
        let opening = self
            .active_agent()
            .is_some_and(|agent| !agent.diff_state.activity_visible);
        if opening && self.file_viewer.visible {
            self.emit(AppEvent::ToggleFileViewer);
        }
        if self.layout.terminal_visible {
            self.keybindings.toggle_terminal_mode(true);
            self.emit(AppEvent::TerminalVisibilityChanged(false));
            if self
                .active_agent()
                .is_some_and(|agent| !agent.diff_state.activity_visible)
            {
                self.toggle_diff_activity();
            }
        } else {
            self.toggle_diff_activity();
        }
        if self
            .active_agent()
            .is_some_and(|agent| agent.diff_state.activity_visible)
        {
            self.emit(AppEvent::FocusRequested(FOCUS_DIFF_ACTIVITY));
        } else {
            self.sync_focus();
        }
    }

    fn toggle_file_viewer(&mut self) {
        if self.layout.terminal_visible {
            self.keybindings.toggle_terminal_mode(true);
            self.emit(AppEvent::TerminalVisibilityChanged(false));
        }
        let opening = !self.file_viewer.visible;
        if opening
            && self
                .active_agent()
                .is_some_and(|agent| agent.diff_state.activity_visible)
        {
            self.emit(AppEvent::ToggleDiffActivity);
        }
        let preferred = self
            .explorer_entries()
            .get(self.explorer.selected)
            .map(|entry| entry.path.clone());
        if let Err(error) = self.file_viewer.toggle(&self.cwd, preferred.as_deref()) {
            self.notice = Some(error);
        }
    }

    fn open_file_viewer(&mut self, path: PathBuf) {
        if self.layout.terminal_visible {
            self.keybindings.toggle_terminal_mode(true);
            self.emit(AppEvent::TerminalVisibilityChanged(false));
        }
        if self
            .active_agent()
            .is_some_and(|agent| agent.diff_state.activity_visible)
        {
            self.emit(AppEvent::ToggleDiffActivity);
        }
        match self.file_viewer.open(path) {
            Ok(()) => self.file_viewer.visible = true,
            Err(error) => self.notice = Some(error),
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

    /// Replaces `worktrees` wholesale. `worktree_removed` is the only other
    /// writer of that field after startup, and it drops a single tab rather
    /// than replacing the list; `select_worktree` moves `active_worktree`
    /// alone, always in lockstep with `cwd`. This is the one writer that can
    /// leave the two diverged, when `cwd` is absent from the incoming list. An
    /// empty list is refused rather than rendered: git always reports at least
    /// the primary, so an empty result means the query failed, and dropping
    /// every tab would leave nothing to switch back to.
    fn worktrees_discovered(&mut self, worktrees: Vec<Worktree>) {
        if worktrees.is_empty() {
            return;
        }
        self.active_worktree = worktrees
            .iter()
            .position(|worktree| worktree.path == self.cwd)
            .unwrap_or(0);
        self.worktrees = worktrees;
    }

    /// Re-discovers rather than pushing the new worktree onto the list, so the
    /// tab strip stays exactly what git reports. A worktree created in some
    /// other repository simply will not appear, which is the correct outcome
    /// and needs no workspace comparison to arrange.
    fn worktree_created(&mut self, worktree: Worktree) {
        match worktrees::discover(&self.cwd) {
            Ok(discovered) => {
                self.worktrees_discovered(discovered);
                self.notice = Some(format!("Created worktree {}", worktree.label));
            }
            Err(error) => self.notice = Some(error),
        }
    }

    /// Drops the tab. If the user was looking at it, the move to the primary is
    /// published as a follow-up event rather than called directly, so ordering
    /// stays deterministic and `select_worktree`'s teardown — revoking RPC
    /// capabilities, clearing agents, reloading sessions — runs exactly once.
    fn worktree_removed(&mut self, removed: &Worktree) {
        let was_active = self
            .worktrees
            .get(self.active_worktree)
            .is_some_and(|worktree| worktree.path == removed.path);
        self.worktrees
            .retain(|worktree| worktree.path != removed.path);
        if self.worktrees.is_empty() {
            return;
        }
        self.active_worktree = self
            .worktrees
            .iter()
            .position(|worktree| worktree.path == self.cwd)
            .unwrap_or(0);
        if was_active {
            self.emit(AppEvent::SelectWorktree(0));
        }
        self.notice = Some(format!("Removed worktree {}", removed.label));
    }

    fn select_worktree(&mut self, index: usize) {
        let Some(worktree) = self.worktrees.get(index) else {
            return;
        };
        if worktree.path == self.cwd {
            return;
        }

        let cwd = worktree.path.clone();
        if let Err(error) = self.workspaces.ensure(&cwd) {
            self.notice = Some(error);
            return;
        }

        self.active_worktree = index;
        self.cwd = cwd;
        self.slash_command_catalog = agency_commands();
        self.emit(AppEvent::SlashCatalogRequested);
        self.overlays.slash.close();
        for agent in &self.agents {
            self.rpc_capabilities.revoke(&agent.rpc_token);
        }
        self.agents.clear();
        self.active_agent = None;
        self.emit(AppEvent::TerminalVisibilityChanged(false));
        self.active_terminal = self
            .terminals
            .iter()
            .position(|terminal| terminal.cwd() == self.cwd);
        self.explorer.selected = 0;
        self.explorer.expanded.clear();
        self.toolbar.selected_session = 0;
        self.overlays.pending_session_trash = None;
        self.notice = Some(format!("Switched to {}", self.cwd.display()));
    }

    fn explorer_entries(&self) -> Vec<ExplorerEntry> {
        let mut entries = Vec::new();
        collect_explorer_entries(
            &self.cwd,
            &self.cwd,
            0,
            &self.explorer.expanded,
            &mut entries,
        );
        entries
    }

    fn toggle_explorer_entry(&mut self, index: usize) {
        self.explorer.selected = index;
        self.toggle_selected_explorer_entry();
    }

    fn expand_explorer_entry(&mut self) {
        let Some(entry) = self.explorer_entries().get(self.explorer.selected).cloned() else {
            return;
        };
        if entry.directory {
            self.explorer.expanded.insert(entry.path);
        }
    }

    fn toggle_selected_explorer_entry(&mut self) {
        let Some(entry) = self.explorer_entries().get(self.explorer.selected).cloned() else {
            return;
        };
        if entry.directory && !self.explorer.expanded.remove(&entry.path) {
            self.explorer.expanded.insert(entry.path);
        } else if !entry.directory {
            self.emit(AppEvent::OpenFileViewer(entry.path));
        }
    }

    fn collapse_explorer_entry(&mut self) {
        let entries = self.explorer_entries();
        let Some(entry) = entries.get(self.explorer.selected) else {
            return;
        };
        if entry.directory && self.explorer.expanded.remove(&entry.path) {
            return;
        }
        if let Some(parent) = entry.path.parent()
            && let Some(index) = entries
                .iter()
                .position(|candidate| candidate.path == parent)
        {
            self.explorer.selected = index;
        }
    }

    fn start_terminal(&mut self, program: Program) {
        match TerminalSession::spawn(&mut self.multiplexer, program, &self.cwd) {
            Ok(terminal) => {
                self.terminals.push(terminal);
                self.active_terminal = Some(self.terminals.len() - 1);
                self.emit(AppEvent::TerminalVisibilityChanged(true));
                self.notice = None;
            }
            Err(error) => {
                self.emit(AppEvent::TerminalVisibilityChanged(false));
                self.notice = Some(error);
            }
        }
    }

    fn start_agent(&mut self, provider: Provider) {
        let workspace = self.cwd.clone();
        match self.start_session_in(provider, workspace, None) {
            Ok(_) => {
                self.active_agent = Some(self.agents.len() - 1);
                // The completion ranking depends on the focused agent's
                // provider, so a highlighted row that survived the switch
                // could silently point at a different command.
                self.overlays.slash.close();
                self.emit(AppEvent::TerminalVisibilityChanged(false));
                self.emit(AppEvent::EnterComposer);
                self.notice = None;
            }
            Err(error) => self.notice = Some(error),
        }
    }

    /// Spawns a session in `workspace` and, when `initial_prompt` is set, sends
    /// it as the session's first message. Focus is left alone on purpose: a
    /// session started by a tool call belongs to the worktree it was started
    /// in, not to whatever the user is looking at.
    fn start_session_in(
        &mut self,
        provider: Provider,
        workspace: PathBuf,
        initial_prompt: Option<String>,
    ) -> Result<String, String> {
        self.workspaces.ensure(&workspace)?;
        let conversation_id = new_conversation_id();
        let rpc_token = self.issue_rpc_capability(&conversation_id, provider, &workspace)?;
        let environment = self.rpc_environment(&rpc_token, &conversation_id);
        let mcp_servers = self.workspaces.state(&workspace).mcp_servers.to_vec();
        let session = match AgentSession::spawn_with_env_and_mcps(
            provider,
            &workspace,
            &environment,
            &mcp_servers,
        ) {
            Ok(session) => session,
            Err(error) => {
                self.rpc_capabilities.revoke(&rpc_token);
                return Err(error);
            }
        };
        let session_directory = self
            .workspaces
            .state(&workspace)
            .registry
            .session_directory(&conversation_id);
        let diff_state = DiffSessionState::load(&session_directory).unwrap_or_default();
        self.agents.push(AgentView {
            workspace,
            conversation_id: conversation_id.clone(),
            rpc_token,
            session,
            transcript: Vec::new(),
            transcript_dirty: false,
            conversation: Conversation::default(),
            prompt: String::new(),
            prompt_selected: false,
            prompt_cursor: 0,
            prompt_selection_anchor: None,
            command_provider: None,
            images: Vec::new(),
            pending_question: None,
            status: "Initializing".to_owned(),
            session_id: None,
            pending_session_name: None,
            pending_conversation_id: Some(conversation_id.clone()),
            completed_turns: 0,
            activity: AgentActivity::Starting,
            queued_messages: VecDeque::new(),
            image_cache: HashMap::new(),
            image_cache_directory: session_directory.join("images"),
            diff_state,
            session_directory,
            last_changed_at_millis: unix_time_millis(),
            mcp_status: McpStatus::Waiting,
            plugin_installs: TranscriptInstalls::default(),
        });
        if let Some(prompt) = initial_prompt
            && let Some(agent) = self.agents.last_mut()
        {
            agent.prompt = normalized_prompt(prompt);
            agent.prompt_cursor = agent.prompt.len();
            agent.prompt_selection_anchor = None;
            agent.submit();
        }
        Ok(conversation_id)
    }

    fn issue_rpc_capability(
        &self,
        conversation_id: &str,
        provider: Provider,
        workspace: &Path,
    ) -> Result<String, String> {
        if self.rpc_server.is_none() {
            return Err("Agency RPC is unavailable".to_owned());
        }
        self.rpc_capabilities.issue(SessionContext {
            conversation_id: conversation_id.to_owned(),
            workspace: workspace.to_path_buf(),
            provider: match provider {
                Provider::Codex => "codex",
                Provider::Claude => "claude",
            }
            .to_owned(),
            provider_session_id: None,
            generation: 1,
        })
    }

    fn rpc_environment(&self, token: &str, conversation_id: &str) -> Vec<(String, String)> {
        let Some(server) = &self.rpc_server else {
            return Vec::new();
        };
        vec![
            (
                ENV_RPC_SOCKET.to_owned(),
                server.socket_path().to_string_lossy().into_owned(),
            ),
            (ENV_SESSION_TOKEN.to_owned(), token.to_owned()),
            (ENV_CONVERSATION_ID.to_owned(), conversation_id.to_owned()),
            (
                ENV_MCP_COMMAND.to_owned(),
                agency_mcp_command().to_string_lossy().into_owned(),
            ),
        ]
    }

    fn handle_rpc_calls(&mut self) {
        let calls = self
            .rpc_server
            .as_ref()
            .map(|server| server.try_calls().collect::<Vec<_>>())
            .unwrap_or_default();
        for call in calls {
            let result = match call.method.as_str() {
                "worktree.list" => worktrees::discover(&call.context.workspace).map(|worktrees| {
                    serde_json::json!({
                        "caller": rpc_caller(&call.context),
                        "worktrees": worktrees.into_iter().map(worktree_json).collect::<Vec<_>>()
                    })
                }),
                "worktree.create" => {
                    let branch = call
                        .params
                        .get("branch")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "create_worktree requires a branch".to_owned());
                    branch.and_then(|branch| {
                        let base = call.params.get("base").and_then(serde_json::Value::as_str);
                        worktrees::create(&call.context.workspace, branch, base).map(|worktree| {
                            let value = worktree_json(worktree.clone());
                            self.emit(AppEvent::WorktreeCreated { worktree });
                            serde_json::json!({
                                "caller": rpc_caller(&call.context),
                                "worktree": value
                            })
                        })
                    })
                }
                "worktree.remove" => {
                    let branch = call
                        .params
                        .get("branch")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "remove_worktree requires a branch".to_owned());
                    branch.and_then(|branch| {
                        worktrees::remove(&call.context.workspace, branch).map(|worktree| {
                            self.emit(AppEvent::WorktreeRemoved {
                                worktree: worktree.clone(),
                            });
                            let value = worktree_json(worktree);
                            serde_json::json!({
                                "caller": rpc_caller(&call.context),
                                "worktree": value
                            })
                        })
                    })
                }
                "mcp.status" => {
                    let connected = call
                        .params
                        .get("connected")
                        .and_then(serde_json::Value::as_bool)
                        .ok_or_else(|| "mcp.status requires connected".to_owned());
                    connected.and_then(|connected| {
                        let status = if connected {
                            McpStatus::Connected
                        } else {
                            McpStatus::Disconnected
                        };
                        self.agents
                            .iter_mut()
                            .find(|agent| agent.conversation_id == call.context.conversation_id)
                            .map(|agent| {
                                agent.mcp_status = status;
                                serde_json::json!({ "connected": connected })
                            })
                            .ok_or_else(|| "MCP agent is no longer running".to_owned())
                    })
                }
                _ => Err(format!("Unknown Agency RPC method: {}", call.method)),
            };
            let _ = call.reply.send(match result {
                Ok(result) => RpcResponse::success(result),
                Err(error) => RpcResponse::error(error),
            });
        }
    }

    fn resume_session(&mut self, index: usize) {
        let Some(record) = self.sessions().records().get(index).cloned() else {
            return;
        };
        if let Some(running_index) = self
            .agents
            .iter()
            .position(|agent| agent.conversation_id == record.conversation_id)
        {
            self.toolbar.selected_session = index;
            self.selected_agent = self.agents[running_index].session.provider();
            self.active_agent = Some(running_index);
            // Same reasoning as `start_agent`: the focused agent just
            // changed, so a highlighted completion row cannot be trusted.
            self.overlays.slash.close();
            self.emit(AppEvent::TerminalVisibilityChanged(false));
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
        let rpc_token =
            match self.issue_rpc_capability(&record.conversation_id, provider, &self.cwd) {
                Ok(token) => token,
                Err(error) => {
                    self.notice = Some(error);
                    return;
                }
            };
        let environment = self.rpc_environment(&rpc_token, &record.conversation_id);
        match AgentSession::resume_with_env_and_mcps(
            provider,
            &id,
            &self.cwd,
            &environment,
            self.mcp_servers(),
        ) {
            Ok(session) => {
                self.toolbar.selected_session = index;
                let session_directory = self.sessions().session_directory(record.conversation_id());
                let diff_state = match DiffSessionState::load(&session_directory) {
                    Ok(state) => state,
                    Err(error) => {
                        self.notice = Some(error);
                        DiffSessionState::default()
                    }
                };
                self.agents.push(AgentView {
                    workspace: self.cwd.clone(),
                    conversation_id: record.conversation_id.clone(),
                    rpc_token,
                    session,
                    transcript: Vec::new(),
                    transcript_dirty: false,
                    conversation: Conversation::default(),
                    prompt: String::new(),
                    prompt_selected: false,
                    prompt_cursor: 0,
                    prompt_selection_anchor: None,
                    command_provider: None,
                    images: Vec::new(),
                    pending_question: None,
                    status: "Resuming".to_owned(),
                    session_id: Some(id),
                    pending_session_name: None,
                    pending_conversation_id: None,
                    completed_turns: 0,
                    activity: AgentActivity::Starting,
                    queued_messages: VecDeque::new(),
                    image_cache: HashMap::new(),
                    image_cache_directory: self
                        .sessions()
                        .session_directory(record.conversation_id())
                        .join("images"),
                    diff_state,
                    session_directory,
                    last_changed_at_millis: unix_time_millis(),
                    mcp_status: McpStatus::Waiting,
                    plugin_installs: TranscriptInstalls::default(),
                });
                self.active_agent = Some(self.agents.len() - 1);
                // Same reasoning as `start_agent`: the focused agent just
                // changed, so a highlighted completion row cannot be trusted.
                self.overlays.slash.close();
                self.emit(AppEvent::TerminalVisibilityChanged(false));
                self.notice = None;
            }
            Err(error) => {
                self.rpc_capabilities.revoke(&rpc_token);
                self.notice = Some(error);
            }
        }
    }

    /// Focus follows the agent menu: it borrows focus while it floats and hands
    /// it back to the surface it covered once it closes.
    fn agent_menu_focus_changed(&mut self) {
        self.sync_focus();
        if self.agent_menu.open {
            return;
        }
        let Some(target) = self.agent_menu.return_focus.take() else {
            return;
        };
        if self.interaction.focus.focus(target) {
            self.activate_focused_context();
        }
    }

    /// Switching agents rebinds the *current* Agency session to the chosen
    /// agent. The Agency conversation is the durable identity; the provider
    /// behind it is not, so switching never jumps to some other session that
    /// happens to be running under that agent.
    fn select_agent(&mut self, provider: Provider) {
        self.selected_agent = provider;
        let Some(index) = self.active_agent else {
            // No session to rebind: the switch opens one under the chosen agent.
            self.start_agent(provider);
            if self.notice.is_none() {
                self.notice = Some(format!("Started a {} session", provider.label()));
            }
            return;
        };
        if self.agents[index].session.provider() == provider {
            return;
        }
        self.rebind_session(index, provider);
    }

    /// Continues one Agency conversation under a different agent. A conversation
    /// keeps a separate binding per agent, so an agent it has already run under
    /// resumes with its own history and a new one starts clean. Either way the
    /// conversation, its session directory, and its diffs stay put.
    fn rebind_session(&mut self, index: usize, provider: Provider) {
        let conversation_id = self.agents[index].conversation_id.clone();
        let workspace = self.agents[index].workspace.clone();
        let binding = self
            .workspaces
            .state(&workspace)
            .registry
            .records()
            .iter()
            .find(|record| record.conversation_id() == conversation_id)
            .and_then(|record| record.binding(provider))
            .map(str::to_owned);

        let rpc_token = match self.issue_rpc_capability(&conversation_id, provider, &workspace) {
            Ok(token) => token,
            Err(error) => {
                self.notice = Some(error);
                return;
            }
        };
        let environment = self.rpc_environment(&rpc_token, &conversation_id);
        let mcp_servers = self.workspaces.state(&workspace).mcp_servers.to_vec();
        let started = match &binding {
            Some(session_id) => AgentSession::resume_with_env_and_mcps(
                provider,
                session_id,
                &workspace,
                &environment,
                &mcp_servers,
            ),
            None => AgentSession::spawn_with_env_and_mcps(
                provider,
                &workspace,
                &environment,
                &mcp_servers,
            ),
        };
        let session = match started {
            Ok(session) => session,
            Err(error) => {
                self.rpc_capabilities.revoke(&rpc_token);
                self.notice = Some(error);
                return;
            }
        };

        let previous_token = std::mem::replace(&mut self.agents[index].rpc_token, rpc_token);
        self.rpc_capabilities.revoke(&previous_token);

        let resumed = binding.is_some();
        let agent = &mut self.agents[index];
        agent.session = session;
        agent.session_id = binding;
        agent.pending_session_name = None;
        // A resumed binding is already recorded; a fresh one has to bind its new
        // provider session to this same Agency conversation once it reports in.
        agent.pending_conversation_id = (!resumed).then(|| conversation_id.clone());
        agent.status = if resumed { "Resuming" } else { "Initializing" }.to_owned();
        agent.activity = AgentActivity::Starting;
        agent.mcp_status = McpStatus::Waiting;
        agent.pending_question = None;
        agent.command_provider = None;
        agent.queued_messages.clear();
        agent.completed_turns = 0;
        agent.last_changed_at_millis = unix_time_millis();
        // A resume replays the agent's own history. A fresh start has none, so
        // the transcript is cleared rather than left implying the new agent can
        // see what the previous one was told.
        if !resumed {
            agent.conversation = Conversation::default();
            agent.transcript.clear();
        }
        agent.transcript_dirty = true;

        self.active_agent = Some(index);
        // The completion ranking follows the focused agent's provider, and
        // that provider just changed under this same index, so a highlighted
        // row would silently point at a different command.
        self.overlays.slash.close();
        if let Some(session) = self
            .sessions()
            .records()
            .iter()
            .position(|record| record.conversation_id() == conversation_id)
        {
            self.toolbar.selected_session = session;
        }
        self.emit(AppEvent::TerminalVisibilityChanged(false));
        self.emit(AppEvent::RefreshVisibleTranscript);
        self.notice = Some(if resumed {
            format!("Continuing this session with {}", provider.label())
        } else {
            format!("Switched this session to {}", provider.label())
        });
    }

    fn request_session_trash(&mut self, index: usize) {
        if index < self.sessions().records().len() {
            self.toolbar.selected_session = index;
            self.overlays.pending_session_trash = Some(index);
        }
    }

    fn confirm_session_trash(&mut self) {
        let Some(index) = self.overlays.pending_session_trash.take() else {
            return;
        };
        match self.sessions_mut().remove(index) {
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
                self.toolbar.selected_session =
                    index.min(self.sessions().records().len().saturating_sub(1));
                self.emit(AppEvent::RefreshVisibleTranscript);
                self.notice = Some(format!(
                    "Trashed session {}",
                    record.name.as_deref().unwrap_or(record.conversation_id())
                ));
            }
            Err(error) => {
                self.overlays.pending_session_trash = Some(index);
                self.notice = Some(error);
            }
        }
    }

    fn active_terminal(&self) -> Option<&TerminalSession> {
        self.active_terminal
            .and_then(|index| self.terminals.get(index))
    }

    /// The session registry of the worktree the user is looking at. Sessions
    /// running in other worktrees live in their own registries and are reached
    /// through `self.workspaces`.
    fn sessions(&self) -> &SessionRegistry {
        &self.workspaces.state(&self.cwd).registry
    }

    fn sessions_mut(&mut self) -> &mut SessionRegistry {
        let cwd = self.cwd.clone();
        &mut self.workspaces.state_mut(&cwd).registry
    }

    fn mcp_servers(&self) -> &[McpServer] {
        &self.workspaces.state(&self.cwd).mcp_servers
    }

    fn record_session_updates(&mut self, updates: Vec<SessionUpdate>) {
        for (provider, id, name, conversation_id, rpc_token) in updates {
            self.rpc_capabilities
                .bind_provider_session(&rpc_token, id.clone());
            let workspace = self
                .agents
                .iter()
                .find(|agent| agent.rpc_token == rpc_token)
                .map_or_else(|| self.cwd.clone(), |agent| agent.workspace.clone());
            let registry = &mut self.workspaces.state_mut(&workspace).registry;
            let result = if let Some(conversation_id) = conversation_id {
                registry.record_binding(conversation_id, provider, id, name)
            } else {
                registry.record(provider, id, name)
            };
            if let Err(error) = result {
                self.notice = Some(error);
            }
        }
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
        let mut indices = (0..self.sessions().records().len()).collect::<Vec<_>>();
        indices.sort_by_key(|index| {
            let session = &self.sessions().records()[*index];
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
                Ok(text) => agent.insert_prompt_text(&text),
                Err(error) => {
                    self.notice = Some(format!("Clipboard has no pasteable content: {error}"))
                }
            },
        }
    }

    fn view(&self) -> Element<'_, AppEvent> {
        let leader_pending = self.keybindings.is_leader_pending();
        let animation_elapsed = self.animation_started_at.elapsed();
        let worktree_tabs =
            self.worktrees
                .iter()
                .enumerate()
                .fold(row![].spacing(4), |tabs, (index, worktree)| {
                    let selected = index == self.active_worktree;
                    let label: Element<'_, AppEvent> = if leader_pending && index < 10 {
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
                            .on_press_maybe((!selected).then_some(AppEvent::SelectWorktree(index))),
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
                let session = &self.sessions().records()[index];
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
                let session_agent = self
                    .agents
                    .iter()
                    .find(|agent| agent.conversation_id == session.conversation_id);
                let agent_status =
                    session_agent.map_or(ui_theme::AgentStatus::Resume, AgentView::agent_status);
                let name = session.name.as_deref().unwrap_or("Untitled session");
                let badge = agent_status_badge(agent_status, animation_elapsed);
                let card =
                    button(column![badge, text(name).size(13), text(id).size(10),].spacing(3))
                        .width(Fill)
                        .padding(iced::Padding::from([8, 10]).right(38))
                        .style(move |_theme: &Theme, status| {
                            ui_theme::session_button(index == self.toolbar.selected_session, status)
                        });
                let card = if is_active {
                    card
                } else {
                    card.on_press(AppEvent::ResumeSession(index))
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
                    .on_press(AppEvent::RequestSessionTrash(index)),
                )
                .padding(6)
                .align_right(Fill)
                .align_top(Fill);
                sessions.push(stack![card, trash])
            },
        );
        let focused_element = self.interaction.focus.focused();
        let tool_button = |tool, icon: &'static [u8], hint: String| {
            let selected = self.layout.toolbar_visible && self.layout.sidebar_tool == tool;
            let focused = selected && activity_focus_target(true, tool) == Some(focused_element);
            let control = button(
                svg(svg::Handle::from_memory(icon))
                    .width(Length::Fixed(19.0))
                    .height(Length::Fixed(19.0))
                    .style(move |_theme: &Theme, _status| ui_theme::tool_icon(selected)),
            )
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(36.0))
            .padding(8)
            .style(move |_theme: &Theme, status| ui_theme::tool_button(selected, focused, status))
            .on_press(AppEvent::ToggleActivity(tool));
            let control: Element<'_, AppEvent> = control.into();
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
        let terminal_selected = self.layout.terminal_visible;
        let terminal_focused = terminal_selected && focused_element == FOCUS_TERMINAL;
        let terminal_control = button(
            svg(svg::Handle::from_memory(TERMINAL_ICON))
                .width(Length::Fixed(19.0))
                .height(Length::Fixed(19.0))
                .style(move |_theme: &Theme, _status| ui_theme::tool_icon(terminal_selected)),
        )
        .width(Length::Fixed(36.0))
        .height(Length::Fixed(36.0))
        .padding(8)
        .style(move |_theme: &Theme, status| {
            ui_theme::tool_button(terminal_selected, terminal_focused, status)
        })
        .on_press(AppEvent::ToggleTerminalActivity);
        let terminal_control: Element<'_, AppEvent> = if leader_pending {
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
        let settings_selected = self.layout.settings_open;
        let settings_control = button(
            svg(svg::Handle::from_memory(SETTINGS_ICON))
                .width(Length::Fixed(19.0))
                .height(Length::Fixed(19.0))
                .style(move |_theme: &Theme, _status| ui_theme::tool_icon(settings_selected)),
        )
        .width(Length::Fixed(36.0))
        .height(Length::Fixed(36.0))
        .padding(8)
        .style(move |_theme: &Theme, status| {
            ui_theme::tool_button(settings_selected, false, status)
        })
        .on_press(AppEvent::ToggleSettings);
        let settings_control: Element<'_, AppEvent> = if leader_pending {
            stack![
                settings_control,
                container(shortcut_badge(
                    self.keybindings.toggle_settings_hint().to_owned()
                ))
                .padding([2, 3])
                .align_right(Fill)
                .align_bottom(Fill),
            ]
            .into()
        } else {
            settings_control.into()
        };

        // The left activity bar contains worktree-scoped tools only.
        let worktree_activity_bar = container(
            column![
                column![
                    tool_button(
                        SidebarTool::Sessions,
                        MESSAGE_SQUARE_ICON,
                        self.keybindings.toggle_sessions_hint().to_owned(),
                    ),
                    tool_button(
                        SidebarTool::Explorer,
                        FOLDER_ICON,
                        self.keybindings.toggle_explorer_hint().to_owned(),
                    ),
                    tool_button(
                        SidebarTool::Mcp,
                        NETWORK_ICON,
                        self.keybindings.toggle_mcp_hint().to_owned(),
                    ),
                    terminal_control,
                ]
                .spacing(4),
                iced::widget::Space::new().height(Fill),
                settings_control,
            ]
            .height(Fill)
            .align_x(iced::Alignment::Center),
        )
        .width(Length::Fixed(48.0))
        .height(Fill)
        .padding([8, 6])
        .style(|_theme: &Theme| ui_theme::activity_bar());

        let panel: Element<'_, AppEvent> =
            match (self.layout.toolbar_visible, self.layout.sidebar_tool) {
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
                            let disclosure: Element<'_, AppEvent> = if is_directory {
                                let icon = if self.explorer.expanded.contains(&entry.path) {
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
                                    ui_theme::file_entry(index == self.explorer.selected, status)
                                })
                                .on_press(AppEvent::ToggleExplorerEntry(index)),
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
                            text("j/k navigate  ·  h/l fold  ·  Enter open  ·  <leader>d").size(10),
                        ]
                        .spacing(9),
                    )
                    .width(Length::Fixed(280.0))
                    .height(Fill)
                    .padding([14, 12])
                    .style(|_theme: &Theme| ui_theme::rail())
                    .into()
                }
                (true, SidebarTool::Mcp) => {
                    let agency_state =
                        agency_mcp_server_state(self.agents.iter().map(|agent| agent.mcp_status));
                    let agency_access_tags =
                        self.configured_agents
                            .iter()
                            .fold(row![].spacing(5), |tags, provider| {
                                tags.push(
                                    container(text(provider.label()).size(10))
                                        .padding([2, 5])
                                        .style(|_theme: &Theme| ui_theme::mcp_access_badge()),
                                )
                            });
                    let agency_access: Element<'_, AppEvent> = if self.configured_agents.is_empty()
                    {
                        text("No configured agents available").size(10).into()
                    } else {
                        agency_access_tags.into()
                    };
                    let agency_server = container(
                        column![
                            row![
                                text("agency").size(13),
                                iced::widget::Space::new().width(Fill),
                                container(text(agency_state.label()).size(10))
                                    .padding([2, 5])
                                    .style(move |_theme: &Theme| {
                                        ui_theme::mcp_server_state_badge(agency_state)
                                    }),
                            ]
                            .align_y(iced::Alignment::Center),
                            text("BUILT IN · AUTO-INJECTED").size(9),
                            agency_access,
                        ]
                        .spacing(7),
                    )
                    .width(Fill)
                    .padding([9, 10])
                    .style(|_theme: &Theme| ui_theme::mcp_agent_card());
                    let servers = self.mcp_servers().iter().fold(
                        column![agency_server].spacing(8),
                        |servers, server| {
                            let state = mcp_server_state(server, &self.agents);
                            let access_tags = self.configured_agents.iter().fold(
                                row![].spacing(5),
                                |tags, provider| {
                                    tags.push(
                                        container(text(provider.label()).size(10))
                                            .padding([2, 5])
                                            .style(|_theme: &Theme| ui_theme::mcp_access_badge()),
                                    )
                                },
                            );
                            let access: Element<'_, AppEvent> = if self.configured_agents.is_empty()
                            {
                                text("No configured agents available").size(10).into()
                            } else {
                                access_tags.into()
                            };
                            servers.push(
                                container(
                                    column![
                                        row![
                                            text(&server.name).size(13),
                                            iced::widget::Space::new().width(Fill),
                                            container(text(state.label()).size(10))
                                                .padding([2, 5])
                                                .style(move |_theme: &Theme| {
                                                    ui_theme::mcp_server_state_badge(state)
                                                }),
                                        ]
                                        .align_y(iced::Alignment::Center),
                                        text("AGENT ACCESS").size(9),
                                        access,
                                    ]
                                    .spacing(7),
                                )
                                .width(Fill)
                                .padding([9, 10])
                                .style(|_theme: &Theme| ui_theme::mcp_agent_card()),
                            )
                        },
                    );
                    let body: Element<'_, AppEvent> = scrollable(servers).height(Fill).into();
                    let server_count = self.mcp_servers().len() + 1;
                    container(
                        column![
                            sidebar_header("MCP", NETWORK_ICON),
                            rule::horizontal(1),
                            body,
                            text(format!(
                                "{server_count} MCP server{}",
                                if server_count == 1 { "" } else { "s" }
                            ))
                            .size(10),
                            text("<leader>m").size(10),
                        ]
                        .spacing(14),
                    )
                    .width(Length::Fixed(280.0))
                    .height(Fill)
                    .padding(16)
                    .style(|_theme: &Theme| ui_theme::rail())
                    .into()
                }
            };
        let rail_divider: Element<'_, AppEvent> = if self.layout.toolbar_visible {
            rule::vertical(1).into()
        } else {
            iced::widget::Space::new().width(Length::Shrink).into()
        };
        let panel_focused = self.layout.toolbar_visible
            && match self.layout.sidebar_tool {
                SidebarTool::Explorer => focused_element == FOCUS_EXPLORER,
                SidebarTool::Sessions | SidebarTool::Mcp => focused_element == FOCUS_TOOLBAR,
            };
        let toolbar: Element<'_, AppEvent> = row![
            worktree_activity_bar,
            rail_divider,
            toolbar_panel_surface(panel, self.layout.toolbar_visible, panel_focused)
        ]
        .height(Fill)
        .into();

        let agent_content: Element<'_, AppEvent> = if let Some(agent) = self.active_agent() {
            let input_active = self.interaction.focus.context() == KeybindingContext::Composer;
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
            let transcript: Element<'_, AppEvent> = if agent.transcript.is_empty()
                && self.keybindings.is_normal()
            {
                container(
                    button(
                        column![
                            text("Begin a conversation").size(20),
                            text("Press i to enter composer mode").size(13),
                        ]
                        .spacing(6)
                        .align_x(iced::Alignment::Center),
                    )
                    .padding([14, 20])
                    .style(|_theme: &Theme, status| ui_theme::dialog_button(false, status))
                    .on_press(AppEvent::EnterComposer),
                )
                .center_x(Fill)
                .center_y(Fill)
                .width(Fill)
                .height(Fill)
                .into()
            } else if agent.transcript.is_empty() {
                column![text("Waiting for agent…").size(15)]
                    .width(Fill)
                    .into()
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
                                        .map(AppEvent::LinkClicked),
                                ]
                                .spacing(10)
                                .align_y(iced::Alignment::Start),
                            )
                            .padding([4, 4]),
                        ),
                        TranscriptEntry::CommandExecution {
                            content,
                            output,
                            status,
                            exit_code,
                            ..
                        } => transcript
                            .push(command_execution_card(content, output, status, *exit_code)),
                        TranscriptEntry::FileChanges { status, changes } => transcript.push(
                            container(file_changes_card(status, changes))
                                .width(Fill)
                                .padding([4, 32]),
                        ),
                        TranscriptEntry::FileRead {
                            path,
                            status,
                            lines,
                        } => transcript.push(file_read_card(path, status, *lines)),
                        TranscriptEntry::WebSearch { queries } => {
                            transcript.push(web_search_card(queries))
                        }
                        TranscriptEntry::PluginInstall(install) => {
                            transcript.push(plugin_install_card(install))
                        }
                        TranscriptEntry::Activity(message) => transcript
                            .push(container(text(message).size(12).width(Fill)).padding([2, 4])),
                    })
                    .width(Fill)
                    .into()
            };
            let prompt = composer_prompt(
                agent,
                input_active && self.cursor_visible && !agent.prompt_selected,
            );
            let input_indicator = agent_status_badge(agent.agent_status(), animation_elapsed);
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
                                    .on_press(AppEvent::AnswerChoice(index))
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

            let transcript_view: Element<'_, AppEvent> =
                if agent.transcript.is_empty() && self.keybindings.is_normal() {
                    transcript
                } else {
                    scrollable(transcript)
                        .id(AGENT_TRANSCRIPT_ID)
                        .width(Fill)
                        .height(Fill)
                        .into()
                };
            let mut content = column![
                container(header).padding([10, 16]),
                rule::horizontal(1),
                container(transcript_view)
                    .width(Fill)
                    .padding(24)
                    .height(Fill),
                rule::horizontal(1),
            ];
            if let Some(question) = question {
                content = content.push(question).push(rule::horizontal(1));
            }
            let completions = slash_command_completions(
                &self.slash_command_catalog,
                &agent.prompt,
                Some(agent.session.provider()),
            )
            .into_iter()
            .enumerate()
            .fold(column![].spacing(4), |completions, (index, completion)| {
                let provider_tag: Element<'_, AppEvent> = completion.provider.map_or_else(
                    || {
                        container(text("AGENCY").size(10))
                            .padding([2, 6])
                            .style(|_theme: &Theme| ui_theme::agent_type_badge(true))
                            .into()
                    },
                    |provider| {
                        let label = if completion.built_in {
                            format!("BUILT-IN · {}", provider.label().to_uppercase())
                        } else {
                            provider.label().to_uppercase()
                        };
                        container(text(label).size(10))
                            .padding([2, 6])
                            .style(move |_theme: &Theme| {
                                ui_theme::agent_type_badge(provider == Provider::Codex)
                            })
                            .into()
                    },
                );
                completions.push(
                    button(
                        row![
                            provider_tag,
                            text(&completion.command).font(Font::MONOSPACE).size(14),
                            text(&completion.description).size(12),
                        ]
                        .spacing(16)
                        .align_y(iced::Alignment::Center),
                    )
                    .on_press(AppEvent::CompleteSlashCommand(
                        completion.insertion.clone(),
                        completion.provider,
                    ))
                    .width(Fill)
                    .style(move |_theme: &Theme, status| {
                        ui_theme::slash_command_button(
                            index == self.overlays.slash.selected(),
                            status,
                        )
                    }),
                )
            });
            if self.overlays.slash.is_open()
                && completion_count(&self.slash_command_catalog, &agent.prompt) > 0
            {
                let hint = row![
                    shortcut_badge("Tab".to_owned()),
                    text("completes").size(11),
                    shortcut_badge("↑↓".to_owned()),
                    text("selects").size(11),
                    shortcut_badge("Enter".to_owned()),
                    text("inserts").size(11),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center);
                content = content
                    .push(container(column![completions, hint].spacing(8)).padding([8, 16]))
                    .push(rule::horizontal(1));
            }
            let composer: Element<'_, AppEvent> = container({
                let mut composer_content =
                    column![input_indicator, text(input_details).size(12), prompt,].spacing(8);
                if self.interaction.input_mode.composer_needs_insert_hint() {
                    composer_content = composer_content.push(
                        row![
                            text("Press").size(12),
                            shortcut_badge("i".to_owned()),
                            text("to start typing").size(12),
                        ]
                        .spacing(6)
                        .align_y(iced::Alignment::Center),
                    );
                }
                composer_content
            })
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
                    button("Open repository").on_press(AppEvent::OpenRepository),
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

        let settings_content: Element<'_, AppEvent> = {
            let cards = self.agent_installations.iter().fold(
                column![].spacing(12),
                |cards, installation| {
                    let installed = installation.path.is_some();
                    let status = if installed { "DETECTED" } else { "NOT FOUND" };
                    let path = installation.path.as_ref().map_or_else(
                        || "Not found on PATH".to_owned(),
                        |path| path.display().to_string(),
                    );
                    let version = installation.version.as_deref().unwrap_or(if installed {
                        "Version unavailable"
                    } else {
                        "—"
                    });
                    let is_default = installation.provider == self.default_agent;
                    let default_control: Element<'_, AppEvent> = if is_default {
                        container(text("DEFAULT").size(10))
                            .padding([4, 8])
                            .style(|_theme: &Theme| ui_theme::agent_badge())
                            .into()
                    } else {
                        button("Set as default")
                            .padding([5, 9])
                            .style(|_theme: &Theme, status| ui_theme::dialog_button(false, status))
                            .on_press_maybe(
                                installed
                                    .then_some(AppEvent::SetDefaultAgent(installation.provider)),
                            )
                            .into()
                    };
                    cards.push(
                        container(
                            column![
                                row![
                                    column![
                                        text(installation.provider.label()).size(18),
                                        text(status).size(10),
                                    ]
                                    .spacing(3)
                                    .width(Fill),
                                    default_control,
                                ]
                                .align_y(iced::Alignment::Center),
                                rule::horizontal(1),
                                row![
                                    text("Executable").size(11).width(Length::Fixed(100.0)),
                                    text(path).font(Font::MONOSPACE).size(12),
                                ]
                                .spacing(12),
                                row![
                                    text("Version").size(11).width(Length::Fixed(100.0)),
                                    text(version).font(Font::MONOSPACE).size(12),
                                ]
                                .spacing(12),
                            ]
                            .spacing(11),
                        )
                        .width(Fill)
                        .padding(16)
                        .style(|_theme: &Theme| ui_theme::mcp_agent_card()),
                    )
                },
            );
            let navigation = container(
                column![
                    text("SETTINGS").size(11),
                    button(
                        row![
                            svg(svg::Handle::from_memory(MESSAGE_SQUARE_ICON))
                                .width(Length::Fixed(16.0))
                                .height(Length::Fixed(16.0))
                                .style(|_theme: &Theme, _status| ui_theme::tool_icon(true)),
                            text("Agents").size(13),
                        ]
                        .spacing(9)
                        .align_y(iced::Alignment::Center),
                    )
                    .width(Fill)
                    .padding([8, 10])
                    .style(|_theme: &Theme, status| ui_theme::session_button(true, status)),
                ]
                .spacing(14),
            )
            .width(Length::Fixed(210.0))
            .height(Fill)
            .padding(16)
            .style(|_theme: &Theme| ui_theme::rail());
            let page = container(
                column![
                    row![
                        column![
                            text("Agents").size(26),
                            text("Configure the command-line agents Agency can use.").size(13),
                        ]
                        .spacing(4)
                        .width(Fill),
                        button("Refresh")
                            .padding([6, 12])
                            .style(|_theme: &Theme, status| {
                                ui_theme::dialog_button(false, status)
                            })
                            .on_press(AppEvent::RefreshAgents),
                    ]
                    .align_y(iced::Alignment::Center),
                    rule::horizontal(1),
                    scrollable(cards).height(Fill),
                ]
                .spacing(18),
            )
            .width(Fill)
            .height(Fill)
            .padding(24)
            .style(|_theme: &Theme| ui_theme::rail());
            row![navigation, rule::vertical(1), page]
                .height(Fill)
                .into()
        };
        let content = if self.layout.settings_open {
            settings_content
        } else {
            agent_content
        };

        let terminal_view: Element<'_, AppEvent> = if self.layout.terminal_visible {
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
            Element<'_, AppEvent>,
            Element<'_, AppEvent>,
            Element<'_, AppEvent>,
        ) = if let Some(agent) = self.active_agent() {
            let state = &agent.diff_state;
            let viewer: Element<'_, AppEvent> =
                if state.viewer_visible && !self.layout.terminal_visible {
                    let artifact = state.artifacts.get(state.selected);
                    let title = artifact
                        .map(|artifact| artifact.title.clone())
                        .unwrap_or_else(|| "DIFF".to_owned());
                    let description = artifact
                        .map(|artifact| artifact.description.clone())
                        .unwrap_or_else(|| "No diff selected".to_owned());
                    let lines = artifact
                        .map(|artifact| {
                            rich_diff(&artifact.diff, (state.viewer_scroll / 20) as usize)
                        })
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
                    container(text(format!(
                        "DIFF {}  ·  j/k scroll  ·  v visual  ·  Enter jump  ·  Ctrl+C close",
                        self.keybindings.display_label()
                    )).size(10))
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
                        .on_press(AppEvent::SelectDiff(index)),
                    )
                },
            );
            let activity: Element<'_, AppEvent> =
                if state.activity_visible && !self.layout.terminal_visible {
                    container(
                        column![
                            diff_sidebar_header(),
                            rule::horizontal(1),
                            scrollable(artifacts).height(Fill),
                            text("j/k select  ·  Enter open  ·  <leader>f").size(10),
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
            let selected = state.activity_visible && !self.layout.terminal_visible;
            let focused = selected && focused_element == FOCUS_DIFF_ACTIVITY;
            let control = button(
                svg(svg::Handle::from_memory(FILE_ICON))
                    .width(Length::Fixed(19.0))
                    .height(Length::Fixed(19.0))
                    .style(move |_theme: &Theme, _status| ui_theme::tool_icon(selected)),
            )
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(36.0))
            .padding(8)
            .style(move |_theme: &Theme, status| ui_theme::tool_button(selected, focused, status))
            .on_press(AppEvent::ToggleDiffActivity);
            let count_badge = container(diff_count_badge(state.artifacts.len()))
                .padding([1, 2])
                .align_right(Fill)
                .align_top(Fill);
            let control: Element<'_, AppEvent> = if leader_pending {
                stack![
                    control,
                    count_badge,
                    container(shortcut_badge(
                        self.keybindings.toggle_diffs_hint().to_owned()
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
        let file_viewer_visible = self.file_viewer.visible && !self.layout.terminal_visible;
        let file_view: Element<'_, AppEvent> = if file_viewer_visible {
            let rendered =
                self.file_viewer
                    .blocks
                    .iter()
                    .fold(column![].spacing(14), |content, block| match block {
                        file_viewer::Block::Markdown(markdown_content) => content.push(
                            markdown::view(markdown_content.items(), ui_theme::markdown_settings())
                                .map(AppEvent::LinkClicked),
                        ),
                        file_viewer::Block::Mermaid(diagram) => {
                            let diagram_lines = diagram.lines.iter().fold(
                                column![text(format!("MERMAID · {}", diagram.kind)).size(10)]
                                    .spacing(8),
                                |lines, line| lines.push(text(line).size(13)),
                            );
                            content.push(
                                container(diagram_lines)
                                    .width(Fill)
                                    .padding(14)
                                    .style(|_theme: &Theme| ui_theme::mcp_agent_card()),
                            )
                        }
                    });
            let body: Element<'_, AppEvent> =
                if self.file_viewer.mode == file_viewer::Mode::Rendered {
                    rendered.into()
                } else {
                    rich_file(&self.file_viewer.source, self.file_viewer.path.as_deref())
                };
            let title = self.file_viewer.path.as_ref().map_or_else(
                || "FILE VIEWER".to_owned(),
                |path| {
                    path.strip_prefix(&self.cwd)
                        .unwrap_or(path)
                        .display()
                        .to_string()
                },
            );
            container(
                column![
                    row![
                        text(title).size(11),
                        iced::widget::Space::new().width(Fill),
                        button(text("×").size(16))
                            .padding([2, 7])
                            .style(|_theme: &Theme, status| ui_theme::icon_button(status))
                            .on_press(AppEvent::ToggleFileViewer),
                    ]
                    .align_y(iced::Alignment::Center),
                    if self.file_viewer.is_markdown() {
                        row![
                            button(text("Rendered").size(11))
                                .padding([5, 9])
                                .style(move |_theme: &Theme, status| ui_theme::tool_button(
                                    self.file_viewer.mode == file_viewer::Mode::Rendered,
                                    false,
                                    status,
                                ))
                                .on_press(AppEvent::SetFileViewerMode(file_viewer::Mode::Rendered)),
                            button(text("Raw").size(11))
                                .padding([5, 9])
                                .style(move |_theme: &Theme, status| ui_theme::tool_button(
                                    self.file_viewer.mode == file_viewer::Mode::Raw,
                                    false,
                                    status,
                                ))
                                .on_press(AppEvent::SetFileViewerMode(file_viewer::Mode::Raw)),
                            iced::widget::Space::new().width(Fill),
                            text("Tab switches view").size(10),
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center)
                    } else {
                        row![text("READ ONLY").size(10)]
                    },
                    rule::horizontal(1),
                    scrollable(body).width(Fill).height(Fill).direction(
                        if self.file_viewer.wraps_content() {
                            iced::widget::scrollable::Direction::Vertical(
                                iced::widget::scrollable::Scrollbar::default(),
                            )
                        } else {
                            iced::widget::scrollable::Direction::Both {
                                vertical: iced::widget::scrollable::Scrollbar::default(),
                                horizontal: iced::widget::scrollable::Scrollbar::default(),
                            }
                        }
                    ),
                ]
                .spacing(10),
            )
            .width(Fill)
            .height(Fill)
            .padding(14)
            .style(|_theme: &Theme| ui_theme::rail())
            .into()
        } else {
            iced::widget::Space::new().width(Length::Shrink).into()
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
            && !self.layout.terminal_visible;
        let diff_activity_visible = self
            .active_agent()
            .is_some_and(|agent| agent.diff_state.activity_visible)
            && !self.layout.terminal_visible;
        let diff_viewer_pane = right_pane(
            diff_viewer,
            diff_viewer_visible,
            focused_element == FOCUS_DIFF_VIEWER,
            Fill,
        );
        let diff_activity_pane = right_pane(
            diff_activity,
            diff_activity_visible,
            focused_element == FOCUS_DIFF_ACTIVITY,
            Length::Fixed(261.0),
        );
        let file_viewer_pane = right_pane(file_view, file_viewer_visible, false, Fill);
        let right_activity_pane = right_pane(
            session_activity_bar.into(),
            true,
            false,
            Length::Fixed(49.0),
        );

        let indicator = self.keybindings.mode_indicator();
        let indicator_color = match indicator {
            ModeIndicator::Normal => self.mode_colors.normal,
            ModeIndicator::Insert => self.mode_colors.agent,
            ModeIndicator::Visual => self.mode_colors.visual,
            ModeIndicator::Terminal => self.mode_colors.terminal,
            ModeIndicator::Leader => self.mode_colors.leader,
        };
        let indicator_text_color = contrasting_text(indicator_color);
        let mode_indicator = container(text(self.keybindings.display_label()).size(12))
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
        let agent_menu_open = self.agent_menu.open;
        let agent_indicator: Element<'_, AppEvent> =
            button(text(format!("AGENT {}", self.selected_agent.label())).size(12))
                .padding([3, 8])
                .on_press(AppEvent::ToggleAgentMenu)
                .style(move |_theme: &Theme, status| ui_theme::agent_chip(agent_menu_open, status))
                .into();
        let agent_indicator: Element<'_, AppEvent> = if leader_pending {
            stack![
                agent_indicator,
                container(shortcut_badge(
                    self.keybindings.toggle_agent_menu_hint().to_owned()
                ))
                .align_right(Fill)
                .align_top(Fill),
            ]
            .into()
        } else {
            agent_indicator
        };

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

        let toolbar = toolbar_surface(
            toolbar,
            matches!(focused_element, FOCUS_TOOLBAR | FOCUS_EXPLORER),
        );
        let content = if self.layout.terminal_visible {
            focused_surface(terminal_view, focused_element == FOCUS_TERMINAL)
        } else {
            focused_surface(
                content,
                matches!(focused_element, FOCUS_WORKSPACE | FOCUS_COMPOSER),
            )
        };
        let application: Element<'_, AppEvent> = column![
            tab_bar,
            rule::horizontal(1),
            row![
                toolbar,
                rule::vertical(1),
                content,
                file_viewer_pane,
                diff_viewer_pane,
                diff_activity_pane,
                right_activity_pane
            ]
            .width(Fill)
            .height(Fill),
            rule::horizontal(1),
            container(status)
                .width(Fill)
                .padding([0, 12])
                .center_y(Length::Fixed(STATUS_BAR_HEIGHT))
                .style(|_theme: &Theme| ui_theme::status_bar()),
        ]
        .into();

        let application: Element<'_, AppEvent> = if self.agent_menu.open {
            stack![application, self.agent_menu_layer()].into()
        } else {
            application
        };

        let Some(index) = self.overlays.pending_session_trash else {
            return application;
        };
        let Some(session) = self.sessions().records().get(index) else {
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
                        .on_press(AppEvent::CancelSessionTrash),
                    button("Trash session")
                        .style(|_theme: &Theme, status| { ui_theme::dialog_button(true, status) })
                        .on_press(AppEvent::ConfirmSessionTrash),
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

    /// The agent switcher floats directly above the status bar's agent chip.
    /// The chip's offset depends on the mode indicator beside it, so the layer
    /// reserves an invisible stand-in of the same shape instead of guessing a
    /// column that every mode label would shift.
    fn agent_menu_layer(&self) -> Element<'_, AppEvent> {
        let entries: Element<'_, AppEvent> = if self.configured_agents.is_empty() {
            column![
                text("No agents found on PATH").size(13),
                text("Install Codex or Claude Code, then reopen this menu.").size(11),
            ]
            .spacing(6)
            .into()
        } else {
            self.configured_agents
                .iter()
                .enumerate()
                .fold(column![].spacing(4), |entries, (index, provider)| {
                    let provider = *provider;
                    let selected = index == self.agent_menu.selected;
                    let marker: Element<'_, AppEvent> = if provider == self.selected_agent {
                        container(text("CURRENT").size(10))
                            .padding([2, 6])
                            .style(|_theme: &Theme| ui_theme::agent_badge())
                            .into()
                    } else if provider == self.default_agent {
                        container(text("DEFAULT").size(10))
                            .padding([2, 6])
                            .style(move |_theme: &Theme| {
                                ui_theme::agent_type_badge(provider == Provider::Codex)
                            })
                            .into()
                    } else {
                        iced::widget::Space::new().into()
                    };
                    entries.push(
                        button(
                            row![
                                text(provider.label()).size(13),
                                iced::widget::Space::new().width(Fill),
                                marker,
                            ]
                            .spacing(10)
                            .align_y(iced::Alignment::Center),
                        )
                        .width(Fill)
                        .padding([5, 9])
                        .on_press(AppEvent::SelectAgent(provider))
                        .style(move |_theme: &Theme, status| {
                            ui_theme::menu_entry(selected, status)
                        }),
                    )
                })
                .into()
        };

        let menu = container(
            column![
                text("Switch agent").size(11),
                entries,
                text("j/k move  ·  Enter switch  ·  Esc close").size(10),
            ]
            .spacing(9),
        )
        .width(Length::Fixed(268.0))
        .padding(12)
        .style(|_theme: &Theme| ui_theme::floating_menu());

        let chip_anchor = container(
            text(self.keybindings.display_label())
                .size(12)
                .color(Color::TRANSPARENT),
        )
        .padding([3, 8]);

        container(row![chip_anchor, menu].spacing(24))
            .width(Fill)
            .align_bottom(Fill)
            .padding(
                Padding::default()
                    .left(12.0)
                    .bottom(STATUS_BAR_HEIGHT + AGENT_MENU_GAP),
            )
            .into()
    }
}

fn contrasting_text(background: Color) -> Color {
    let background_luminance = relative_luminance(background);
    let dark_contrast = contrast_ratio(
        background_luminance,
        relative_luminance(ui_theme::DARK_TEXT),
    );
    let light_contrast = contrast_ratio(background_luminance, relative_luminance(ui_theme::TEXT));
    if dark_contrast >= light_contrast {
        ui_theme::DARK_TEXT
    } else {
        ui_theme::TEXT
    }
}

fn relative_luminance(color: Color) -> f32 {
    fn linearize(channel: f32) -> f32 {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * linearize(color.r) + 0.7152 * linearize(color.g) + 0.0722 * linearize(color.b)
}

fn contrast_ratio(first_luminance: f32, second_luminance: f32) -> f32 {
    let lighter = first_luminance.max(second_luminance);
    let darker = first_luminance.min(second_luminance);
    (lighter + 0.05) / (darker + 0.05)
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

fn activity_focus_target(visible: bool, activity: SidebarTool) -> Option<FocusId> {
    visible.then_some(match activity {
        SidebarTool::Explorer => FOCUS_EXPLORER,
        SidebarTool::Sessions | SidebarTool::Mcp => FOCUS_TOOLBAR,
    })
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
    use super::{
        FOCUS_EXPLORER, FOCUS_TOOLBAR, SidebarTool, activity_focus_target, toggled_activity,
    };

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

    #[test]
    fn newly_shown_activity_receives_focus() {
        assert_eq!(
            activity_focus_target(true, SidebarTool::Explorer),
            Some(FOCUS_EXPLORER)
        );
        assert_eq!(
            activity_focus_target(true, SidebarTool::Sessions),
            Some(FOCUS_TOOLBAR)
        );
        assert_eq!(
            activity_focus_target(true, SidebarTool::Mcp),
            Some(FOCUS_TOOLBAR)
        );
    }

    #[test]
    fn hidden_activity_does_not_receive_focus() {
        assert_eq!(activity_focus_target(false, SidebarTool::Explorer), None);
    }
}

#[cfg(test)]
mod agent_menu_tests {
    use super::{AgentMenuContext, AgentMenuMotion, AgentMenuState, AppEvent, FOCUS_COMPOSER};
    use agency_agents::Provider;

    const AGENTS: [Provider; 2] = [Provider::Codex, Provider::Claude];

    fn context(agents: &[Provider], selected: Provider) -> AgentMenuContext<'_> {
        AgentMenuContext {
            agents,
            selected_agent: selected,
            focused: FOCUS_COMPOSER,
        }
    }

    fn reduce(menu: &mut AgentMenuState, events: &[AppEvent], selected: Provider) {
        for event in events {
            menu.on_event(event, context(&AGENTS, selected));
        }
    }

    /// The menu opens on the agent that is already selected so `Enter` alone is
    /// never a surprise switch.
    #[test]
    fn opening_the_menu_highlights_the_selected_agent() {
        let mut menu = AgentMenuState::default();
        reduce(&mut menu, &[AppEvent::ToggleAgentMenu], Provider::Claude);

        assert!(menu.open);
        assert_eq!(menu.highlighted(&AGENTS), Some(Provider::Claude));
        assert_eq!(menu.return_focus, Some(FOCUS_COMPOSER));
    }

    #[test]
    fn the_same_binding_closes_an_open_menu() {
        let mut menu = AgentMenuState::default();
        reduce(
            &mut menu,
            &[AppEvent::ToggleAgentMenu, AppEvent::ToggleAgentMenu],
            Provider::Codex,
        );

        assert!(!menu.open);
    }

    #[test]
    fn motions_wrap_around_the_configured_agents() {
        let mut menu = AgentMenuState::default();
        reduce(&mut menu, &[AppEvent::ToggleAgentMenu], Provider::Codex);
        assert_eq!(menu.highlighted(&AGENTS), Some(Provider::Codex));

        reduce(
            &mut menu,
            &[AppEvent::MoveAgentMenu(AgentMenuMotion::Next)],
            Provider::Codex,
        );
        assert_eq!(menu.highlighted(&AGENTS), Some(Provider::Claude));

        reduce(
            &mut menu,
            &[AppEvent::MoveAgentMenu(AgentMenuMotion::Next)],
            Provider::Codex,
        );
        assert_eq!(menu.highlighted(&AGENTS), Some(Provider::Codex));

        reduce(
            &mut menu,
            &[AppEvent::MoveAgentMenu(AgentMenuMotion::Previous)],
            Provider::Codex,
        );
        assert_eq!(menu.highlighted(&AGENTS), Some(Provider::Claude));

        reduce(
            &mut menu,
            &[AppEvent::MoveAgentMenu(AgentMenuMotion::First)],
            Provider::Codex,
        );
        assert_eq!(menu.highlighted(&AGENTS), Some(Provider::Codex));

        reduce(
            &mut menu,
            &[AppEvent::MoveAgentMenu(AgentMenuMotion::Last)],
            Provider::Codex,
        );
        assert_eq!(menu.highlighted(&AGENTS), Some(Provider::Claude));
    }

    #[test]
    fn choosing_or_dismissing_an_agent_closes_the_menu() {
        for resolution in [
            AppEvent::SelectAgent(Provider::Claude),
            AppEvent::CloseAgentMenu,
            AppEvent::StartAgent(Provider::Codex),
            AppEvent::ToggleSettings,
        ] {
            let mut menu = AgentMenuState::default();
            reduce(&mut menu, &[AppEvent::ToggleAgentMenu], Provider::Codex);
            assert!(menu.open);

            reduce(&mut menu, &[resolution], Provider::Codex);
            assert!(!menu.open);
        }
    }

    /// Without a configured agent there is nothing to highlight, so motions must
    /// stay in range rather than indexing an empty list.
    #[test]
    fn motions_are_inert_without_configured_agents() {
        let mut menu = AgentMenuState::default();
        let empty: [Provider; 0] = [];
        menu.on_event(&AppEvent::ToggleAgentMenu, context(&empty, Provider::Codex));
        menu.on_event(
            &AppEvent::MoveAgentMenu(AgentMenuMotion::Next),
            context(&empty, Provider::Codex),
        );

        assert!(menu.open);
        assert_eq!(menu.highlighted(&empty), None);
    }
}

#[cfg(test)]
mod focus_mode_tests {
    use super::{
        FOCUS_AGENT_MENU, FOCUS_COMPOSER, FOCUS_CONFIRMATION, FOCUS_DIFF_ACTIVITY,
        FOCUS_DIFF_VIEWER, FOCUS_EXPLORER, FOCUS_SLASH_COMPLETION, FOCUS_TERMINAL, FOCUS_TOOLBAR,
        FOCUS_WORKSPACE, FocusVisibility, InteractionState, KeybindingContext, Mode,
        ui_element_modes,
    };

    const EVERY_MODE: [Mode; 4] = [Mode::Normal, Mode::Insert, Mode::Visual, Mode::Terminal];

    fn workspace_with_agent() -> FocusVisibility {
        FocusVisibility {
            toolbar: true,
            composer: true,
            ..FocusVisibility::default()
        }
    }

    /// Regression: `<leader> n` used to enter INSERT while the sessions toolbar
    /// still owned focus. INSERT outside the composer resolves to no action, so
    /// every binding, including Escape, stopped responding after starting a
    /// session.
    #[test]
    fn insert_mode_outside_the_composer_falls_back_to_normal() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(workspace_with_agent());
        assert!(interaction.focus.focus(FOCUS_TOOLBAR));

        assert_eq!(interaction.reconciled_mode(Mode::Insert), Mode::Normal);
        assert_eq!(interaction.reconciled_mode(Mode::Visual), Mode::Normal);
        assert_eq!(interaction.reconciled_mode(Mode::Terminal), Mode::Normal);
        assert_eq!(interaction.reconciled_mode(Mode::Normal), Mode::Normal);
    }

    #[test]
    fn a_focused_composer_keeps_its_insert_like_modes() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(workspace_with_agent());
        assert!(interaction.focus.focus(FOCUS_COMPOSER));

        assert_eq!(interaction.reconciled_mode(Mode::Insert), Mode::Insert);
        assert_eq!(interaction.reconciled_mode(Mode::Visual), Mode::Visual);
        assert_eq!(interaction.reconciled_mode(Mode::Terminal), Mode::Normal);
    }

    /// The composer cannot be focused before a session exists, so entering it
    /// must stay a no-op rather than leaving the application in INSERT with
    /// another element focused.
    #[test]
    fn entering_the_composer_requires_a_visible_composer() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(FocusVisibility {
            toolbar: true,
            workspace: true,
            ..FocusVisibility::default()
        });
        assert!(!interaction.focus.focus(FOCUS_COMPOSER));

        interaction.sync_visibility(workspace_with_agent());
        assert!(interaction.focus.focus(FOCUS_COMPOSER));
    }

    /// The slash completion list borrows focus from the composer, so typing has
    /// to keep resolving against the composer keymap in INSERT.
    #[test]
    fn the_slash_completion_overlay_keeps_composer_bindings() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(FocusVisibility {
            slash_completion: true,
            ..workspace_with_agent()
        });

        assert_eq!(interaction.focus.focused(), FOCUS_SLASH_COMPLETION);
        assert_eq!(interaction.focus.context(), KeybindingContext::Composer);
        assert_eq!(interaction.reconciled_mode(Mode::Insert), Mode::Insert);
    }

    /// Regression: closing the completion list used to drop focus on whichever
    /// element sorted first — the sessions toolbar — which forced NORMAL and
    /// stranded the user outside the composer mid-command.
    #[test]
    fn closing_the_slash_completion_overlay_returns_focus_to_the_composer() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(workspace_with_agent());
        assert!(interaction.focus.focus(FOCUS_COMPOSER));

        interaction.sync_visibility(FocusVisibility {
            slash_completion: true,
            ..workspace_with_agent()
        });
        assert_eq!(interaction.focus.focused(), FOCUS_SLASH_COMPLETION);

        interaction.sync_visibility(workspace_with_agent());

        assert_eq!(interaction.focus.focused(), FOCUS_COMPOSER);
        assert_eq!(interaction.reconciled_mode(Mode::Insert), Mode::Insert);
    }

    /// Overlays stack, so the surface underneath all of them is what focus
    /// returns to once the last one closes.
    #[test]
    fn stacked_overlays_return_focus_to_the_surface_they_all_borrowed_from() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(workspace_with_agent());
        assert!(interaction.focus.focus(FOCUS_COMPOSER));

        interaction.sync_visibility(FocusVisibility {
            slash_completion: true,
            ..workspace_with_agent()
        });
        interaction.sync_visibility(FocusVisibility {
            slash_completion: true,
            agent_menu: true,
            ..workspace_with_agent()
        });
        assert_eq!(interaction.focus.focused(), FOCUS_AGENT_MENU);

        interaction.sync_visibility(FocusVisibility {
            slash_completion: true,
            ..workspace_with_agent()
        });
        assert_eq!(interaction.focus.focused(), FOCUS_SLASH_COMPLETION);

        interaction.sync_visibility(workspace_with_agent());
        assert_eq!(interaction.focus.focused(), FOCUS_COMPOSER);
    }

    /// A borrowed surface that disappeared while the overlay was open cannot
    /// take focus back, so the cycle picks the next visible element instead.
    #[test]
    fn a_vanished_borrower_falls_back_to_the_focus_cycle() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(workspace_with_agent());
        assert!(interaction.focus.focus(FOCUS_COMPOSER));

        interaction.sync_visibility(FocusVisibility {
            slash_completion: true,
            ..workspace_with_agent()
        });
        interaction.sync_visibility(FocusVisibility {
            toolbar: true,
            ..FocusVisibility::default()
        });

        assert_eq!(interaction.focus.focused(), FOCUS_TOOLBAR);
    }

    /// The confirmation modal owns input and is driven from NORMAL.
    #[test]
    fn the_confirmation_modal_returns_to_normal() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(FocusVisibility {
            confirmation: true,
            ..workspace_with_agent()
        });

        assert_eq!(interaction.focus.focused(), FOCUS_CONFIRMATION);
        assert_eq!(interaction.reconciled_mode(Mode::Insert), Mode::Normal);
    }

    /// The agent menu floats over the status bar and owns input, so it takes
    /// focus from whatever surface it was opened over and binds NORMAL there.
    #[test]
    fn the_agent_menu_borrows_focus_while_it_floats() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(FocusVisibility {
            agent_menu: true,
            ..workspace_with_agent()
        });

        assert_eq!(interaction.focus.focused(), FOCUS_AGENT_MENU);
        assert_eq!(interaction.focus.context(), KeybindingContext::AgentMenu);
        assert_eq!(interaction.reconciled_mode(Mode::Insert), Mode::Normal);

        interaction.sync_visibility(workspace_with_agent());
        assert_ne!(interaction.focus.focused(), FOCUS_AGENT_MENU);
    }

    #[test]
    fn the_terminal_only_binds_terminal_mode() {
        let mut interaction = InteractionState::default();
        interaction.sync_visibility(FocusVisibility {
            terminal: true,
            ..FocusVisibility::default()
        });
        assert!(interaction.focus.focus(FOCUS_TERMINAL));

        assert_eq!(interaction.reconciled_mode(Mode::Terminal), Mode::Terminal);
        assert_eq!(interaction.reconciled_mode(Mode::Insert), Mode::Normal);
    }

    /// An element missing from the registry silently loses every binding in the
    /// modes it should support, so each focusable element must declare its
    /// modes.
    #[test]
    fn every_focusable_element_declares_its_modes() {
        let modes = ui_element_modes();
        for element in [
            FOCUS_TOOLBAR,
            FOCUS_EXPLORER,
            FOCUS_WORKSPACE,
            FOCUS_COMPOSER,
            FOCUS_TERMINAL,
            FOCUS_DIFF_VIEWER,
            FOCUS_DIFF_ACTIVITY,
            FOCUS_SLASH_COMPLETION,
            FOCUS_CONFIRMATION,
            FOCUS_AGENT_MENU,
        ] {
            assert!(
                EVERY_MODE.iter().any(|mode| modes.supports(element, *mode)),
                "focusable element {element:?} declares no modes"
            );
        }
    }
}

fn sidebar_header(label: &'static str, icon: &'static [u8]) -> Element<'static, AppEvent> {
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
        .on_press(AppEvent::ToggleToolbar),
    ]
    .align_y(iced::Alignment::Center)
    .spacing(8)
    .into()
}

fn diff_sidebar_header() -> Element<'static, AppEvent> {
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
        .on_press(AppEvent::ToggleDiffActivity),
    ]
    .align_y(iced::Alignment::Center)
    .spacing(8)
    .into()
}

fn rich_diff(diff: &str, skipped: usize) -> Element<'static, AppEvent> {
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

fn rich_file(source: &str, path: Option<&std::path::Path>) -> Element<'static, AppEvent> {
    let lines = source.lines().map(str::to_owned).collect::<Vec<_>>();
    let syntax = path
        .and_then(std::path::Path::extension)
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("txt");
    let mut highlighter = iced::highlighter::Stream::new(&iced::highlighter::Settings {
        theme: iced::highlighter::Theme::Base16Ocean,
        token: syntax.to_owned(),
    });
    let width = (lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or_default() as f32
        * 7.5
        + 70.0)
        .max(600.0);

    lines
        .into_iter()
        .enumerate()
        .fold(column![].spacing(0), |lines, (index, line)| {
            let highlights = highlighter
                .highlight_line(&line)
                .map(|(range, highlight)| {
                    (
                        range,
                        ui_theme::syntax_color(highlight.color()),
                        highlight.font(),
                    )
                })
                .collect::<Vec<_>>();
            highlighter.commit();
            let spans = highlights
                .into_iter()
                .map(|(range, color, font)| {
                    let mut highlighted = span(line[range].to_owned()).color(color);
                    if let Some(font) = font {
                        highlighted = highlighted.font(font);
                    }
                    highlighted
                })
                .collect::<Vec<iced::widget::text::Span<'static, ()>>>();
            lines.push(
                row![
                    container(text((index + 1).to_string()).font(Font::MONOSPACE).size(11))
                        .width(Length::Fixed(48.0))
                        .padding([3, 6])
                        .align_x(iced::alignment::Horizontal::Right)
                        .style(|_theme: &Theme| ui_theme::diff_gutter()),
                    container(rich_text(spans).font(Font::MONOSPACE).size(12))
                        .padding([3, 8])
                        .width(Fill),
                ]
                .width(Length::Fixed(width)),
            )
        })
        .width(Length::Fixed(width))
        .into()
}

fn file_changes_card<'a>(
    status: &'a str,
    changes: &'a [diffs::FileChange],
) -> Element<'a, AppEvent> {
    let count = changes.len();
    let header = row![
        svg(svg::Handle::from_memory(FILE_ICON))
            .width(Length::Fixed(16.0))
            .height(Length::Fixed(16.0))
            .style(|_theme: &Theme, _status| ui_theme::icon()),
        text(if count == 1 {
            "File edit"
        } else {
            "File edits"
        })
        .size(12)
        .font(Font::MONOSPACE),
        iced::widget::Space::new().width(Fill),
        container(text(count.to_string()).size(10).font(Font::MONOSPACE))
            .padding([2, 6])
            .style(|_theme: &Theme| ui_theme::file_change_count()),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    let body = if changes.is_empty() {
        column![
            container(text("No file changes were applied").size(12))
                .padding([10, 0])
                .width(Fill)
        ]
    } else {
        changes.iter().fold(column![].spacing(6), |rows, change| {
            let path = std::path::Path::new(&change.path);
            let filename = path
                .file_name()
                .map_or_else(|| change.path.as_str(), |name| name.to_str().unwrap_or(""));
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.display().to_string());
            let action = change
                .description
                .split_ascii_whitespace()
                .next()
                .unwrap_or("Updated");
            let details = change
                .description
                .strip_prefix(action)
                .unwrap_or(&change.description)
                .trim()
                .trim_start_matches('·')
                .trim();
            let path_details: Element<'_, AppEvent> = if let Some(parent) = parent {
                column![
                    text(filename).size(13).width(Fill),
                    text(parent).size(10).font(Font::MONOSPACE).width(Fill),
                ]
                .spacing(2)
                .width(Fill)
                .into()
            } else {
                text(filename).size(13).width(Fill).into()
            };
            let summary: Element<'_, AppEvent> = if details.is_empty() {
                iced::widget::Space::new().height(Length::Shrink).into()
            } else {
                text(details).size(10).font(Font::MONOSPACE).into()
            };
            rows.push(
                container(
                    column![
                        row![
                            svg(svg::Handle::from_memory(FILE_ICON))
                                .width(Length::Fixed(15.0))
                                .height(Length::Fixed(15.0))
                                .style(|_theme: &Theme, _status| {
                                    ui_theme::tree_item_icon(false)
                                }),
                            path_details,
                            container(text(action).size(10)).padding([2, 6]).style(
                                move |_theme: &Theme| { ui_theme::file_change_badge(action) }
                            ),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                        summary,
                    ]
                    .spacing(6),
                )
                .width(Fill)
                .padding([8, 10])
                .style(|_theme: &Theme| ui_theme::file_change_row()),
            )
        })
    };

    container(column![header, rule::horizontal(1), body].spacing(9))
        .width(Fill)
        .padding([10, 12])
        .style(move |_theme: &Theme| ui_theme::status_card(status))
        .into()
}

/// The shape every reported activity shares: an icon, what it was, how it
/// ended, and the detail of the work itself.
fn activity_card<'a>(
    icon: &'static [u8],
    title: String,
    status: &'a str,
    body: Element<'a, AppEvent>,
) -> Element<'a, AppEvent> {
    let badge = status.to_owned();
    container(
        container(
            column![
                row![
                    svg(svg::Handle::from_memory(icon))
                        .width(Length::Fixed(16.0))
                        .height(Length::Fixed(16.0))
                        .style(|_theme: &Theme, _status| ui_theme::icon()),
                    text(title).size(12).font(Font::MONOSPACE),
                    iced::widget::Space::new().width(Fill),
                    container(text(status_label(status)).size(10).font(Font::MONOSPACE))
                        .padding([2, 6])
                        .style(move |_theme: &Theme| ui_theme::status_badge(&badge)),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                rule::horizontal(1),
                body,
            ]
            .spacing(9),
        )
        .width(Fill)
        .padding([10, 12])
        .style(move |_theme: &Theme| ui_theme::status_card(status)),
    )
    .width(Fill)
    .padding([4, 32])
    .into()
}

fn status_label(status: &str) -> String {
    match status {
        "inProgress" => "Running".to_owned(),
        status => {
            let mut characters = status.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        }
    }
}

fn monospace_block(source: &str, terminal: bool) -> Element<'_, AppEvent> {
    container(
        text(source)
            .size(12)
            .font(Font::MONOSPACE)
            .width(Fill)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
    )
    .width(Fill)
    .padding([8, 10])
    .style(move |_theme: &Theme| {
        if terminal {
            ui_theme::terminal_output()
        } else {
            ui_theme::file_change_row()
        }
    })
    .into()
}

fn command_execution_card<'a>(
    content: &'a markdown::Content,
    output: &'a str,
    status: &'a str,
    exit_code: Option<i64>,
) -> Element<'a, AppEvent> {
    let mut body = column![
        markdown::view(content.items(), ui_theme::markdown_settings()).map(AppEvent::LinkClicked)
    ]
    .spacing(6);
    if !output.trim().is_empty() {
        body = body.push(monospace_block(output.trim_end(), true));
    }
    if let Some(code) = exit_code.filter(|code| *code != 0) {
        body = body.push(
            text(format!("Exited with status {code}"))
                .size(11)
                .color(ui_theme::DANGER),
        );
    }
    activity_card(
        TERMINAL_ICON,
        "Command".to_owned(),
        status,
        body.width(Fill).into(),
    )
}

fn file_read_card<'a>(path: &'a str, status: &'a str, lines: Option<u64>) -> Element<'a, AppEvent> {
    let mut details = row![
        svg(svg::Handle::from_memory(FILE_ICON))
            .width(Length::Fixed(15.0))
            .height(Length::Fixed(15.0))
            .style(|_theme: &Theme, _status| ui_theme::tree_item_icon(false)),
        text(path).size(13).width(Fill),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);
    if let Some(lines) = lines {
        details = details.push(
            text(format!("{lines} line{}", if lines == 1 { "" } else { "s" }))
                .size(10)
                .font(Font::MONOSPACE),
        );
    }
    activity_card(
        FILE_ICON,
        "Read file".to_owned(),
        status,
        container(details)
            .width(Fill)
            .padding([8, 10])
            .style(|_theme: &Theme| ui_theme::file_change_row())
            .into(),
    )
}

fn plugin_install_card(install: &PluginInstallEntry) -> Element<'_, AppEvent> {
    let mut body = column![monospace_block(&install.command, false)].spacing(6);
    if !install.output.is_empty() {
        body = body.push(monospace_block(&install.output, true));
    }
    if let Some(detail) = &install.detail {
        body = body.push(text(detail).size(12).color(ui_theme::DANGER).width(Fill));
    }
    activity_card(
        TERMINAL_ICON,
        format!("Plugin install  •  {}", install.provider.label()),
        install.status.label(),
        body.width(Fill).into(),
    )
}

fn web_search_card(queries: &[String]) -> Element<'_, AppEvent> {
    let query_count = queries.len();
    let rows = queries.iter().fold(column![].spacing(6), |rows, query| {
        rows.push(
            container(
                row![
                    text("↗").size(14).color(ui_theme::PRIMARY),
                    text(query).size(12).width(Fill),
                ]
                .spacing(9)
                .align_y(iced::Alignment::Start),
            )
            .width(Fill)
            .padding([8, 10])
            .style(|_theme: &Theme| ui_theme::file_change_row()),
        )
    });
    container(
        container(
            column![
                row![
                    svg(svg::Handle::from_memory(NETWORK_ICON))
                        .width(Length::Fixed(16.0))
                        .height(Length::Fixed(16.0))
                        .style(|_theme: &Theme, _status| ui_theme::icon()),
                    text("Web search").size(12).font(Font::MONOSPACE),
                    iced::widget::Space::new().width(Fill),
                    text(format!(
                        "{query_count} quer{}",
                        if query_count == 1 { "y" } else { "ies" }
                    ))
                    .size(10),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                rule::horizontal(1),
                rows,
            ]
            .spacing(9),
        )
        .width(Fill)
        .padding([10, 12])
        .style(|_theme: &Theme| ui_theme::mcp_agent_card()),
    )
    .width(Fill)
    .padding([4, 32])
    .into()
}

fn right_pane<'a>(
    content: Element<'a, AppEvent>,
    visible: bool,
    focused: bool,
    width: impl Into<Length>,
) -> Element<'a, AppEvent> {
    if visible {
        row![rule::vertical(1), focused_surface(content, focused)]
            .width(width)
            .height(Fill)
            .into()
    } else {
        iced::widget::Space::new().width(Length::Shrink).into()
    }
}

fn focused_surface<'a>(content: Element<'a, AppEvent>, focused: bool) -> Element<'a, AppEvent> {
    focused_surface_with_width(content, focused, Fill)
}

fn focused_surface_with_width<'a>(
    content: Element<'a, AppEvent>,
    focused: bool,
    width: Length,
) -> Element<'a, AppEvent> {
    container(content)
        .width(width)
        .height(Fill)
        .style(move |_theme: &Theme| ui_theme::focus_surface(focused))
        .into()
}

fn optional_focused_surface<'a>(
    content: Element<'a, AppEvent>,
    visible: bool,
    focused: bool,
    width: Length,
) -> Element<'a, AppEvent> {
    if visible {
        focused_surface_with_width(content, focused, width)
    } else {
        iced::widget::Space::new().width(Length::Shrink).into()
    }
}

fn toolbar_surface<'a>(content: Element<'a, AppEvent>, focused: bool) -> Element<'a, AppEvent> {
    focused_surface_with_width(content, focused, Length::Shrink)
}

fn toolbar_panel_surface<'a>(
    content: Element<'a, AppEvent>,
    visible: bool,
    focused: bool,
) -> Element<'a, AppEvent> {
    optional_focused_surface(content, visible, focused, Length::Shrink)
}

fn shortcut_badge(key: String) -> Element<'static, AppEvent> {
    container(text(key).font(Font::MONOSPACE).size(11))
        .padding([2, 5])
        .style(|_theme: &Theme| ui_theme::shortcut_badge())
        .into()
}

fn agent_status_badge(
    status: ui_theme::AgentStatus,
    animation_elapsed: Duration,
) -> Element<'static, AppEvent> {
    let pulse = (animation_elapsed.as_secs_f32() * std::f32::consts::PI)
        .sin()
        .mul_add(0.5, 0.5);
    let label = match status {
        ui_theme::AgentStatus::Active => "ACTIVE".to_owned(),
        ui_theme::AgentStatus::Thinking => {
            let dots = ".".repeat((animation_elapsed.as_millis() / 350 % 3 + 1) as usize);
            format!("THINKING{dots}")
        }
        ui_theme::AgentStatus::Waiting => "WAITING FOR INPUT".to_owned(),
        ui_theme::AgentStatus::Idle => "IDLE".to_owned(),
        ui_theme::AgentStatus::Resume => "RESUME".to_owned(),
    };
    container(text(label).size(10))
        .width(Length::Fixed(112.0))
        .center_x(Length::Fixed(112.0))
        .padding([2, 5])
        .style(move |_theme: &Theme| ui_theme::agent_status_badge(status, pulse))
        .into()
}

fn diff_count_badge(count: usize) -> Element<'static, AppEvent> {
    container(text(count.to_string()).font(Font::MONOSPACE).size(10))
        .padding([1, 4])
        .style(|_theme: &Theme| ui_theme::counter_badge())
        .into()
}

/// Walks the workspace for the file explorer.
///
/// `root` is threaded through the recursion so the workspace's own
/// `.agency/worktrees` can be skipped: every worktree is a full checkout of
/// this repository, so rendering it here would show the project inside itself
/// and let one file be opened under two paths. Matched by resolved path rather
/// than by name, so a `worktrees` directory that belongs to the project stays
/// visible.
fn collect_explorer_entries(
    root: &std::path::Path,
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
    let worktrees_directory = config::worktrees_directory(root);
    for child in children {
        let path = child.path();
        if path == worktrees_directory {
            continue;
        }
        let directory = child.file_type().is_ok_and(|kind| kind.is_dir());
        entries.push(ExplorerEntry {
            path: path.clone(),
            depth,
            directory,
        });
        if directory && expanded.contains(&path) {
            collect_explorer_entries(root, &path, depth + 1, expanded, entries);
        }
    }
}

/// Whether an accepted agent command has to be sent somewhere other than the
/// agent the composer is pointed at.
///
/// The catalog lists every configured agent's commands in one list, so picking
/// a Claude Code skill while Codex is focused is ordinary. Sending it where it
/// was typed would hand Codex a command it has never heard of, so the submit
/// path routes it to the agent that owns it instead.
fn command_needs_agent_switch(active: Provider, command: Provider) -> bool {
    active != command
}

fn window_geometry(id: window::Id, close_after: bool) -> Task<AppEvent> {
    window::size(id).then(move |size| {
        window::position(id).then(move |position| {
            window::is_maximized(id).then(move |maximized| {
                window::mode(id).map(move |mode| {
                    AppEvent::WindowGeometryReady(id, size, position, maximized, mode, close_after)
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

/// The prompt model treats `'\n'` as the only line break, because every motion
/// helper splits on it. Platforms hand us `"\r\n"` from a paste and `"\r"` from
/// the Enter key, so text is normalized at the one point where it enters the
/// model rather than at each of the places that read it.
fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

fn clamped_prompt_cursor(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

/// Inserts `text` at `cursor`, normalized, and returns the cursor position
/// after it. Split out from `AgentView::insert_prompt_text` because the
/// arithmetic is what breaks — a cursor computed from the pre-normalized
/// length lands short by one per `\r` stripped — and `AgentView` owns a
/// spawned session, so it cannot be built in a test.
fn insert_normalized(prompt: &mut String, cursor: usize, text: &str) -> usize {
    let text = normalize_newlines(text);
    prompt.insert_str(cursor, &text);
    cursor + text.len()
}

/// Normalizes `text` for a wholesale prompt replacement, without paying for a
/// clone when there is nothing to normalize. Every other path that carries
/// external text into the prompt goes through `insert_prompt_text`, which
/// normalizes; the completion, tab-fill, and resolved-submission paths
/// replace the prompt outright instead of inserting into it, so they need
/// this to hold the same invariant against a translator that hands back a
/// `\r`.
fn normalized_prompt(text: String) -> String {
    match normalize_newlines(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(normalized) => normalized,
    }
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(index, _)| cursor + index)
}

fn composer_motion_target(text: &str, cursor: usize, motion: keybindings::ComposerMotion) -> usize {
    use keybindings::ComposerMotion::*;
    match motion {
        Left => previous_char_boundary(text, cursor),
        Right => next_char_boundary(text, cursor),
        Up => {
            let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
            if line_start == 0 {
                0
            } else {
                let previous_end = line_start - 1;
                let previous_start = text[..previous_end]
                    .rfind('\n')
                    .map_or(0, |index| index + 1);
                line_column_target(text, previous_start, previous_end, cursor - line_start)
            }
        }
        Down => {
            let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
            let Some(line_end_offset) = text[cursor..].find('\n') else {
                return text.len();
            };
            let next_start = cursor + line_end_offset + 1;
            let next_end = text[next_start..]
                .find('\n')
                .map_or(text.len(), |offset| next_start + offset);
            line_column_target(text, next_start, next_end, cursor - line_start)
        }
        LineStart => text[..cursor].rfind('\n').map_or(0, |index| index + 1),
        LineEnd => text[cursor..]
            .find('\n')
            .map_or(text.len(), |offset| cursor + offset),
        DocumentStart => 0,
        DocumentEnd => text.len(),
        WordForward => {
            let mut position = cursor;
            let mut saw_word = false;
            for (offset, character) in text[cursor..].char_indices() {
                let word = character.is_alphanumeric() || character == '_';
                if saw_word && !word {
                    position = cursor + offset;
                } else if !saw_word && word {
                    if cursor + offset > cursor {
                        return cursor + offset;
                    }
                    saw_word = true;
                } else if saw_word {
                    position = next_char_boundary(text, cursor + offset);
                }
            }
            position.max(cursor)
        }
        WordBackward => {
            let chars = text[..cursor].char_indices().collect::<Vec<_>>();
            let mut seen_word = false;
            for &(index, character) in chars.iter().rev() {
                let word = character.is_alphanumeric() || character == '_';
                if word {
                    seen_word = true;
                } else if seen_word {
                    return next_char_boundary(text, index);
                }
            }
            0
        }
        WordEnd => {
            let mut seen_word = false;
            for (offset, character) in text[cursor..].char_indices() {
                let word = character.is_alphanumeric() || character == '_';
                if word {
                    seen_word = true;
                } else if seen_word {
                    return previous_char_boundary(text, cursor + offset);
                }
            }
            text.len()
        }
    }
}

fn line_column_target(text: &str, start: usize, end: usize, column: usize) -> usize {
    let mut target = start + column.min(end - start);
    while !text.is_char_boundary(target) {
        target -= 1;
    }
    target
}

fn composer_operation_range(
    text: &str,
    cursor: usize,
    motion: Option<keybindings::ComposerMotion>,
) -> Option<(usize, usize)> {
    if motion.is_none() {
        let start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
        let end = text[cursor..]
            .find('\n')
            .map_or(text.len(), |offset| cursor + offset + 1);
        return (start < end).then_some((start, end));
    }
    let motion = motion?;
    let target = composer_motion_target(text, cursor, motion);
    let (start, mut end) = if target < cursor {
        (target, cursor)
    } else {
        (cursor, target)
    };
    if motion == keybindings::ComposerMotion::WordEnd && end < text.len() {
        end = next_char_boundary(text, end);
    }
    (start < end).then_some((start, end))
}

/// The height of one composer line, matched to the block cursor so a blank
/// line keeps its vertical space instead of collapsing.
const COMPOSER_LINE_HEIGHT: f32 = 17.0;

/// One drawn piece of a composer line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptSpan {
    Cursor,
    Text { range: Range<usize>, selected: bool },
}

/// The composer laid out one row per line.
///
/// Splitting uses `split('\n')` rather than `lines()`, which drops a trailing
/// empty line and would leave a cursor after a final newline with nowhere to
/// draw. Line boundaries stay unambiguous because the `'\n'` occupies a byte:
/// `cursor == line_end` is the end of one line, and `cursor == line_start` is
/// the start of the next, so exactly one line claims the cursor.
fn composer_lines(
    prompt: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
) -> Vec<Vec<PromptSpan>> {
    let mut lines = Vec::new();
    let mut line_start = 0;
    for line in prompt.split('\n') {
        let line_end = line_start + line.len();
        let span = line_start..=line_end;
        let mut boundaries = vec![line_start, line_end];
        if span.contains(&cursor) {
            boundaries.push(cursor);
        }
        if let Some((start, end)) = selection {
            boundaries.extend(
                [start, end]
                    .into_iter()
                    .filter(|bound| span.contains(bound)),
            );
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let mut spans = Vec::new();
        for window in boundaries.windows(2) {
            let (start, end) = (window[0], window[1]);
            if start == cursor {
                spans.push(PromptSpan::Cursor);
            }
            let selected = selection.is_some_and(|(selection_start, selection_end)| {
                start >= selection_start && end <= selection_end
            });
            spans.push(PromptSpan::Text {
                range: start..end,
                selected,
            });
        }
        // A cursor at the very end of a line closes no window, so it is pushed
        // here rather than by the loop above.
        if cursor == line_end {
            spans.push(PromptSpan::Cursor);
        }
        lines.push(spans);
        line_start = line_end + 1;
    }
    lines
}

fn composer_prompt(agent: &AgentView, cursor_visible: bool) -> Element<'_, AppEvent> {
    let cursor = || -> Element<'_, AppEvent> {
        container(text(" ").font(Font::MONOSPACE).size(14))
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(COMPOSER_LINE_HEIGHT))
            .style(move |_theme: &Theme| {
                if cursor_visible {
                    ui_theme::block_cursor()
                } else {
                    container::Style::default()
                }
            })
            .into()
    };

    if agent.prompt.is_empty() {
        return row![
            cursor(),
            text("Type a message and press Enter")
                .font(Font::MONOSPACE)
                .size(14),
        ]
        .spacing(2)
        .into();
    }

    let mut prompt = iced::widget::Column::new();
    for spans in composer_lines(&agent.prompt, agent.prompt_cursor, agent.prompt_selection()) {
        let mut line = iced::widget::Row::new()
            .spacing(0)
            .height(Length::Fixed(COMPOSER_LINE_HEIGHT));
        for span in spans {
            line = line.push(match span {
                PromptSpan::Cursor => cursor(),
                PromptSpan::Text { range, selected } => {
                    container(text(&agent.prompt[range]).font(Font::MONOSPACE).size(14))
                        .style(move |_theme: &Theme| {
                            if selected {
                                ui_theme::text_selection()
                            } else {
                                container::Style::default()
                            }
                        })
                        .into()
                }
            });
        }
        prompt = prompt.push(line);
    }
    prompt.into()
}

fn transcript_is_visible(
    active_conversation_id: Option<&str>,
    terminal_visible: bool,
    settings_open: bool,
    conversation_id: &str,
) -> bool {
    !terminal_visible && !settings_open && active_conversation_id == Some(conversation_id)
}

impl AgentView {
    fn agent_status(&self) -> ui_theme::AgentStatus {
        self.activity.badge()
    }

    fn prompt_selection(&self) -> Option<(usize, usize)> {
        self.prompt_selection_anchor
            .map(|anchor| {
                (
                    anchor.min(self.prompt_cursor),
                    anchor.max(self.prompt_cursor),
                )
            })
            .filter(|(start, end)| start != end)
    }

    fn insert_prompt_text(&mut self, text: &str) {
        self.delete_prompt_selection();
        let cursor = clamped_prompt_cursor(&self.prompt, self.prompt_cursor);
        self.prompt_cursor = insert_normalized(&mut self.prompt, cursor, text);
    }

    fn clear_prompt(&mut self) {
        self.prompt.clear();
        self.prompt_cursor = 0;
        self.prompt_selected = false;
        self.prompt_selection_anchor = None;
        self.command_provider = None;
    }

    fn delete_prompt_selection(&mut self) -> bool {
        let selection = if self.prompt_selected && self.prompt_selection_anchor.is_none() {
            Some((0, self.prompt.len()))
        } else {
            self.prompt_selection()
        };
        let Some((start, end)) = selection else {
            return false;
        };
        self.prompt.drain(start..end);
        self.prompt_cursor = start;
        self.prompt_selection_anchor = None;
        self.prompt_selected = false;
        self.command_provider = None;
        true
    }

    fn drain_runtime_events(&mut self) -> Option<(String, Vec<AgentEvent>)> {
        let events = self.session.try_events().collect::<Vec<_>>();
        if events.is_empty() {
            return None;
        }
        self.last_changed_at_millis = unix_time_millis();
        Some((self.conversation_id.clone(), events))
    }

    fn on_runtime_events(&mut self, events: Vec<AgentEvent>) -> (Vec<SessionUpdate>, bool) {
        let mut updates = Vec::new();
        let mut transcript_changed = false;
        for event in events {
            transcript_changed |= self.on_runtime_event(event, &mut updates);
        }
        transcript_changed |= self.flush_queued_message();
        (updates, transcript_changed)
    }

    fn on_runtime_event(&mut self, event: AgentEvent, updates: &mut Vec<SessionUpdate>) -> bool {
        let mut transcript_changed = false;
        match event {
            AgentEvent::SessionStarted { id, .. } => {
                self.session_id = Some(id.clone());
                let name = self.pending_session_name.take();
                let conversation_id = self.pending_conversation_id.take();
                updates.push((
                    self.session.provider(),
                    id,
                    name,
                    conversation_id,
                    self.rpc_token.clone(),
                ));
            }
            AgentEvent::SessionNameChanged { id, name } => {
                updates.push((
                    self.session.provider(),
                    id,
                    Some(name),
                    None,
                    self.rpc_token.clone(),
                ));
            }
            AgentEvent::Ready => {
                self.activity = AgentActivity::Idle;
                self.status = "Ready".to_owned();
            }
            AgentEvent::ConversationReset(conversation) => {
                self.activity = AgentActivity::Idle;
                self.conversation = conversation;
                transcript_changed = true;
            }
            AgentEvent::Conversation(update) => {
                self.apply_conversation_update(*update);
                transcript_changed = true;
            }
            AgentEvent::Status(status) => self.status = status,
            AgentEvent::Question(request) => {
                self.activity = AgentActivity::WaitingForInput;
                self.status = "Waiting for answer".to_owned();
                self.pending_question = Some(PendingQuestion {
                    request,
                    current: 0,
                    answers: Vec::new(),
                });
            }
            AgentEvent::Error(error) => {
                self.activity = AgentActivity::Error;
                self.status = "Error".to_owned();
                self.transcript
                    .push(TranscriptEntry::Activity(format!("[error] {error}")));
            }
            AgentEvent::TurnCompleted => {
                self.activity = AgentActivity::Idle;
                self.completed_turns = self.completed_turns.saturating_add(1);
                self.status = "Ready".to_owned();
            }
        }
        transcript_changed
    }

    fn apply_conversation_update(&mut self, update: ConversationUpdate) {
        match update {
            ConversationUpdate::Append { event } => {
                if let EventPayload::ToolCall { name, input, .. } = &event.payload {
                    self.activity = if name.eq_ignore_ascii_case("reasoning") {
                        AgentActivity::Thinking
                    } else {
                        AgentActivity::Active
                    };
                    if self
                        .diff_state
                        .capture(&event.id, input, self.transcript.len())
                    {
                        let _ = self.diff_state.save(&self.session_directory);
                    }
                }
                append_conversation_event(&mut self.conversation.events, event);
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
        self.transcript = interleave_transcript(
            &self.conversation.events,
            self.plugin_installs.entries(),
            cache,
            cache_directory,
        );
        self.transcript_dirty = false;
    }

    /// Reduces one plugin install event, reporting whether this transcript
    /// changed because of it.
    fn on_plugin_install(&mut self, event: &PluginInstallEvent) -> bool {
        self.plugin_installs
            .on_event(event, self.conversation.events.len())
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
                self.activity = AgentActivity::Active;
                self.status = "Working".to_owned();
            }
            Err(error) => {
                self.activity = AgentActivity::Error;
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
        self.clear_prompt();
        self.queued_messages
            .push_back(QueuedMessage { prompt, images });
        if self.flush_queued_message() {
            self.rebuild_transcript();
        }
        suggested_name
    }

    fn flush_queued_message(&mut self) -> bool {
        if self.activity.is_busy() {
            return false;
        }
        let Some(queued) = self.queued_messages.pop_front() else {
            return false;
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
        match self.session.send(queued.prompt, queued.images) {
            Ok(()) => {
                self.activity = AgentActivity::Active;
                self.status = "Working".to_owned();
            }
            Err(error) => {
                self.activity = AgentActivity::Error;
                self.status = "Error".to_owned();
                self.transcript
                    .push(TranscriptEntry::Activity(format!("[error] {error}")));
            }
        }
        true
    }

    fn transcript_len(&self) -> usize {
        self.transcript.iter().map(transcript_entry_len).sum()
    }
}

/// How much a transcript entry contributes to the rendered length. The view
/// scrolls to the end whenever this total grows.
fn transcript_entry_len(entry: &TranscriptEntry) -> usize {
    match entry {
        TranscriptEntry::User {
            message,
            attachments,
            images,
        } => {
            message.len() + attachments + images.iter().map(|image| image.data.len()).sum::<usize>()
        }
        TranscriptEntry::Assistant { source, .. } => source.len(),
        TranscriptEntry::CommandExecution { source, .. } => source.len(),
        TranscriptEntry::FileChanges { status, changes } => {
            status.len()
                + changes
                    .iter()
                    .map(|change| change.path.len() + change.description.len())
                    .sum::<usize>()
        }
        TranscriptEntry::WebSearch { queries } => queries.iter().map(String::len).sum(),
        TranscriptEntry::FileRead { path, status, .. } => path.len() + status.len(),
        TranscriptEntry::PluginInstall(install) => install.command.len() + install.output.len(),
        TranscriptEntry::Activity(message) => message.len(),
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

fn rpc_caller(context: &SessionContext) -> serde_json::Value {
    serde_json::json!({
        "agency_session_id": context.conversation_id,
        "provider": context.provider,
        "provider_session_id": context.provider_session_id,
        "generation": context.generation
    })
}

fn agency_mcp_command() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agency"))
}

fn worktree_json(worktree: Worktree) -> serde_json::Value {
    serde_json::json!({
        "path": worktree.path,
        "label": worktree.label,
        "branch": worktree.branch
    })
}

impl TranscriptEntry {
    fn assistant(source: String) -> Self {
        let content = markdown::Content::parse(&source);
        Self::Assistant { source, content }
    }

    fn command_execution(
        command: String,
        output: String,
        status: String,
        exit_code: Option<i64>,
    ) -> Self {
        let source = fenced_command(&command, command_language());
        let content = markdown::Content::parse(&source);
        Self::CommandExecution {
            source,
            content,
            output,
            status,
            exit_code,
        }
    }
}

/// Records a reported event. Work an agent reports twice — once when it starts
/// and once when it finishes — keeps the place it already has, so the finished
/// report replaces the started one instead of appearing beside it.
fn append_conversation_event(events: &mut Vec<ConversationEvent>, mut event: ConversationEvent) {
    if let Some(existing) = events.iter_mut().find(|existing| existing.id == event.id) {
        existing.payload = event.payload;
        existing.native = event.native;
        return;
    }
    if event.parent_id.is_none() {
        event.parent_id = events.last().map(|event| event.id.clone());
    }
    events.push(event);
}

/// Renders conversation events and plugin installs as one transcript, keeping
/// each install where it was started.
fn interleave_transcript(
    events: &[ConversationEvent],
    installs: &[PluginInstallEntry],
    image_cache: &mut HashMap<String, TranscriptImage>,
    image_cache_directory: &std::path::Path,
) -> Vec<TranscriptEntry> {
    let mut transcript = Vec::new();
    let mut installs = installs.iter();
    let mut pending = installs.next();

    for (index, event) in events.iter().enumerate() {
        while pending.is_some_and(|install| install.after_events <= index) {
            if let Some(install) = pending.take() {
                transcript.push(TranscriptEntry::PluginInstall(install.clone()));
            }
            pending = installs.next();
        }
        if let Some(entry) = transcript_entry(event, image_cache, image_cache_directory) {
            transcript.push(entry);
        }
    }
    transcript.extend(
        pending
            .into_iter()
            .chain(installs)
            .cloned()
            .map(TranscriptEntry::PluginInstall),
    );

    transcript
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
            // Work every agent does is normalized into the canonical tool
            // vocabulary, so one handler renders it the same for all of them.
            match tools::kind(input) {
                Some(tools::FILE_CHANGE) => {
                    return Some(TranscriptEntry::FileChanges {
                        status: tools::status(input).to_owned(),
                        changes: file_changes(input),
                    });
                }
                Some(tools::COMMAND_EXECUTION) => {
                    return Some(TranscriptEntry::command_execution(
                        input
                            .get("command")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(name)
                            .to_owned(),
                        input
                            .get("aggregatedOutput")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        tools::status(input).to_owned(),
                        input.get("exitCode").and_then(serde_json::Value::as_i64),
                    ));
                }
                Some(tools::FILE_READ) => {
                    return Some(TranscriptEntry::FileRead {
                        path: input
                            .get("path")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        status: tools::status(input).to_owned(),
                        lines: input.get("lines").and_then(serde_json::Value::as_u64),
                    });
                }
                _ => {}
            }
            if is_web_search(name, input) {
                return Some(TranscriptEntry::WebSearch {
                    queries: web_search_queries(input),
                });
            }
            Some(TranscriptEntry::Activity(format!(
                "[tool] {name} {}",
                compact_json(input)
            )))
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

fn is_web_search(name: &str, input: &serde_json::Value) -> bool {
    let normalized = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("websearch")
        || input
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("webSearch"))
        || input.get("search_query").is_some()
}

fn web_search_queries(input: &serde_json::Value) -> Vec<String> {
    let mut queries = ["query", "q"]
        .into_iter()
        .filter_map(|key| input.get(key).and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(searches) = input
        .get("search_query")
        .and_then(serde_json::Value::as_array)
    {
        queries.extend(searches.iter().filter_map(|search| {
            search
                .get("q")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        }));
    }
    if queries.is_empty() {
        queries.push("Web search".to_owned());
    }
    queries
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
    use agency_translator_api::commands::AgentCommand;

    /// The cursor has to sit on the line and column the byte offset names. Drawn
    /// from a single row, as it was, it landed at a horizontal offset that ignored
    /// every line break before it.
    #[test]
    fn the_composer_cursor_lands_on_its_own_line_and_column() {
        assert_eq!(
            composer_lines("abc\ndef", 5, None),
            vec![
                vec![PromptSpan::Text {
                    range: 0..3,
                    selected: false
                }],
                vec![
                    PromptSpan::Text {
                        range: 4..5,
                        selected: false
                    },
                    PromptSpan::Cursor,
                    PromptSpan::Text {
                        range: 5..7,
                        selected: false
                    },
                ],
            ]
        );
    }

    /// `str::lines` drops a trailing empty line, which would leave the cursor
    /// after a final newline with nowhere to draw.
    #[test]
    fn a_trailing_newline_keeps_a_final_line_for_the_cursor() {
        assert_eq!(
            composer_lines("abc\n", 4, None),
            vec![
                vec![PromptSpan::Text {
                    range: 0..3,
                    selected: false
                }],
                vec![PromptSpan::Cursor],
            ]
        );
    }

    #[test]
    fn a_blank_interior_line_still_occupies_a_row() {
        let lines = composer_lines("a\n\nb", 0, None);

        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            vec![
                PromptSpan::Cursor,
                PromptSpan::Text {
                    range: 0..1,
                    selected: false
                }
            ]
        );
        assert!(lines[1].is_empty());
    }

    /// A selection crossing line breaks has to mark a partial first line, a whole
    /// middle line, and a partial last line, with the newline itself drawing
    /// nothing.
    #[test]
    fn a_selection_spanning_lines_marks_each_line_it_covers() {
        assert_eq!(
            composer_lines("one\ntwo\nthree", 9, Some((2, 9))),
            vec![
                vec![
                    PromptSpan::Text {
                        range: 0..2,
                        selected: false
                    },
                    PromptSpan::Text {
                        range: 2..3,
                        selected: true
                    },
                ],
                vec![PromptSpan::Text {
                    range: 4..7,
                    selected: true
                }],
                vec![
                    PromptSpan::Text {
                        range: 8..9,
                        selected: true
                    },
                    PromptSpan::Cursor,
                    PromptSpan::Text {
                        range: 9..13,
                        selected: false
                    },
                ],
            ]
        );
    }

    #[test]
    fn event_bus_preserves_publish_order_for_follow_up_events() {
        let mut bus = EventBus::default();
        assert_eq!(bus.publish("open"), 0);
        assert_eq!(bus.publish("focus"), 1);

        let first = bus.next().unwrap();
        let second = bus.next().unwrap();
        assert_eq!((first.sequence, first.event), (0, "open"));
        assert_eq!((second.sequence, second.event), (1, "focus"));
        assert!(bus.next().is_none());
    }

    #[test]
    fn prompt_insertions_clamp_stale_cursors_to_char_boundaries() {
        // A cleared prompt used to leave the cursor past the end, which made the
        // next typed character panic inside `String::insert_str`.
        assert_eq!(clamped_prompt_cursor("", 7), 0);
        assert_eq!(clamped_prompt_cursor("héllo", 99), "héllo".len());
        assert_eq!(clamped_prompt_cursor("héllo", 2), 1);
    }

    /// Every motion helper splits lines on `'\n'`, so a `'\r'` that reaches the
    /// model is a line break nothing can see. Normalizing at the single point
    /// where text enters the prompt is what makes that invariant hold.
    #[test]
    fn text_entering_the_prompt_normalizes_every_line_break_to_a_newline() {
        assert_eq!(normalize_newlines("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(normalize_newlines("already\nfine"), "already\nfine");
        assert_eq!(normalize_newlines("no breaks"), "no breaks");
    }

    /// The arithmetic `insert_prompt_text` depends on: a cursor computed from
    /// the pre-normalized length would land short by one per `\r` stripped, so
    /// after inserting `"a\r\nb\rc"` into an empty prompt the cursor must sit
    /// at the end of the normalized `"a\nb\nc"`, not the end of the original
    /// six-byte string.
    #[test]
    fn insert_normalized_normalizes_and_advances_the_cursor_past_the_inserted_text() {
        let mut prompt = String::new();
        let cursor = insert_normalized(&mut prompt, 0, "a\r\nb\rc");
        assert_eq!(prompt, "a\nb\nc");
        assert_eq!(cursor, prompt.len());
    }

    /// A midstream insertion, so the returned cursor is not trivially the
    /// whole string's length — it has to be the insertion point plus the
    /// normalized text's own length.
    #[test]
    fn insert_normalized_advances_from_a_cursor_in_the_middle_of_existing_text() {
        let mut prompt = "before after".to_owned();
        let cursor = insert_normalized(&mut prompt, "before".len(), " middle");
        assert_eq!(prompt, "before middle after");
        assert_eq!(cursor, "before middle".len());
    }

    #[test]
    fn hidden_optional_focus_surface_does_not_claim_row_width() {
        let surface: Element<'static, AppEvent> =
            toolbar_panel_surface(text("hidden panel").into(), false, false);

        assert_eq!(surface.as_widget().size().width, Length::Shrink);
    }

    #[test]
    fn collapsed_toolbar_focus_surface_does_not_expand_to_fill() {
        let toolbar: Element<'static, AppEvent> = toolbar_surface(
            iced::widget::Space::new().width(Length::Fixed(48.0)).into(),
            false,
        );

        assert_eq!(toolbar.as_widget().size().width, Length::Shrink);
    }

    #[test]
    fn layout_facet_reduces_events_without_external_flag_coordination() {
        let mut layout = LayoutState::default();

        layout.on_event(&AppEvent::ToggleActivity(SidebarTool::Explorer));
        assert!(layout.toolbar_visible);
        assert_eq!(layout.sidebar_tool, SidebarTool::Explorer);

        layout.on_event(&AppEvent::TerminalVisibilityChanged(true));
        assert!(layout.terminal_visible);
        assert!(!layout.settings_open);

        layout.on_event(&AppEvent::EnterComposer);
        assert!(!layout.toolbar_visible);
        assert!(!layout.terminal_visible);
    }

    #[test]
    fn input_mode_facet_reduces_mode_switch_events_for_composer_hints() {
        let mut input_mode = InputModeState::default();
        assert!(input_mode.composer_needs_insert_hint());

        input_mode.on_event(&AppEvent::InputModeChanged { mode: Mode::Insert });
        assert!(!input_mode.composer_needs_insert_hint());

        input_mode.on_event(&AppEvent::InputModeChanged { mode: Mode::Normal });
        assert!(input_mode.composer_needs_insert_hint());

        input_mode.on_event(&AppEvent::InputModeChanged {
            mode: Mode::Terminal,
        });
        assert!(!input_mode.composer_needs_insert_hint());
    }

    #[test]
    fn agent_activity_has_one_explicit_badge_mapping() {
        assert_eq!(
            AgentActivity::WaitingForInput.badge(),
            ui_theme::AgentStatus::Waiting
        );
        assert_eq!(
            AgentActivity::Thinking.badge(),
            ui_theme::AgentStatus::Thinking
        );
        assert_eq!(AgentActivity::Active.badge(), ui_theme::AgentStatus::Active);
        assert_eq!(AgentActivity::Idle.badge(), ui_theme::AgentStatus::Idle);
        assert_eq!(
            AgentActivity::Starting.badge(),
            ui_theme::AgentStatus::Active
        );
    }

    #[test]
    fn composer_vertical_motions_preserve_the_column_and_clamp_short_lines() {
        let text = "alpha\nxy\nomega";

        assert_eq!(
            composer_motion_target(text, 4, keybindings::ComposerMotion::Down),
            8
        );
        assert_eq!(
            composer_motion_target(text, 8, keybindings::ComposerMotion::Down),
            11
        );
        assert_eq!(
            composer_motion_target(text, 11, keybindings::ComposerMotion::Up),
            8
        );
    }

    #[test]
    fn composer_document_motions_reach_both_ends() {
        let text = "one\ntwo";

        assert_eq!(
            composer_motion_target(text, 5, keybindings::ComposerMotion::DocumentStart),
            0
        );
        assert_eq!(
            composer_motion_target(text, 2, keybindings::ComposerMotion::DocumentEnd),
            text.len()
        );
    }

    #[test]
    fn transcript_visibility_requires_the_active_uncovered_session() {
        assert!(transcript_is_visible(
            Some("active"),
            false,
            false,
            "active"
        ));
        assert!(!transcript_is_visible(
            Some("background"),
            false,
            false,
            "active"
        ));
        assert!(!transcript_is_visible(
            Some("active"),
            true,
            false,
            "active"
        ));
        assert!(!transcript_is_visible(
            Some("active"),
            false,
            true,
            "active"
        ));
        assert!(!transcript_is_visible(None, false, false, "active"));
    }

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
    fn file_changes_become_structured_transcript_cards() {
        let event = ConversationEvent {
            id: "file-change-1".to_owned(),
            parent_id: None,
            turn_id: None,
            source: ClientId::new("codex"),
            payload: EventPayload::ToolCall {
                id: Some("file-change-1".to_owned()),
                name: "apply_patch".to_owned(),
                input: serde_json::json!({
                    "type": "fileChange",
                    "status": "completed",
                    "changes": [{
                        "path": "src/main.rs",
                        "kind": "update",
                        "diff": "@@ -1 +1 @@\n-old\n+new"
                    }]
                }),
            },
            native: None,
        };

        let Some(TranscriptEntry::FileChanges { status, changes }) =
            transcript_entry(&event, &mut HashMap::new(), &std::env::temp_dir())
        else {
            unreachable!();
        };
        assert_eq!(status, "completed");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "src/main.rs");
        assert_eq!(changes[0].description, "Updated · +1 −1");
    }

    fn user_event(id: &str, message: &str) -> ConversationEvent {
        ConversationEvent {
            id: id.to_owned(),
            parent_id: None,
            turn_id: None,
            source: ClientId::new("agency"),
            payload: EventPayload::Message {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: message.to_owned(),
                }],
            },
            native: None,
        }
    }

    fn install_entry(id: u64, after_events: usize) -> PluginInstallEntry {
        PluginInstallEntry {
            id,
            provider: Provider::Claude,
            command: "claude plugin marketplace add owner/repo".to_owned(),
            output: String::new(),
            status: plugins::InstallStatus::Running,
            detail: None,
            after_events,
        }
    }

    #[test]
    fn plugin_installs_keep_their_place_when_the_transcript_is_rebuilt() {
        let events = [user_event("1", "first"), user_event("2", "second")];
        let installs = [install_entry(0, 1), install_entry(1, 2)];

        let transcript = interleave_transcript(
            &events,
            &installs,
            &mut HashMap::new(),
            &std::env::temp_dir(),
        );

        assert!(matches!(
            transcript.as_slice(),
            [
                TranscriptEntry::User { .. },
                TranscriptEntry::PluginInstall(first),
                TranscriptEntry::User { .. },
                TranscriptEntry::PluginInstall(second),
            ] if first.id == 0 && second.id == 1
        ));
    }

    #[test]
    fn plugin_installs_started_before_any_message_lead_the_transcript() {
        let transcript = interleave_transcript(
            &[user_event("1", "first")],
            &[install_entry(0, 0)],
            &mut HashMap::new(),
            &std::env::temp_dir(),
        );

        assert!(matches!(
            transcript.as_slice(),
            [
                TranscriptEntry::PluginInstall(_),
                TranscriptEntry::User { .. },
            ]
        ));
    }

    #[test]
    fn streamed_plugin_output_grows_the_transcript_so_it_scrolls() {
        let mut installs = TranscriptInstalls::default();
        let started = PluginInstallEvent::Started {
            id: 0,
            conversation_id: "conversation".to_owned(),
            provider: Provider::Codex,
            command: "codex plugin marketplace add owner/repo".to_owned(),
        };
        installs.on_event(&started, 0);
        let before = interleave_transcript(
            &[],
            installs.entries(),
            &mut HashMap::new(),
            &std::env::temp_dir(),
        );

        installs.on_event(
            &PluginInstallEvent::Output {
                id: 0,
                conversation_id: "conversation".to_owned(),
                provider: Provider::Codex,
                output: "cloning owner/repo".to_owned(),
            },
            0,
        );
        let after = interleave_transcript(
            &[],
            installs.entries(),
            &mut HashMap::new(),
            &std::env::temp_dir(),
        );

        let rendered = |transcript: &[TranscriptEntry]| {
            transcript.iter().map(transcript_entry_len).sum::<usize>()
        };
        assert!(rendered(&after) > rendered(&before));
    }

    fn tool_call_event(id: &str, name: &str, input: serde_json::Value) -> ConversationEvent {
        ConversationEvent {
            id: id.to_owned(),
            parent_id: None,
            turn_id: None,
            source: ClientId::new("agency"),
            payload: EventPayload::ToolCall {
                id: Some(id.to_owned()),
                name: name.to_owned(),
                input,
            },
            native: None,
        }
    }

    #[test]
    fn commands_render_from_the_canonical_call_whichever_agent_ran_them() {
        let event = tool_call_event(
            "exec-1",
            "echo hello",
            tools::command_execution("echo hello", tools::COMPLETED, Some("hello\n"), Some(0)),
        );

        let Some(TranscriptEntry::CommandExecution {
            source,
            output,
            status,
            exit_code,
            ..
        }) = transcript_entry(&event, &mut HashMap::new(), &std::env::temp_dir())
        else {
            panic!("a command should render as a command");
        };
        assert!(source.contains("echo hello"));
        assert_eq!(output, "hello\n");
        assert_eq!(status, tools::COMPLETED);
        assert_eq!(exit_code, Some(0));
    }

    #[test]
    fn a_running_command_renders_before_it_has_output() {
        let event = tool_call_event(
            "exec-1",
            "cargo test",
            tools::command_execution("cargo test", tools::IN_PROGRESS, None, None),
        );

        let Some(TranscriptEntry::CommandExecution { output, status, .. }) =
            transcript_entry(&event, &mut HashMap::new(), &std::env::temp_dir())
        else {
            panic!("a running command should still render as a command");
        };
        assert!(output.is_empty());
        assert_eq!(status_label(&status), "Running");
    }

    #[test]
    fn reads_become_structured_transcript_cards() {
        let event = tool_call_event(
            "read-1",
            tools::FILE_READ,
            tools::file_read("src/main.rs", tools::COMPLETED, Some(42)),
        );

        let Some(TranscriptEntry::FileRead {
            path,
            status,
            lines,
        }) = transcript_entry(&event, &mut HashMap::new(), &std::env::temp_dir())
        else {
            panic!("a read should render as a read");
        };
        assert_eq!((path.as_str(), lines), ("src/main.rs", Some(42)));
        assert_eq!(status, tools::COMPLETED);
    }

    #[test]
    fn finished_work_replaces_the_report_that_started_it() {
        let mut events = vec![
            user_event("1", "Run the tests"),
            tool_call_event(
                "exec-1",
                "cargo test",
                tools::command_execution("cargo test", tools::IN_PROGRESS, None, None),
            ),
        ];

        append_conversation_event(
            &mut events,
            tool_call_event(
                "exec-1",
                "cargo test",
                tools::command_execution("cargo test", tools::COMPLETED, Some("ok\n"), Some(0)),
            ),
        );

        assert_eq!(events.len(), 2, "the command keeps its place");
        let EventPayload::ToolCall { input, .. } = &events[1].payload else {
            unreachable!()
        };
        assert_eq!(tools::status(input), tools::COMPLETED);
        assert_eq!(input["aggregatedOutput"], "ok\n");
    }

    #[test]
    fn newly_reported_work_follows_what_came_before_it() {
        let mut events = vec![user_event("1", "Run the tests")];

        append_conversation_event(
            &mut events,
            tool_call_event("exec-1", "cargo test", serde_json::json!({})),
        );

        assert_eq!(events.len(), 2);
        assert_eq!(events[1].parent_id.as_deref(), Some("1"));
    }

    #[test]
    fn web_search_calls_become_structured_transcript_cards() {
        let event = ConversationEvent {
            id: "web-search-1".to_owned(),
            parent_id: None,
            turn_id: None,
            source: ClientId::new("codex"),
            payload: EventPayload::ToolCall {
                id: Some("web-search-1".to_owned()),
                name: "webSearch".to_owned(),
                input: serde_json::json!({
                    "search_query": [
                        { "q": "Iced syntax highlighting" },
                        { "q": "Tokyo Night palette" }
                    ]
                }),
            },
            native: None,
        };

        let Some(TranscriptEntry::WebSearch { queries }) =
            transcript_entry(&event, &mut HashMap::new(), &std::env::temp_dir())
        else {
            unreachable!();
        };
        assert_eq!(queries, ["Iced syntax highlighting", "Tokyo Night palette"]);
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

    #[test]
    fn configured_agent_detection_checks_every_path_entry() {
        let root =
            std::env::temp_dir().join(format!("agency-configured-agents-{}", std::process::id()));
        let codex_directory = root.join("codex-bin");
        let claude_directory = root.join("claude-bin");
        std::fs::create_dir_all(&codex_directory).unwrap();
        std::fs::create_dir_all(&claude_directory).unwrap();
        std::fs::write(codex_directory.join("codex"), []).unwrap();
        std::fs::write(claude_directory.join("claude"), []).unwrap();
        let path = std::env::join_paths([codex_directory, claude_directory]).unwrap();

        assert_eq!(
            configured_agents(Some(&path)),
            vec![Provider::Codex, Provider::Claude]
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn agency_mcp_state_tracks_auto_injected_agent_connections() {
        assert_eq!(agency_mcp_server_state([]), McpServerState::Connected);
        assert_eq!(
            agency_mcp_server_state([McpStatus::Connected]),
            McpServerState::Connected
        );
        assert_eq!(
            agency_mcp_server_state([McpStatus::Connected, McpStatus::Waiting]),
            McpServerState::Connected
        );
        assert_eq!(
            agency_mcp_server_state([McpStatus::Connected, McpStatus::Disconnected]),
            McpServerState::Error
        );
    }

    fn agent_command(name: &str) -> AgentCommand {
        AgentCommand {
            name: name.to_owned(),
            description: "Does a thing".to_owned(),
            invocation: format!("/{name} "),
            argument_hint: None,
            origin: agency_translator_api::commands::CommandOrigin::Personal,
        }
    }

    /// A command belonging to another agent has to be routed there rather than
    /// sent where it was typed.
    #[test]
    fn a_command_from_another_agent_needs_a_switch() {
        assert!(command_needs_agent_switch(
            Provider::Codex,
            Provider::Claude
        ));
        assert!(command_needs_agent_switch(
            Provider::Claude,
            Provider::Codex
        ));
        assert!(!command_needs_agent_switch(
            Provider::Claude,
            Provider::Claude
        ));
        assert!(!command_needs_agent_switch(
            Provider::Codex,
            Provider::Codex
        ));
    }

    /// The routing decision is only as good as the provider the catalog stamps
    /// on each row, so this pins the two together: a Claude entry picked while
    /// Codex is focused must route away, and Agency's own commands must carry no
    /// provider at all so they never reach the switch.
    #[test]
    fn the_catalog_stamps_the_provider_that_routing_depends_on() {
        let catalog = slash_commands::merge_catalog(vec![
            (Provider::Claude, agent_command("superpowers:brainstorming")),
            (Provider::Codex, agent_command("review")),
        ]);

        let brainstorming = catalog
            .iter()
            .find(|completion| completion.command == "/superpowers:brainstorming")
            .expect("the Claude entry should be listed");
        let owner = brainstorming
            .provider
            .expect("an agent command must name its owner or it cannot be routed");
        assert_eq!(owner, Provider::Claude);
        assert!(command_needs_agent_switch(Provider::Codex, owner));
        assert!(!command_needs_agent_switch(Provider::Claude, owner));

        // Agency handles its own commands, so there is nobody to switch to.
        let init = catalog
            .iter()
            .find(|completion| completion.command == "/init")
            .expect("Agency's own commands are always listed");
        assert_eq!(init.provider, None);
    }

    /// Resolution and routing have to agree. The provider a resolved submission
    /// names is the one `command_needs_agent_switch` is asked about, so if they
    /// ever disagreed a command would either strand on the wrong agent or churn
    /// between two. This also pins that the rewritten prompt is what gets sent.
    #[test]
    fn a_resolved_submission_names_the_agent_that_routing_will_switch_to() {
        let catalog = slash_commands::merge_catalog(vec![(
            Provider::Claude,
            agent_command("superpowers:brainstorming"),
        )]);

        let resolved = slash_commands::resolve_submission(
            &catalog,
            "/brainstorming an idea",
            Some(Provider::Codex),
        )
        .expect("a catalog command is not an Agency usage error");

        let slash_commands::Submission::Agent { provider, prompt } = resolved else {
            panic!("a catalog command must resolve to the agent that owns it");
        };
        assert_eq!(provider, Provider::Claude);
        assert_eq!(prompt, "/superpowers:brainstorming an idea");
        assert!(command_needs_agent_switch(Provider::Codex, provider));
        assert!(!command_needs_agent_switch(Provider::Claude, provider));
    }

    /// The rows offered first must be exactly the ones that will not be
    /// rerouted. Ranking and `command_needs_agent_switch` are separate
    /// decisions; if they ever disagree, the top of the list stops meaning
    /// "the agent you are talking to" and Enter starts committing a row that
    /// silently switches agents.
    #[test]
    fn the_commands_offered_first_are_the_ones_that_need_no_switch() {
        let catalog = slash_commands::merge_catalog(vec![
            (Provider::Claude, agent_command("superpowers:brainstorming")),
            (Provider::Codex, agent_command("review")),
        ]);

        for active in [Provider::Codex, Provider::Claude] {
            let mut seen_a_switch = false;
            for completion in slash_command_completions(&catalog, "/", Some(active)) {
                let Some(owner) = completion.provider else {
                    assert!(
                        !seen_a_switch,
                        "Agency's own commands must lead, but {} came after an agent's",
                        completion.command
                    );
                    continue;
                };
                if command_needs_agent_switch(active, owner) {
                    seen_a_switch = true;
                } else {
                    assert!(
                        !seen_a_switch,
                        "{} needs no switch but was listed below one that does",
                        completion.command
                    );
                }
            }
        }
    }

    // `slash_commands::tests::agency_commands_are_always_offered` and
    // `a_translator_command_keeps_its_invocation_and_origin` already cover
    // `merge_catalog`'s pure behavior (agency commands always present, agent
    // commands take their own invocation). The reducer tests below instead
    // pin how `Agency` *uses* that function: that loading is a replace, not
    // an accumulate, and that failure leaves the prior state alone.

    #[test]
    fn a_second_loaded_catalog_replaces_the_first_rather_than_accumulating() {
        let mut agency = Agency::for_testing();

        let _ = agency.reduce_event(AppEvent::SlashCatalogLoaded(vec![(
            Provider::Claude,
            agent_command("deploy"),
        )]));
        let _ = agency.reduce_event(AppEvent::SlashCatalogLoaded(vec![(
            Provider::Claude,
            agent_command("audit"),
        )]));

        assert!(
            agency
                .slash_command_catalog
                .iter()
                .any(|completion| completion.command == "/audit")
        );
        assert!(
            !agency
                .slash_command_catalog
                .iter()
                .any(|completion| completion.command == "/deploy")
        );
    }

    #[test]
    fn a_failed_load_leaves_the_previous_catalog_in_place() {
        let mut agency = Agency::for_testing();
        let loaded =
            slash_commands::merge_catalog(vec![(Provider::Claude, agent_command("deploy"))]);
        agency.slash_command_catalog = loaded;
        let before = agency.slash_command_catalog.clone();

        let _ = agency.reduce_event(AppEvent::SlashCatalogFailed("disk on fire".to_owned()));

        assert_eq!(agency.slash_command_catalog, before);
        assert_eq!(agency.notice.as_deref(), Some("disk on fire"));
    }

    #[test]
    fn a_loaded_catalog_is_stored_and_leaves_any_existing_notice_untouched() {
        let mut agency = Agency::for_testing();
        agency.notice = Some("unrelated notice".to_owned());

        let _ = agency.reduce_event(AppEvent::SlashCatalogLoaded(vec![(
            Provider::Claude,
            agent_command("deploy"),
        )]));

        assert!(
            agency
                .slash_command_catalog
                .iter()
                .any(|completion| completion.command == "/deploy")
        );
        // Unlike SlashCatalogFailed, a successful load has no notice of its
        // own to report, and must not clobber one set by something else.
        assert_eq!(agency.notice.as_deref(), Some("unrelated notice"));
    }

    #[test]
    fn a_loaded_or_failed_catalog_publishes_no_follow_up_events() {
        // Both handlers only assign a field; if either started publishing
        // events, a loaded/failed catalog could re-trigger its own reload.
        let mut agency = Agency::for_testing();
        // `build` always publishes the initial `WorktreesDiscovered`, which is
        // unrelated to what this test checks; drain it before asserting on
        // events raised by the handlers under test.
        agency.drain_events();

        let _ = agency.reduce_event(AppEvent::SlashCatalogLoaded(Vec::new()));
        assert!(agency.drain_events().is_empty());

        let _ = agency.reduce_event(AppEvent::SlashCatalogFailed("disk on fire".to_owned()));
        assert!(agency.drain_events().is_empty());
    }

    /// Startup discovery goes through the same reducer as creation and
    /// removal, so a new Agency publishes its initial worktree list rather
    /// than only assigning it. Without this, the startup publish is
    /// indistinguishable from dead code.
    #[test]
    fn a_new_agency_publishes_its_initial_worktree_discovery() {
        let mut agency = Agency::for_testing();

        assert!(matches!(
            agency.drain_events().as_slice(),
            [AppEvent::WorktreesDiscovered { .. }]
        ));
    }

    #[test]
    fn switching_worktrees_reseeds_agency_commands_and_requests_a_reload() {
        let mut agency = Agency::for_testing();
        agency.slash_command_catalog =
            slash_commands::merge_catalog(vec![(Provider::Claude, agent_command("deploy"))]);

        let other_root =
            std::env::temp_dir().join(format!("agency-worktree-switch-{}", std::process::id()));
        std::fs::create_dir_all(&other_root).unwrap();
        agency.worktrees.push(Worktree {
            label: "other".to_owned(),
            path: other_root.clone(),
            branch: None,
        });
        let other_index = agency.worktrees.len() - 1;
        assert_ne!(other_index, agency.active_worktree);

        let _ = agency.reduce_event(AppEvent::SelectWorktree(other_index));

        // The agent half is stale the instant cwd changes, so the catalog
        // drops back to what's always available while the reload is pending.
        assert_eq!(
            agency.slash_command_catalog,
            slash_commands::agency_commands()
        );
        assert!(
            agency
                .drain_events()
                .iter()
                .any(|event| matches!(event, AppEvent::SlashCatalogRequested))
        );

        std::fs::remove_dir_all(&other_root).unwrap();
    }

    fn worktree_at(path: &std::path::Path, branch: &str) -> Worktree {
        Worktree {
            path: path.to_path_buf(),
            label: branch.to_owned(),
            branch: Some(branch.to_owned()),
        }
    }

    /// Discovery is the single source of truth for the tab strip, so the active
    /// tab is re-derived from cwd rather than carried across. Git reports the
    /// primary first, which is why index 0 is the fallback.
    #[test]
    fn discovering_worktrees_reresolves_the_active_tab_from_cwd() {
        let mut agency = Agency::for_testing();
        let primary = std::path::PathBuf::from("/repo");
        let feature = std::path::PathBuf::from("/repo/.agency/worktrees/feature");
        agency.cwd = feature.clone();

        let _ = agency.reduce_event(AppEvent::WorktreesDiscovered {
            worktrees: vec![
                worktree_at(&primary, "main"),
                worktree_at(&feature, "feature"),
            ],
        });

        assert_eq!(agency.worktrees.len(), 2);
        assert_eq!(agency.active_worktree, 1);
    }

    #[test]
    fn discovering_worktrees_falls_back_to_the_primary_when_cwd_is_gone() {
        let mut agency = Agency::for_testing();
        agency.cwd = std::path::PathBuf::from("/repo/.agency/worktrees/deleted");

        let _ = agency.reduce_event(AppEvent::WorktreesDiscovered {
            worktrees: vec![worktree_at(std::path::Path::new("/repo"), "main")],
        });

        assert_eq!(agency.active_worktree, 0);
    }

    /// An agent creating a worktree in the background must not move the user's
    /// view. The tab appears; focus stays put.
    ///
    /// This one needs a real repository: the reducer re-discovers rather than
    /// pushing the payload onto the list, so a fabricated path would never show
    /// up in the result.
    #[test]
    fn a_created_worktree_appends_a_tab_without_moving_focus() {
        let root = worktrees::tests_support::repository("created-tab");
        let mut agency = Agency::for_testing();
        agency.cwd = root.clone();
        agency.worktrees = vec![worktree_at(&root, "main")];
        agency.active_worktree = 0;
        let created = worktrees::create(&root, "feature", None).unwrap();

        let _ = agency.reduce_event(AppEvent::WorktreeCreated { worktree: created });

        assert_eq!(agency.worktrees.len(), 2);
        assert!(
            agency
                .worktrees
                .iter()
                .any(|worktree| worktree.branch.as_deref() == Some("feature"))
        );
        assert_eq!(agency.active_worktree, 0);
        assert_eq!(agency.cwd, root);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Task 5 removes the active worktree, clamps `active_worktree`, and
    /// publishes `SelectWorktree(0)` while `cwd` still names the deleted
    /// checkout. An index-equality guard would early-return and strand the app
    /// there, so the guard compares paths.
    #[test]
    fn selecting_the_active_index_still_switches_when_cwd_no_longer_exists() {
        let root = worktrees::tests_support::repository("stale-cwd");
        let mut agency = Agency::for_testing();
        agency.worktrees = vec![worktree_at(&root, "main")];
        agency.active_worktree = 0;
        agency.cwd = root.join(".agency/worktrees/deleted");

        let _ = agency.reduce_event(AppEvent::SelectWorktree(0));

        assert_eq!(agency.cwd, root);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removing_an_inactive_worktree_drops_its_tab_and_keeps_focus() {
        let mut agency = Agency::for_testing();
        let primary = std::path::PathBuf::from("/repo");
        agency.cwd = primary.clone();
        agency.worktrees = vec![
            worktree_at(&primary, "main"),
            worktree_at(
                std::path::Path::new("/repo/.agency/worktrees/feature"),
                "feature",
            ),
        ];
        agency.active_worktree = 0;
        // `Agency::for_testing` leaves the startup `WorktreesDiscovered` on the
        // bus; drain it so the assertion below reflects only this reducer.
        let _ = agency.drain_events();

        let _ = agency.reduce_event(AppEvent::WorktreeRemoved {
            worktree: worktree_at(
                std::path::Path::new("/repo/.agency/worktrees/feature"),
                "feature",
            ),
        });

        assert_eq!(agency.worktrees.len(), 1);
        assert_eq!(agency.worktrees[0].path, primary);
        assert_eq!(agency.active_worktree, 0);
        assert_eq!(agency.cwd, primary);
        assert!(agency.drain_events().is_empty());
    }

    /// Removing the worktree the user is looking at cannot fail the caller —
    /// the tool would then succeed or fail based on which tab happens to be
    /// focused. The app moves to the primary instead, as a follow-up event so
    /// ordering stays deterministic rather than recursing into the handler.
    #[test]
    fn removing_the_active_worktree_falls_back_to_the_primary_tab() {
        let mut agency = Agency::for_testing();
        let primary = std::path::PathBuf::from("/repo");
        let feature = std::path::PathBuf::from("/repo/.agency/worktrees/feature");
        agency.cwd = feature.clone();
        agency.worktrees = vec![
            worktree_at(&primary, "main"),
            worktree_at(&feature, "feature"),
        ];
        agency.active_worktree = 1;

        let _ = agency.reduce_event(AppEvent::WorktreeRemoved {
            worktree: worktree_at(&feature, "feature"),
        });

        assert_eq!(agency.worktrees.len(), 1);
        assert_eq!(agency.worktrees[0].path, primary);
        assert!(
            agency
                .drain_events()
                .iter()
                .any(|event| matches!(event, AppEvent::SelectWorktree(0))),
            "the app must move off the worktree it just deleted"
        );
    }

    /// `remove` trims the branch it is given, so the identifier the caller sent
    /// and the identifier git knows can differ. The event carries the resolved
    /// worktree for exactly that reason: a tab that survives its own removal
    /// leaves the app pointed at a directory that no longer exists.
    #[test]
    fn removing_a_worktree_named_with_stray_whitespace_still_drops_its_tab() {
        let root = worktrees::tests_support::repository("remove-whitespace");
        let created = worktrees::create(&root, "feature", None).unwrap();
        let removed = worktrees::remove(&root, "  feature\n").unwrap();
        assert_eq!(removed.path, created.path);

        let mut agency = Agency::for_testing();
        let _ = agency.drain_events();
        agency.cwd = root.clone();
        agency.worktrees = vec![worktree_at(&root, "main"), created];
        agency.active_worktree = 0;

        let _ = agency.reduce_event(AppEvent::WorktreeRemoved { worktree: removed });

        assert_eq!(agency.worktrees.len(), 1);
        assert_eq!(agency.worktrees[0].path, root);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Worktree resolution must not care who is calling. `blueprint` is not a
    /// provider Agency ships, and the caller block has to carry it through
    /// unchanged — a `match` on provider anywhere in this path fails here
    /// rather than at the next integration.
    #[test]
    fn worktree_calls_resolve_for_a_provider_agency_does_not_ship() {
        let context = SessionContext {
            conversation_id: "conversation-1".to_owned(),
            workspace: std::path::PathBuf::from("/repo"),
            provider: "blueprint".to_owned(),
            provider_session_id: Some("blueprint-9".to_owned()),
            generation: 3,
        };

        assert_eq!(
            rpc_caller(&context),
            serde_json::json!({
                "agency_session_id": "conversation-1",
                "provider": "blueprint",
                "provider_session_id": "blueprint-9",
                "generation": 3
            })
        );
    }

    #[test]
    fn a_finished_plugin_install_requests_a_reload_but_other_plugin_events_do_not() {
        let mut agency = Agency::for_testing();
        let finished = PluginInstallEvent::Finished {
            id: 1,
            conversation_id: "conversation".to_owned(),
            provider: Provider::Claude,
            kind: plugins::InstallKind::Plugin,
            status: plugins::InstallStatus::Installed,
            detail: None,
        };

        let _ = agency.reduce_event(AppEvent::PluginInstall(finished));

        assert!(
            agency
                .drain_events()
                .iter()
                .any(|event| matches!(event, AppEvent::SlashCatalogRequested))
        );

        // Output streams one event per chunk of terminal text; re-indexing on
        // every chunk would make every plugin install feel like it stalls.
        let output = PluginInstallEvent::Output {
            id: 1,
            conversation_id: "conversation".to_owned(),
            provider: Provider::Claude,
            output: "installing...".to_owned(),
        };

        let _ = agency.reduce_event(AppEvent::PluginInstall(output));

        assert!(
            !agency
                .drain_events()
                .iter()
                .any(|event| matches!(event, AppEvent::SlashCatalogRequested))
        );
    }

    #[test]
    fn connecting_an_mcp_server_requests_a_reload() {
        // `add_mcp_server` itself shells out to a real `codex` binary to look
        // the server up; `connect_mcp_server` is everything after that lookup
        // succeeds, which is what actually wires the reload trigger in.
        let mut agency = Agency::for_testing();
        let server = McpServer {
            name: "example".to_owned(),
            enabled: true,
            transport: McpTransport::Stdio {
                command: "example-mcp".to_owned(),
                args: Vec::new(),
                env: None,
                cwd: None,
            },
        };

        agency.connect_mcp_server("example", server).unwrap();

        assert!(
            agency
                .drain_events()
                .iter()
                .any(|event| matches!(event, AppEvent::SlashCatalogRequested))
        );
    }

    #[test]
    fn refreshing_agents_requests_a_reload() {
        // A user who installs an agent after Agency starts and hits refresh
        // gets an updated agent list; without this trigger the slash catalog
        // would stay Agency-only until a worktree switch or restart.
        let mut agency = Agency::for_testing();

        let _ = agency.reduce_event(AppEvent::RefreshAgents);

        assert!(
            agency
                .drain_events()
                .iter()
                .any(|event| matches!(event, AppEvent::SlashCatalogRequested))
        );
    }

    /// `SlashCatalogRequested`'s handler must actually return the
    /// `Task::perform(...)` that runs discovery, not `Task::none()` — a
    /// deleted `return` would leave every other test in this file green.
    /// `Task` does not expose its inner state for a direct equality check,
    /// but `units()` distinguishes "does nothing" (`Task::none()`, 0 units)
    /// from a real unit of work, which is exactly the distinction that
    /// matters here.
    #[test]
    fn slash_catalog_requested_returns_a_real_task_rather_than_none() {
        let mut agency = Agency::for_testing();

        let task = agency.reduce_event(AppEvent::SlashCatalogRequested);

        assert!(task.units() > 0);
    }

    /// The skip matches the resolved path, not the directory name, so a
    /// `worktrees` directory that is part of the project stays visible.
    #[test]
    fn the_explorer_hides_the_worktrees_directory_but_not_others() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "agency-explorer-worktrees-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".agency").join("worktrees").join("feature")).unwrap();
        std::fs::create_dir_all(root.join("docs").join("worktrees")).unwrap();
        let mut expanded = HashSet::new();
        expanded.insert(root.join(".agency"));
        expanded.insert(root.join("docs"));

        let mut entries = Vec::new();
        collect_explorer_entries(&root, &root, 0, &expanded, &mut entries);
        let paths = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();

        assert!(paths.contains(&root.join(".agency")));
        assert!(
            !paths.contains(&root.join(".agency").join("worktrees")),
            "the worktrees directory must not appear in the primary's tree"
        );
        assert!(paths.contains(&root.join("docs").join("worktrees")));

        std::fs::remove_dir_all(root).unwrap();
    }
}
