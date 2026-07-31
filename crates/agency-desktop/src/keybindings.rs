use crate::config::KeybindingConfig;
use iced::keyboard::{
    Key, Modifiers,
    key::{Code, Named, Physical},
};
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerMotion {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBackward,
    WordEnd,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerOperator {
    Delete,
    Change,
    Yank,
}

/// The semantic keybinding context owned by the currently focused UI element.
///
/// Add a variant when introducing a surface with local keybindings, attach it
/// to that surface's [`FocusId`], and dispatch using [`FocusTracker::context`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeybindingContext(pub &'static str);

#[allow(non_upper_case_globals)]
impl KeybindingContext {
    pub const Workspace: Self = Self("workspace");
    pub const Toolbar: Self = Self("toolbar");
    pub const Explorer: Self = Self("explorer");
    pub const DiffActivity: Self = Self("diff-activity");
    pub const DiffViewer: Self = Self("diff-viewer");
    pub const Composer: Self = Self("composer");
    pub const Terminal: Self = Self("terminal");
    pub const Confirmation: Self = Self("confirmation");
    pub const AgentMenu: Self = Self("agent-menu");

    #[cfg(test)]
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }
}

impl Default for KeybindingContext {
    fn default() -> Self {
        Self::Workspace
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DispatchContext {
    pub focused: KeybindingContext,
    pub terminal_available: bool,
    pub composer_available: bool,
}

impl DispatchContext {
    pub fn focused(focused: KeybindingContext) -> Self {
        Self {
            focused,
            terminal_available: false,
            composer_available: false,
        }
    }
}

/// Numeric window identity. Its value is the window's left-to-right position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FocusId(pub usize);

#[derive(Debug, Clone, Copy)]
struct FocusEntry<C> {
    context: C,
    visible: bool,
}

/// Reusable focus harness associating arbitrary UI elements with contexts.
#[derive(Debug, Clone)]
pub struct FocusTracker<C> {
    windows: BTreeMap<FocusId, FocusEntry<C>>,
    focused: FocusId,
}

#[derive(Debug, Clone)]
struct ElementModes<M> {
    allowed: Vec<M>,
}

/// Registry of the global modes in which each focusable tool can bind keys.
/// It deliberately stores no active mode: mode is application-global.
#[derive(Debug, Clone)]
pub struct ElementModeRegistry<M> {
    elements: BTreeMap<FocusId, ElementModes<M>>,
}

impl<M> Default for ElementModeRegistry<M> {
    fn default() -> Self {
        Self {
            elements: BTreeMap::new(),
        }
    }
}

/// Declarative, provider-neutral keymap keyed by app mode, element context,
/// tool-supported mode, and normalized key sequence. Both IDs and actions are
/// caller-defined, so adding a feature never requires changing this harness.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct ContextKeymap<AppMode, Context, ElementMode, Command> {
    bindings: HashMap<(AppMode, Context, Option<ElementMode>, String), Command>,
}

#[cfg(test)]
impl<AppMode, Context, ElementMode, Command> Default
    for ContextKeymap<AppMode, Context, ElementMode, Command>
{
    fn default() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }
}

#[cfg(test)]
impl<AppMode, Context, ElementMode, Command> ContextKeymap<AppMode, Context, ElementMode, Command>
where
    AppMode: Copy + Eq + Hash,
    Context: Copy + Eq + Hash,
    ElementMode: Copy + Eq + Hash,
{
    pub fn bind(
        &mut self,
        app_mode: AppMode,
        context: Context,
        element_mode: Option<ElementMode>,
        sequence: impl Into<String>,
        command: Command,
    ) {
        self.bindings
            .insert((app_mode, context, element_mode, sequence.into()), command);
    }

    pub fn resolve(
        &self,
        app_mode: AppMode,
        context: Context,
        element_mode: Option<ElementMode>,
        sequence: &str,
    ) -> Option<&Command> {
        self.bindings
            .get(&(app_mode, context, element_mode, sequence.to_owned()))
    }
}

impl<M: Copy + PartialEq> ElementModeRegistry<M> {
    pub fn attach(&mut self, element: FocusId, allowed: impl IntoIterator<Item = M>) {
        let allowed = allowed.into_iter().collect::<Vec<_>>();
        assert!(
            !allowed.is_empty(),
            "an element must support at least one mode"
        );
        self.elements.insert(element, ElementModes { allowed });
    }

    pub fn supports(&self, element: FocusId, mode: M) -> bool {
        self.elements
            .get(&element)
            .is_some_and(|element| element.allowed.contains(&mode))
    }

    #[cfg(test)]
    pub fn allows_modes(&self, element: FocusId) -> bool {
        self.elements
            .get(&element)
            .is_some_and(|element| element.allowed.len() > 1)
    }
}

impl<C: Copy> FocusTracker<C> {
    pub fn new(initial: FocusId, context: C) -> Self {
        Self {
            windows: BTreeMap::from([(
                initial,
                FocusEntry {
                    context,
                    visible: true,
                },
            )]),
            focused: initial,
        }
    }

    pub fn attach(&mut self, element: FocusId, context: C) {
        self.windows.insert(
            element,
            FocusEntry {
                context,
                visible: false,
            },
        );
    }

    pub fn set_visible(&mut self, element: FocusId, visible: bool) -> bool {
        let Some(entry) = self.windows.get_mut(&element) else {
            return false;
        };
        entry.visible = visible;
        true
    }

    pub fn focus(&mut self, element: FocusId) -> bool {
        if self
            .windows
            .get(&element)
            .is_some_and(|entry| entry.visible)
        {
            self.focused = element;
            true
        } else {
            false
        }
    }

    pub fn focused(&self) -> FocusId {
        self.focused
    }

    pub fn is_visible(&self, element: FocusId) -> bool {
        self.windows
            .get(&element)
            .is_some_and(|entry| entry.visible)
    }

    pub fn context(&self) -> C {
        self.windows[&self.focused].context
    }

    pub fn focus_right(&mut self) -> FocusId {
        self.cycle(true)
    }

    pub fn focus_left(&mut self) -> FocusId {
        self.cycle(false)
    }

    fn cycle(&mut self, right: bool) -> FocusId {
        let visible = self
            .windows
            .iter()
            .filter_map(|(id, entry)| entry.visible.then_some(*id))
            .collect::<Vec<_>>();
        if let Some(position) = visible.iter().position(|id| *id == self.focused) {
            let next = if right {
                (position + 1) % visible.len()
            } else {
                position.checked_sub(1).unwrap_or(visible.len() - 1)
            };
            self.focused = visible[next];
        } else if let Some(first) = visible.first() {
            self.focused = *first;
        }
        self.focused
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeIndicator {
    Normal,
    Insert,
    Visual,
    Terminal,
    Leader,
}

impl ModeIndicator {
    #[cfg(test)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
            Self::Terminal => "TERMINAL",
            Self::Leader => "LEADER",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Sessions,
    Explorer,
    Mcp,
    Diffs,
    FileViewer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    FocusRight,
    FocusLeft,
    WorktreePrevious,
    WorktreeNext,
    WorktreeSelect(usize),
    ToggleActivity(Activity),
    ToggleSettings,
    NewSession,
    ToolbarPrevious,
    ToolbarNext,
    ToolbarFirst,
    ToolbarLast,
    ToolbarOpen,
    ToolbarTrash,
    ExplorerPrevious,
    ExplorerNext,
    ExplorerCollapse,
    ExplorerExpand,
    ExplorerOpen,
    DiffPrevious,
    DiffNext,
    DiffFirst,
    DiffLast,
    DiffOpen,
    DiffScrollUp,
    DiffScrollDown,
    DiffJumpToTool,
    DiffClose,
    ToggleTerminal,
    ToggleAgentMenu,
    AgentMenuPrevious,
    AgentMenuNext,
    AgentMenuFirst,
    AgentMenuLast,
    AgentMenuConfirm,
    AgentMenuClose,
    EnterComposer,
    EnterTerminal,
    AgentAppend(String),
    AgentBackspace,
    AgentPaste,
    AgentSelectAll,
    AgentMove(ComposerMotion),
    AgentOperate(ComposerOperator, Option<ComposerMotion>),
    AgentOperateSelection(ComposerOperator),
    AgentDeleteChar,
    AgentInsertAtLineStart,
    AgentAppendAtCursor,
    AgentAppendAtLineEnd,
    AgentSubmit,
    TerminalInput(Vec<u8>),
}

pub struct Keybindings {
    config: KeybindingConfig,
    mode: Mode,
    leader_pending: bool,
    leader_separator_pending: bool,
    focus_chord_pending: Option<bool>,
    composer_operator_pending: Option<ComposerOperator>,
    composer_g_pending: bool,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::from_config(KeybindingConfig::default())
    }
}

impl Keybindings {
    pub fn from_config(config: KeybindingConfig) -> Self {
        Self {
            config,
            mode: Mode::Normal,
            leader_pending: false,
            leader_separator_pending: false,
            focus_chord_pending: None,
            composer_operator_pending: None,
            composer_g_pending: false,
        }
    }

    pub fn is_composer_active(&self) -> bool {
        matches!(self.mode, Mode::Insert | Mode::Visual)
    }

    pub fn is_normal(&self) -> bool {
        self.mode == Mode::Normal
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.composer_operator_pending = None;
        self.composer_g_pending = false;
    }

    pub fn display_label(&self) -> &'static str {
        if self.leader_pending {
            return "LEADER";
        }
        match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::Terminal => "TERMINAL",
        }
    }

    pub fn activate_context(&mut self, context: KeybindingContext) {
        self.leader_pending = false;
        self.leader_separator_pending = false;
        if context == KeybindingContext::Terminal {
            self.mode = Mode::Terminal;
        } else if self.mode == Mode::Terminal {
            self.mode = Mode::Normal;
        }
    }

    pub fn is_leader_pending(&self) -> bool {
        self.leader_pending
    }

    pub fn toggle_sessions_hint(&self) -> &str {
        &self.config.toggle_sessions
    }

    pub fn toggle_explorer_hint(&self) -> &str {
        &self.config.toggle_explorer
    }

    pub fn toggle_mcp_hint(&self) -> &str {
        &self.config.toggle_mcp
    }

    pub fn toggle_diffs_hint(&self) -> &str {
        &self.config.toggle_diffs
    }

    pub fn toggle_settings_hint(&self) -> &str {
        &self.config.toggle_settings
    }

    pub fn toggle_terminal_hint(&self) -> &str {
        &self.config.toggle_terminal
    }

    pub fn enter_active_view_hint(&self) -> &str {
        &self.config.enter_active_view
    }

    pub fn toggle_agent_menu_hint(&self) -> &str {
        &self.config.toggle_agent_menu
    }

    pub fn toggle_terminal_mode(&mut self, terminal_visible: bool) {
        self.mode = if terminal_visible {
            Mode::Normal
        } else {
            Mode::Terminal
        };
    }

    #[cfg(test)]
    pub fn mode_label(&self) -> &'static str {
        self.mode_indicator().label()
    }

    pub fn mode_indicator(&self) -> ModeIndicator {
        if self.leader_pending {
            ModeIndicator::Leader
        } else {
            match self.mode {
                Mode::Normal => ModeIndicator::Normal,
                Mode::Insert => ModeIndicator::Insert,
                Mode::Visual => ModeIndicator::Visual,
                Mode::Terminal => ModeIndicator::Terminal,
            }
        }
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn handle(
        &mut self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
        text: Option<&str>,
        terminal_visible: bool,
        agent_visible: bool,
        toolbar_visible: bool,
        explorer_active: bool,
    ) -> Action {
        self.handle_with_diff(
            key,
            physical_key,
            modifiers,
            text,
            terminal_visible,
            agent_visible,
            toolbar_visible,
            explorer_active,
            false,
            false,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn handle_with_diff(
        &mut self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
        text: Option<&str>,
        terminal_visible: bool,
        agent_visible: bool,
        toolbar_visible: bool,
        explorer_active: bool,
        diff_activity_visible: bool,
        diff_viewer_visible: bool,
    ) -> Action {
        let context = if diff_viewer_visible {
            KeybindingContext::DiffViewer
        } else if diff_activity_visible {
            KeybindingContext::DiffActivity
        } else if toolbar_visible && explorer_active {
            KeybindingContext::Explorer
        } else if toolbar_visible {
            KeybindingContext::Toolbar
        } else if agent_visible {
            KeybindingContext::Composer
        } else {
            KeybindingContext::Workspace
        };
        self.handle_in_context(
            key,
            physical_key,
            modifiers,
            text,
            DispatchContext {
                focused: context,
                terminal_available: terminal_visible,
                composer_available: agent_visible,
            },
        )
    }

    pub fn handle_in_context(
        &mut self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
        text: Option<&str>,
        context: DispatchContext,
    ) -> Action {
        if let Some(right) = self.focus_chord_pending.take()
            && (modifiers.is_empty() || modifiers == Modifiers::CTRL)
            && is_character(key, physical_key, "w", Code::KeyW)
        {
            return if right {
                Action::FocusRight
            } else {
                Action::FocusLeft
            };
        }
        if modifiers.control()
            && is_character(key, physical_key, "w", Code::KeyW)
            && modifiers != Modifiers::CTRL
            && modifiers != (Modifiers::CTRL | Modifiers::SHIFT)
        {
            return Action::None;
        }
        if modifiers.control() && is_character(key, physical_key, "w", Code::KeyW) {
            self.focus_chord_pending = Some(!modifiers.shift());
            return Action::None;
        }

        if self.leader_pending {
            return self.handle_leader_suffix(
                key,
                physical_key,
                modifiers,
                context.terminal_available,
                context.composer_available,
            );
        }

        // NORMAL mode commands remain global even while the composer owns
        // focus. The composer has its own NORMAL-mode motions, but the leader
        // must be recognized before dispatching into that local keymap.
        if self.mode == Mode::Normal
            && modifiers.is_empty()
            && configured_character(key, physical_key, &self.config.leader, " ", Code::Space)
        {
            self.leader_pending = true;
            return Action::None;
        }

        match self.mode {
            Mode::Normal | Mode::Visual if context.focused == KeybindingContext::Composer => {
                self.handle_agent(key, physical_key, modifiers, text)
            }
            Mode::Normal | Mode::Visual => {
                self.handle_normal(key, physical_key, modifiers, context)
            }
            Mode::Insert if context.focused == KeybindingContext::Composer => {
                self.handle_agent(key, physical_key, modifiers, text)
            }
            Mode::Insert => Action::None,
            Mode::Terminal => self.handle_terminal(key, physical_key, modifiers, text),
        }
    }

    fn handle_normal(
        &mut self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
        context: DispatchContext,
    ) -> Action {
        let focused = context.focused;
        // The agent menu is a floating popover that owns input while it is
        // open: unhandled keys resolve to nothing rather than falling through
        // to the surface it floats above.
        if focused == KeybindingContext::AgentMenu {
            if modifiers == Modifiers::SHIFT && is_character(key, physical_key, "g", Code::KeyG) {
                return Action::AgentMenuLast;
            }
            if modifiers.is_empty() {
                if matches!(key.as_ref(), Key::Named(Named::ArrowUp))
                    || is_character(key, physical_key, "k", Code::KeyK)
                {
                    return Action::AgentMenuPrevious;
                }
                if matches!(key.as_ref(), Key::Named(Named::ArrowDown))
                    || is_character(key, physical_key, "j", Code::KeyJ)
                {
                    return Action::AgentMenuNext;
                }
                if is_character(key, physical_key, "g", Code::KeyG) {
                    return Action::AgentMenuFirst;
                }
                if matches!(key.as_ref(), Key::Named(Named::Enter)) {
                    return Action::AgentMenuConfirm;
                }
                if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                    return Action::AgentMenuClose;
                }
            }
            return Action::None;
        }

        if focused == KeybindingContext::DiffViewer
            && modifiers == Modifiers::CTRL
            && is_character(key, physical_key, "c", Code::KeyC)
        {
            return Action::DiffClose;
        }

        if focused == KeybindingContext::DiffViewer && modifiers.is_empty() {
            if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                self.mode = Mode::Normal;
                return Action::None;
            }
            if is_character(key, physical_key, "v", Code::KeyV) {
                self.mode = if self.mode == Mode::Visual {
                    Mode::Normal
                } else {
                    Mode::Visual
                };
                return Action::None;
            }
            if is_character(key, physical_key, "i", Code::KeyI) {
                return Action::None;
            }
            if matches!(key.as_ref(), Key::Named(Named::ArrowUp))
                || is_character(key, physical_key, "k", Code::KeyK)
            {
                return Action::DiffScrollUp;
            }
            if matches!(key.as_ref(), Key::Named(Named::ArrowDown))
                || is_character(key, physical_key, "j", Code::KeyJ)
            {
                return Action::DiffScrollDown;
            }
            if matches!(key.as_ref(), Key::Named(Named::Enter)) {
                return Action::DiffJumpToTool;
            }
        }

        if focused == KeybindingContext::DiffActivity {
            if modifiers == Modifiers::SHIFT && is_character(key, physical_key, "g", Code::KeyG) {
                return Action::DiffLast;
            }
            if modifiers.is_empty() {
                if matches!(key.as_ref(), Key::Named(Named::ArrowUp))
                    || is_character(key, physical_key, "k", Code::KeyK)
                {
                    return Action::DiffPrevious;
                }
                if matches!(key.as_ref(), Key::Named(Named::ArrowDown))
                    || is_character(key, physical_key, "j", Code::KeyJ)
                {
                    return Action::DiffNext;
                }
                if is_character(key, physical_key, "g", Code::KeyG) {
                    return Action::DiffFirst;
                }
                if matches!(key.as_ref(), Key::Named(Named::Enter)) {
                    return Action::DiffOpen;
                }
            }
        }

        if focused == KeybindingContext::Toolbar
            && modifiers == Modifiers::SHIFT
            && is_character(key, physical_key, "g", Code::KeyG)
        {
            return Action::ToolbarLast;
        }

        if focused == KeybindingContext::Explorer && modifiers.is_empty() {
            if matches!(key.as_ref(), Key::Named(Named::ArrowUp))
                || is_character(key, physical_key, "k", Code::KeyK)
            {
                return Action::ExplorerPrevious;
            }
            if matches!(key.as_ref(), Key::Named(Named::ArrowDown))
                || is_character(key, physical_key, "j", Code::KeyJ)
            {
                return Action::ExplorerNext;
            }
            if matches!(key.as_ref(), Key::Named(Named::ArrowLeft))
                || is_character(key, physical_key, "h", Code::KeyH)
            {
                return Action::ExplorerCollapse;
            }
            if matches!(key.as_ref(), Key::Named(Named::ArrowRight))
                || is_character(key, physical_key, "l", Code::KeyL)
            {
                return Action::ExplorerExpand;
            }
            if matches!(key.as_ref(), Key::Named(Named::Enter)) {
                return Action::ExplorerOpen;
            }
        }

        if focused == KeybindingContext::Toolbar && modifiers.is_empty() {
            if is_character(key, physical_key, "k", Code::KeyK) {
                return Action::ToolbarPrevious;
            }
            if is_character(key, physical_key, "j", Code::KeyJ) {
                return Action::ToolbarNext;
            }
            if is_character(key, physical_key, "g", Code::KeyG) {
                return Action::ToolbarFirst;
            }
            if matches!(key.as_ref(), Key::Named(Named::Enter)) {
                return Action::ToolbarOpen;
            }
            if is_character(key, physical_key, "d", Code::KeyD) {
                return Action::ToolbarTrash;
            }
        }

        if modifiers.is_empty() {
            if focused != KeybindingContext::Explorer
                && is_character(key, physical_key, "h", Code::KeyH)
            {
                return Action::WorktreePrevious;
            }
            if focused != KeybindingContext::Explorer
                && is_character(key, physical_key, "l", Code::KeyL)
            {
                return Action::WorktreeNext;
            }
        }

        // Entering an insert-like mode is an action, never a bare mode
        // mutation: the mode is only meaningful once the owning element also
        // holds focus, so the application moves focus and sets the mode
        // together.
        if modifiers.is_empty()
            && configured_character(
                key,
                physical_key,
                &self.config.enter_active_view,
                "i",
                Code::KeyI,
            )
        {
            if context.composer_available {
                return Action::EnterComposer;
            }
            if context.terminal_available {
                return Action::EnterTerminal;
            }
        }

        Action::None
    }

    fn handle_leader_suffix(
        &mut self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
        terminal_visible: bool,
        agent_visible: bool,
    ) -> Action {
        if !self.leader_separator_pending
            && modifiers.is_empty()
            && (is_character(key, physical_key, "-", Code::Minus)
                || is_character(key, physical_key, "=", Code::Equal))
        {
            self.leader_separator_pending = true;
            return Action::None;
        }

        self.leader_pending = false;
        self.leader_separator_pending = false;

        if !modifiers.is_empty() {
            return Action::None;
        }
        if let Some(index) = worktree_number(key, physical_key) {
            self.mode = Mode::Normal;
            return Action::WorktreeSelect(index);
        }
        if configured_character(
            key,
            physical_key,
            &self.config.toggle_explorer,
            "e",
            Code::KeyE,
        ) {
            self.mode = Mode::Normal;
            return Action::ToggleActivity(Activity::Explorer);
        }
        if configured_character(
            key,
            physical_key,
            &self.config.toggle_sessions,
            "s",
            Code::KeyS,
        ) {
            self.mode = Mode::Normal;
            return Action::ToggleActivity(Activity::Sessions);
        }
        if configured_character(key, physical_key, &self.config.toggle_mcp, "m", Code::KeyM) {
            self.mode = Mode::Normal;
            return Action::ToggleActivity(Activity::Mcp);
        }
        if configured_character(
            key,
            physical_key,
            &self.config.toggle_file_viewer,
            "d",
            Code::KeyD,
        ) {
            self.mode = Mode::Normal;
            return Action::ToggleActivity(Activity::FileViewer);
        }
        if configured_character(
            key,
            physical_key,
            &self.config.toggle_diffs,
            "f",
            Code::KeyF,
        ) {
            self.mode = Mode::Normal;
            return Action::ToggleActivity(Activity::Diffs);
        }
        if configured_character(
            key,
            physical_key,
            &self.config.toggle_settings,
            ",",
            Code::Comma,
        ) {
            self.mode = Mode::Normal;
            return Action::ToggleSettings;
        }
        if configured_character(key, physical_key, &self.config.new_session, "n", Code::KeyN) {
            self.mode = Mode::Normal;
            return Action::NewSession;
        }
        if configured_character(
            key,
            physical_key,
            &self.config.toggle_agent_menu,
            "a",
            Code::KeyA,
        ) {
            self.mode = Mode::Normal;
            return Action::ToggleAgentMenu;
        }
        if agent_visible
            && configured_character(
                key,
                physical_key,
                &self.config.enter_active_view,
                "i",
                Code::KeyI,
            )
        {
            return Action::EnterComposer;
        }
        if configured_character(
            key,
            physical_key,
            &self.config.toggle_terminal,
            "t",
            Code::KeyT,
        ) {
            self.toggle_terminal_mode(terminal_visible);
            return Action::ToggleTerminal;
        }
        Action::None
    }

    fn handle_terminal(
        &mut self,
        key: &Key,
        _physical_key: Physical,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> Action {
        if matches!(key.as_ref(), Key::Named(Named::Escape)) {
            self.mode = Mode::Normal;
            return Action::None;
        }

        Action::TerminalInput(terminal_bytes(key, modifiers, text))
    }

    fn handle_agent(
        &mut self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> Action {
        if (modifiers.control() || modifiers.logo())
            && is_character(key, physical_key, "v", Code::KeyV)
        {
            return Action::AgentPaste;
        }
        if (modifiers.control() || modifiers.logo())
            && is_character(key, physical_key, "a", Code::KeyA)
        {
            return Action::AgentSelectAll;
        }

        match self.mode {
            Mode::Insert => match key.as_ref() {
                Key::Named(Named::Escape) => {
                    self.mode = Mode::Normal;
                    Action::None
                }
                Key::Named(Named::Enter) if !modifiers.shift() => Action::AgentSubmit,
                Key::Named(Named::Backspace) => Action::AgentBackspace,
                _ if !modifiers.control()
                    && !modifiers.logo()
                    && (text.is_some()
                        || physical_key == Physical::Code(Code::Space)
                        || matches!(key, Key::Character(_))) =>
                {
                    Action::AgentAppend(printable_text(key, physical_key, text))
                }
                _ => Action::None,
            },
            Mode::Normal | Mode::Visual => {
                if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                    self.composer_operator_pending = None;
                    self.composer_g_pending = false;
                    self.mode = Mode::Normal;
                    return Action::None;
                }
                let shifted = modifiers.shift();
                if modifiers.is_empty() && is_character(key, physical_key, "v", Code::KeyV) {
                    self.mode = if self.mode == Mode::Visual {
                        Mode::Normal
                    } else {
                        Mode::Visual
                    };
                    self.composer_operator_pending = None;
                    self.composer_g_pending = false;
                    return Action::None;
                }

                if self.mode == Mode::Normal {
                    if modifiers.is_empty() && is_character(key, physical_key, "i", Code::KeyI) {
                        self.mode = Mode::Insert;
                        return Action::None;
                    }
                    if modifiers.is_empty() && is_character(key, physical_key, "a", Code::KeyA) {
                        self.mode = Mode::Insert;
                        return Action::AgentAppendAtCursor;
                    }
                    if shifted && is_character(key, physical_key, "a", Code::KeyA) {
                        self.mode = Mode::Insert;
                        return Action::AgentAppendAtLineEnd;
                    }
                    if shifted && is_character(key, physical_key, "i", Code::KeyI) {
                        self.mode = Mode::Insert;
                        return Action::AgentInsertAtLineStart;
                    }
                }

                let plain_g =
                    modifiers.is_empty() && is_character(key, physical_key, "g", Code::KeyG);
                if plain_g {
                    if !self.composer_g_pending {
                        self.composer_g_pending = true;
                        return Action::None;
                    }
                    self.composer_g_pending = false;
                    let motion = ComposerMotion::DocumentStart;
                    if let Some(operator) = self.composer_operator_pending.take() {
                        if operator == ComposerOperator::Change {
                            self.mode = Mode::Insert;
                        }
                        return Action::AgentOperate(operator, Some(motion));
                    }
                    return Action::AgentMove(motion);
                }
                self.composer_g_pending = false;

                if self.mode == Mode::Normal {
                    let operator = if is_character(key, physical_key, "d", Code::KeyD) {
                        Some(ComposerOperator::Delete)
                    } else if is_character(key, physical_key, "c", Code::KeyC) {
                        Some(ComposerOperator::Change)
                    } else if is_character(key, physical_key, "y", Code::KeyY) {
                        Some(ComposerOperator::Yank)
                    } else {
                        None
                    };
                    if let Some(operator) = operator {
                        if shifted && operator != ComposerOperator::Yank {
                            self.mode = if operator == ComposerOperator::Change {
                                Mode::Insert
                            } else {
                                Mode::Normal
                            };
                            self.composer_operator_pending = None;
                            return Action::AgentOperate(operator, Some(ComposerMotion::LineEnd));
                        }
                        if self.composer_operator_pending == Some(operator) {
                            self.composer_operator_pending = None;
                            if operator == ComposerOperator::Change {
                                self.mode = Mode::Insert;
                            }
                            return Action::AgentOperate(operator, None);
                        }
                        self.composer_operator_pending = Some(operator);
                        return Action::None;
                    }

                    let motion = composer_motion(key, physical_key, modifiers);
                    if let Some(motion) = motion {
                        if let Some(operator) = self.composer_operator_pending.take() {
                            if operator == ComposerOperator::Change {
                                self.mode = Mode::Insert;
                            }
                            return Action::AgentOperate(operator, Some(motion));
                        }
                        return Action::AgentMove(motion);
                    }
                    self.composer_operator_pending = None;
                    if modifiers.is_empty() && is_character(key, physical_key, "x", Code::KeyX) {
                        return Action::AgentDeleteChar;
                    }
                } else {
                    let operator = if modifiers.is_empty()
                        && is_character(key, physical_key, "d", Code::KeyD)
                        || modifiers.is_empty() && is_character(key, physical_key, "x", Code::KeyX)
                    {
                        Some(ComposerOperator::Delete)
                    } else if modifiers.is_empty()
                        && is_character(key, physical_key, "c", Code::KeyC)
                    {
                        Some(ComposerOperator::Change)
                    } else if modifiers.is_empty()
                        && is_character(key, physical_key, "y", Code::KeyY)
                    {
                        Some(ComposerOperator::Yank)
                    } else {
                        None
                    };
                    if let Some(operator) = operator {
                        self.mode = if operator == ComposerOperator::Change {
                            Mode::Insert
                        } else {
                            Mode::Normal
                        };
                        return Action::AgentOperateSelection(operator);
                    }

                    if let Some(motion) = composer_motion(key, physical_key, modifiers) {
                        return Action::AgentMove(motion);
                    }
                }

                if modifiers.is_empty() && is_character(key, physical_key, "p", Code::KeyP) {
                    if self.mode == Mode::Visual {
                        self.mode = Mode::Normal;
                    }
                    return Action::AgentPaste;
                }
                Action::None
            }
            Mode::Terminal => Action::None,
        }
    }
}

fn composer_motion(
    key: &Key,
    physical_key: Physical,
    modifiers: Modifiers,
) -> Option<ComposerMotion> {
    let plain = !modifiers.control() && !modifiers.logo() && !modifiers.alt();
    plain.then(|| {
        if is_character(key, physical_key, "h", Code::KeyH) {
            Some(ComposerMotion::Left)
        } else if is_character(key, physical_key, "l", Code::KeyL) {
            Some(ComposerMotion::Right)
        } else if is_character(key, physical_key, "k", Code::KeyK) {
            Some(ComposerMotion::Up)
        } else if is_character(key, physical_key, "j", Code::KeyJ) {
            Some(ComposerMotion::Down)
        } else if is_character(key, physical_key, "w", Code::KeyW) {
            Some(ComposerMotion::WordForward)
        } else if is_character(key, physical_key, "b", Code::KeyB) {
            Some(ComposerMotion::WordBackward)
        } else if is_character(key, physical_key, "e", Code::KeyE) {
            Some(ComposerMotion::WordEnd)
        } else if matches!(key.as_ref(), Key::Character(value) if value == "0") {
            Some(ComposerMotion::LineStart)
        } else if matches!(key.as_ref(), Key::Character(value) if value == "$") {
            Some(ComposerMotion::LineEnd)
        } else if modifiers.shift() && is_character(key, physical_key, "g", Code::KeyG) {
            Some(ComposerMotion::DocumentEnd)
        } else {
            None
        }
    })?
}

fn printable_text(key: &Key, physical_key: Physical, text: Option<&str>) -> String {
    if physical_key == Physical::Code(Code::Space) {
        return " ".to_owned();
    }

    text.map(str::to_owned)
        .unwrap_or_else(|| match key.as_ref() {
            Key::Character(value) => value.to_owned(),
            _ => String::new(),
        })
}

fn is_character(key: &Key, physical_key: Physical, character: &str, code: Code) -> bool {
    matches!(key.as_ref(), Key::Character(value) if value.eq_ignore_ascii_case(character))
        || physical_key == Physical::Code(code)
}

fn configured_character(
    key: &Key,
    physical_key: Physical,
    configured: &str,
    default: &str,
    default_code: Code,
) -> bool {
    let configured = if configured.chars().count() == 1 {
        configured
    } else {
        default
    };
    matches!(key.as_ref(), Key::Character(value) if value.eq_ignore_ascii_case(configured))
        || (configured.eq_ignore_ascii_case(default)
            && physical_key == Physical::Code(default_code))
}

fn worktree_number(key: &Key, physical_key: Physical) -> Option<usize> {
    let digit = match key.as_ref() {
        Key::Character(value) => value.parse::<usize>().ok(),
        _ => None,
    }
    .or(match physical_key {
        Physical::Code(code) => match code {
            Code::Digit0 => Some(0),
            Code::Digit1 => Some(1),
            Code::Digit2 => Some(2),
            Code::Digit3 => Some(3),
            Code::Digit4 => Some(4),
            Code::Digit5 => Some(5),
            Code::Digit6 => Some(6),
            Code::Digit7 => Some(7),
            Code::Digit8 => Some(8),
            Code::Digit9 => Some(9),
            _ => None,
        },
        _ => None,
    })?;
    Some(if digit == 0 { 9 } else { digit - 1 })
}

fn terminal_bytes(key: &Key, modifiers: Modifiers, text: Option<&str>) -> Vec<u8> {
    match key.as_ref() {
        Key::Named(Named::Enter) => b"\r".to_vec(),
        Key::Named(Named::Backspace) => vec![0x7f],
        Key::Named(Named::Tab) => b"\t".to_vec(),
        Key::Named(Named::Escape) => vec![0x1b],
        Key::Named(Named::ArrowUp) => b"\x1b[A".to_vec(),
        Key::Named(Named::ArrowDown) => b"\x1b[B".to_vec(),
        Key::Named(Named::ArrowRight) => b"\x1b[C".to_vec(),
        Key::Named(Named::ArrowLeft) => b"\x1b[D".to_vec(),
        Key::Character(character) if modifiers.control() => character
            .chars()
            .next()
            .and_then(control_byte)
            .into_iter()
            .collect(),
        Key::Character(_) if !modifiers.logo() => {
            text.map_or_else(Vec::new, |value| value.as_bytes().to_vec())
        }
        _ => Vec::new(),
    }
}

fn control_byte(character: char) -> Option<u8> {
    let character = character.to_ascii_lowercase();

    if character.is_ascii_lowercase() {
        Some(character as u8 - b'a' + 1)
    } else {
        match character {
            '[' => Some(0x1b),
            '\\' => Some(0x1c),
            ']' => Some(0x1d),
            '^' => Some(0x1e),
            '_' => Some(0x1f),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_tracker_attaches_contexts_and_rejects_unknown_focus() {
        let workspace = FocusId(1);
        let explorer = FocusId(0);
        let unknown = FocusId(99);
        let mut focus = FocusTracker::new(workspace, KeybindingContext::Workspace);
        focus.attach(explorer, KeybindingContext::Explorer);
        focus.set_visible(explorer, true);

        assert!(focus.focus(explorer));
        assert_eq!(focus.focused(), explorer);
        assert_eq!(focus.context(), KeybindingContext::Explorer);
        assert!(!focus.focus(unknown));
        assert_eq!(focus.focused(), explorer);
    }

    #[test]
    fn focus_tracker_cycles_visible_windows_in_numeric_order() {
        let mut focus = FocusTracker::new(FocusId(1), KeybindingContext::Workspace);
        focus.attach(FocusId(0), KeybindingContext::Explorer);
        focus.attach(FocusId(2), KeybindingContext::DiffViewer);
        focus.attach(FocusId(3), KeybindingContext::DiffActivity);
        focus.set_visible(FocusId(0), true);
        focus.set_visible(FocusId(2), true);

        assert_eq!(focus.focus_right(), FocusId(2));
        assert_eq!(focus.focus_right(), FocusId(0));
        assert_eq!(focus.focus_left(), FocusId(2));
    }

    #[test]
    fn arbitrary_features_can_register_contexts_modes_and_commands() {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        enum CanvasMode {
            Pan,
            Draw,
        }
        #[derive(Debug, PartialEq, Eq)]
        enum CanvasCommand {
            Stroke,
        }

        let canvas = FocusId(42);
        let context = KeybindingContext::new("plugin.canvas");
        let focus = FocusTracker::new(canvas, context);
        let mut modes = ElementModeRegistry::default();
        modes.attach(canvas, [CanvasMode::Pan, CanvasMode::Draw]);
        let mut keymap = ContextKeymap::default();
        keymap.bind(
            Mode::Normal,
            context,
            Some(CanvasMode::Draw),
            "mouse1",
            CanvasCommand::Stroke,
        );

        assert_eq!(focus.context(), context);
        assert!(modes.allows_modes(canvas));
        assert!(modes.supports(canvas, CanvasMode::Draw));
        assert_eq!(
            keymap.resolve(Mode::Normal, context, Some(CanvasMode::Draw), "mouse1"),
            Some(&CanvasCommand::Stroke)
        );
    }

    #[test]
    fn focus_chords_are_global_and_directional() {
        let context = DispatchContext::focused(KeybindingContext::Composer);
        let mut bindings = Keybindings::default();
        let ctrl_w = parse_vscode_key("ctrl+w");
        let ctrl_shift_w = parse_vscode_key("ctrl+shift+w");
        let w = parse_vscode_key("w");

        assert_eq!(
            bindings.handle_in_context(
                &ctrl_w.key,
                ctrl_w.physical,
                ctrl_w.modifiers,
                ctrl_w.text.as_deref(),
                context,
            ),
            Action::None
        );
        assert_eq!(
            bindings
                .handle_in_context(&w.key, w.physical, w.modifiers, w.text.as_deref(), context,),
            Action::FocusRight
        );
        assert_eq!(
            bindings.handle_in_context(
                &ctrl_shift_w.key,
                ctrl_shift_w.physical,
                ctrl_shift_w.modifiers,
                ctrl_shift_w.text.as_deref(),
                context,
            ),
            Action::None
        );
        assert_eq!(
            bindings
                .handle_in_context(&w.key, w.physical, w.modifiers, w.text.as_deref(), context,),
            Action::FocusLeft
        );
        assert_eq!(
            bindings.handle_in_context(
                &ctrl_w.key,
                ctrl_w.physical,
                ctrl_w.modifiers,
                ctrl_w.text.as_deref(),
                context,
            ),
            Action::None
        );
        assert_eq!(
            bindings.handle_in_context(
                &ctrl_w.key,
                ctrl_w.physical,
                ctrl_w.modifiers,
                ctrl_w.text.as_deref(),
                context,
            ),
            Action::FocusRight
        );
    }

    #[test]
    fn focused_tool_selects_bindings_within_the_global_mode() {
        let mut bindings = Keybindings {
            mode: Mode::Insert,
            ..Keybindings::default()
        };
        let action = bindings.handle_in_context(
            &Key::Character("j".into()),
            Physical::Code(Code::KeyJ),
            Modifiers::empty(),
            Some("j"),
            DispatchContext::focused(KeybindingContext::Explorer),
        );

        assert_eq!(action, Action::None);
    }

    #[derive(Clone, Copy, Default)]
    struct Context {
        terminal_visible: bool,
        agent_visible: bool,
        toolbar_visible: bool,
        explorer_active: bool,
        diff_activity_visible: bool,
        diff_viewer_visible: bool,
    }

    struct KeybindingHarness {
        bindings: Keybindings,
        context: Context,
    }

    impl KeybindingHarness {
        fn for_mode(mode: Mode, context: Context) -> Self {
            Self {
                bindings: Keybindings {
                    mode,
                    ..Keybindings::default()
                },
                context,
            }
        }

        fn dispatch(&mut self, binding: &str) -> Action {
            binding
                .split_ascii_whitespace()
                .map(parse_vscode_key)
                .map(|stroke| {
                    self.bindings.handle_with_diff(
                        &stroke.key,
                        stroke.physical,
                        stroke.modifiers,
                        stroke.text.as_deref(),
                        self.context.terminal_visible,
                        self.context.agent_visible,
                        self.context.toolbar_visible,
                        self.context.explorer_active,
                        self.context.diff_activity_visible,
                        self.context.diff_viewer_visible,
                    )
                })
                .last()
                .expect("a keybinding must contain at least one key")
        }

        fn assert(&mut self, binding: &str, expected: Action, mode: ModeIndicator) {
            assert_eq!(
                self.dispatch(binding),
                expected,
                "unexpected action for `{binding}`"
            );
            assert_eq!(
                self.bindings.mode_indicator(),
                mode,
                "unexpected mode after `{binding}`"
            );
        }
    }

    fn assert_mode_bindings(
        mode: Mode,
        context: Context,
        cases: impl IntoIterator<Item = (&'static str, Action, ModeIndicator)>,
    ) {
        for (binding, expected_action, expected_mode) in cases {
            KeybindingHarness::for_mode(mode, context).assert(
                binding,
                expected_action,
                expected_mode,
            );
        }
    }

    struct Stroke {
        key: Key,
        physical: Physical,
        modifiers: Modifiers,
        text: Option<String>,
    }

    fn parse_vscode_key(binding: &str) -> Stroke {
        let parts = binding.split('+').collect::<Vec<_>>();
        let key_name = parts.last().expect("a key stroke must have a key");
        let mut modifiers = Modifiers::empty();
        for modifier in &parts[..parts.len() - 1] {
            modifiers |= match *modifier {
                "ctrl" => Modifiers::CTRL,
                "shift" => Modifiers::SHIFT,
                "alt" => Modifiers::ALT,
                "meta" | "cmd" => Modifiers::LOGO,
                unknown => panic!("unsupported VS Code modifier `{unknown}`"),
            };
        }

        let (key, code, text) = match *key_name {
            "space" => (
                Key::Character(" ".into()),
                Code::Space,
                Some(" ".to_owned()),
            ),
            "enter" => (Key::Named(Named::Enter), Code::Enter, None),
            "backspace" => (Key::Named(Named::Backspace), Code::Backspace, None),
            "tab" => (Key::Named(Named::Tab), Code::Tab, None),
            "escape" => (Key::Named(Named::Escape), Code::Escape, None),
            "up" => (Key::Named(Named::ArrowUp), Code::ArrowUp, None),
            "down" => (Key::Named(Named::ArrowDown), Code::ArrowDown, None),
            "left" => (Key::Named(Named::ArrowLeft), Code::ArrowLeft, None),
            "right" => (Key::Named(Named::ArrowRight), Code::ArrowRight, None),
            "\\" => (
                Key::Character("\\".into()),
                Code::Backslash,
                Some("\\".to_owned()),
            ),
            "-" => (
                Key::Character("-".into()),
                Code::Minus,
                Some("-".to_owned()),
            ),
            "=" => (
                Key::Character("=".into()),
                Code::Equal,
                Some("=".to_owned()),
            ),
            "$" => (
                Key::Character("$".into()),
                Code::Digit4,
                Some("$".to_owned()),
            ),
            digit if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => {
                let code = match digit {
                    "0" => Code::Digit0,
                    "1" => Code::Digit1,
                    "2" => Code::Digit2,
                    "3" => Code::Digit3,
                    "4" => Code::Digit4,
                    "5" => Code::Digit5,
                    "6" => Code::Digit6,
                    "7" => Code::Digit7,
                    "8" => Code::Digit8,
                    "9" => Code::Digit9,
                    _ => unreachable!(),
                };
                (Key::Character(digit.into()), code, Some(digit.to_owned()))
            }
            character if character.len() == 1 => {
                let character = character.chars().next().unwrap();
                let code = letter_code(character)
                    .unwrap_or_else(|| panic!("unsupported VS Code key `{character}`"));
                let text = if modifiers.control() || modifiers.logo() {
                    None
                } else if modifiers.shift() {
                    Some(character.to_ascii_uppercase().to_string())
                } else {
                    Some(character.to_string())
                };
                (Key::Character(character.to_string().into()), code, text)
            }
            unknown => panic!("unsupported VS Code key `{unknown}`"),
        };

        Stroke {
            key,
            physical: Physical::Code(code),
            modifiers,
            text,
        }
    }

    fn letter_code(character: char) -> Option<Code> {
        Some(match character.to_ascii_lowercase() {
            'a' => Code::KeyA,
            'b' => Code::KeyB,
            'c' => Code::KeyC,
            'd' => Code::KeyD,
            'e' => Code::KeyE,
            'f' => Code::KeyF,
            'g' => Code::KeyG,
            'h' => Code::KeyH,
            'i' => Code::KeyI,
            'j' => Code::KeyJ,
            'k' => Code::KeyK,
            'l' => Code::KeyL,
            'm' => Code::KeyM,
            'n' => Code::KeyN,
            'o' => Code::KeyO,
            'p' => Code::KeyP,
            'q' => Code::KeyQ,
            'r' => Code::KeyR,
            's' => Code::KeyS,
            't' => Code::KeyT,
            'u' => Code::KeyU,
            'v' => Code::KeyV,
            'w' => Code::KeyW,
            'x' => Code::KeyX,
            'y' => Code::KeyY,
            'z' => Code::KeyZ,
            ',' => Code::Comma,
            _ => return None,
        })
    }

    #[test]
    fn normal_mode_vscode_keybindings_map_to_actions() {
        let normal = ModeIndicator::Normal;
        let composer = ModeIndicator::Insert;
        let terminal = ModeIndicator::Terminal;

        assert_mode_bindings(
            Mode::Normal,
            Context::default(),
            [
                ("h", Action::WorktreePrevious, normal),
                ("l", Action::WorktreeNext, normal),
                ("1", Action::None, normal),
                ("space 1", Action::WorktreeSelect(0), normal),
                ("space 9", Action::WorktreeSelect(8), normal),
                ("space 0", Action::WorktreeSelect(9), normal),
                (
                    "space e",
                    Action::ToggleActivity(Activity::Explorer),
                    normal,
                ),
                (
                    "space s",
                    Action::ToggleActivity(Activity::Sessions),
                    normal,
                ),
                ("space m", Action::ToggleActivity(Activity::Mcp), normal),
                (
                    "space d",
                    Action::ToggleActivity(Activity::FileViewer),
                    normal,
                ),
                ("space f", Action::ToggleActivity(Activity::Diffs), normal),
                ("space ,", Action::ToggleSettings, normal),
                ("space n", Action::NewSession, normal),
                ("space t", Action::ToggleTerminal, terminal),
            ],
        );

        let sessions = Context {
            toolbar_visible: true,
            ..Context::default()
        };
        assert_mode_bindings(
            Mode::Normal,
            sessions,
            [
                ("k", Action::ToolbarPrevious, normal),
                ("j", Action::ToolbarNext, normal),
                ("g", Action::ToolbarFirst, normal),
                ("shift+g", Action::ToolbarLast, normal),
                ("enter", Action::ToolbarOpen, normal),
                ("d", Action::ToolbarTrash, normal),
            ],
        );

        let explorer = Context {
            toolbar_visible: true,
            explorer_active: true,
            ..Context::default()
        };
        assert_mode_bindings(
            Mode::Normal,
            explorer,
            [
                ("k", Action::ExplorerPrevious, normal),
                ("up", Action::ExplorerPrevious, normal),
                ("j", Action::ExplorerNext, normal),
                ("down", Action::ExplorerNext, normal),
                ("h", Action::ExplorerCollapse, normal),
                ("left", Action::ExplorerCollapse, normal),
                ("l", Action::ExplorerExpand, normal),
                ("right", Action::ExplorerExpand, normal),
                ("enter", Action::ExplorerOpen, normal),
            ],
        );

        let diffs = Context {
            diff_activity_visible: true,
            ..Context::default()
        };
        assert_mode_bindings(
            Mode::Normal,
            diffs,
            [
                ("k", Action::DiffPrevious, normal),
                ("j", Action::DiffNext, normal),
                ("g", Action::DiffFirst, normal),
                ("shift+g", Action::DiffLast, normal),
                ("enter", Action::DiffOpen, normal),
            ],
        );

        let diff_viewer = Context {
            diff_activity_visible: true,
            diff_viewer_visible: true,
            ..Context::default()
        };
        assert_mode_bindings(
            Mode::Normal,
            diff_viewer,
            [
                ("k", Action::DiffScrollUp, normal),
                ("j", Action::DiffScrollDown, normal),
                ("enter", Action::DiffJumpToTool, normal),
                ("ctrl+c", Action::DiffClose, normal),
            ],
        );

        let agent = Context {
            agent_visible: true,
            ..Context::default()
        };
        assert_mode_bindings(Mode::Normal, agent, [("i", Action::None, composer)]);

        let terminal_context = Context {
            terminal_visible: true,
            ..Context::default()
        };
        // TERMINAL is entered by the application once the terminal window has
        // focus, so the binding itself stays in NORMAL and only names the
        // action.
        assert_mode_bindings(
            Mode::Normal,
            terminal_context,
            [("i", Action::EnterTerminal, normal)],
        );
        assert_mode_bindings(
            Mode::Terminal,
            terminal_context,
            [("i", Action::TerminalInput(b"i".to_vec()), terminal)],
        );
    }

    #[test]
    fn composer_mode_vscode_keybindings_map_to_actions() {
        let composer = ModeIndicator::Insert;
        let normal = ModeIndicator::Normal;
        let context = Context {
            agent_visible: true,
            ..Context::default()
        };

        assert_mode_bindings(
            Mode::Insert,
            context,
            [
                ("x", Action::AgentAppend("x".to_owned()), composer),
                ("space", Action::AgentAppend(" ".to_owned()), composer),
                ("backspace", Action::AgentBackspace, composer),
                ("ctrl+v", Action::AgentPaste, composer),
                ("cmd+v", Action::AgentPaste, composer),
                ("ctrl+a", Action::AgentSelectAll, composer),
                ("cmd+a", Action::AgentSelectAll, composer),
                ("enter", Action::AgentSubmit, composer),
                ("escape", Action::None, normal),
            ],
        );

        KeybindingHarness::for_mode(Mode::Insert, context).assert(
            "escape escape",
            Action::None,
            normal,
        );
    }

    #[test]
    fn diff_viewer_supports_normal_and_visual_modes_without_insert() {
        let context = Context {
            agent_visible: true,
            diff_activity_visible: true,
            diff_viewer_visible: true,
            ..Context::default()
        };
        let mut harness = KeybindingHarness::for_mode(Mode::Normal, context);

        assert_eq!(harness.bindings.mode(), Mode::Normal);
        assert_eq!(harness.dispatch("v"), Action::None);
        assert_eq!(harness.bindings.mode(), Mode::Visual);
        assert_eq!(harness.dispatch("j"), Action::DiffScrollDown);
        assert_eq!(harness.dispatch("i"), Action::None);
        assert!(!harness.bindings.is_normal());
        assert_eq!(harness.bindings.mode(), Mode::Visual);
        assert_eq!(harness.dispatch("escape"), Action::None);
        assert_eq!(harness.bindings.mode(), Mode::Normal);
    }

    #[test]
    fn composer_normal_mode_supports_vim_motions_and_operators() {
        let context = Context {
            agent_visible: true,
            ..Context::default()
        };
        let cases = [
            ("h", Action::AgentMove(ComposerMotion::Left)),
            ("j", Action::AgentMove(ComposerMotion::Down)),
            ("k", Action::AgentMove(ComposerMotion::Up)),
            ("l", Action::AgentMove(ComposerMotion::Right)),
            ("w", Action::AgentMove(ComposerMotion::WordForward)),
            ("b", Action::AgentMove(ComposerMotion::WordBackward)),
            ("e", Action::AgentMove(ComposerMotion::WordEnd)),
            ("0", Action::AgentMove(ComposerMotion::LineStart)),
            ("shift+$", Action::AgentMove(ComposerMotion::LineEnd)),
            ("g g", Action::AgentMove(ComposerMotion::DocumentStart)),
            ("shift+g", Action::AgentMove(ComposerMotion::DocumentEnd)),
            ("d d", Action::AgentOperate(ComposerOperator::Delete, None)),
            (
                "d w",
                Action::AgentOperate(ComposerOperator::Delete, Some(ComposerMotion::WordForward)),
            ),
            ("c c", Action::AgentOperate(ComposerOperator::Change, None)),
            ("y y", Action::AgentOperate(ComposerOperator::Yank, None)),
            ("x", Action::AgentDeleteChar),
            ("p", Action::AgentPaste),
        ];

        for (binding, expected) in cases {
            let mut harness = KeybindingHarness::for_mode(Mode::Normal, context);
            assert_eq!(harness.dispatch(binding), expected, "binding `{binding}`");
        }
    }

    #[test]
    fn composer_visual_mode_supports_vim_motions_and_selection_operators() {
        let context = Context {
            agent_visible: true,
            ..Context::default()
        };
        let cases = [
            ("v h", Action::AgentMove(ComposerMotion::Left)),
            ("v j", Action::AgentMove(ComposerMotion::Down)),
            ("v k", Action::AgentMove(ComposerMotion::Up)),
            ("v l", Action::AgentMove(ComposerMotion::Right)),
            ("v w", Action::AgentMove(ComposerMotion::WordForward)),
            ("v b", Action::AgentMove(ComposerMotion::WordBackward)),
            ("v e", Action::AgentMove(ComposerMotion::WordEnd)),
            ("v 0", Action::AgentMove(ComposerMotion::LineStart)),
            ("v shift+$", Action::AgentMove(ComposerMotion::LineEnd)),
            ("v g g", Action::AgentMove(ComposerMotion::DocumentStart)),
            ("v shift+g", Action::AgentMove(ComposerMotion::DocumentEnd)),
            (
                "v d",
                Action::AgentOperateSelection(ComposerOperator::Delete),
            ),
            (
                "v x",
                Action::AgentOperateSelection(ComposerOperator::Delete),
            ),
            (
                "v c",
                Action::AgentOperateSelection(ComposerOperator::Change),
            ),
            ("v y", Action::AgentOperateSelection(ComposerOperator::Yank)),
            ("v p", Action::AgentPaste),
        ];

        for (binding, expected) in cases {
            let mut harness = KeybindingHarness::for_mode(Mode::Normal, context);
            assert_eq!(harness.dispatch(binding), expected, "binding `{binding}`");
        }
    }

    #[test]
    fn terminal_mode_vscode_keybindings_map_to_actions() {
        let terminal = ModeIndicator::Terminal;
        let normal = ModeIndicator::Normal;
        let context = Context {
            terminal_visible: true,
            ..Context::default()
        };

        assert_mode_bindings(
            Mode::Terminal,
            context,
            [
                ("x", Action::TerminalInput(b"x".to_vec()), terminal),
                ("ctrl+c", Action::TerminalInput(vec![0x03]), terminal),
                ("enter", Action::TerminalInput(b"\r".to_vec()), terminal),
                ("backspace", Action::TerminalInput(vec![0x7f]), terminal),
                ("tab", Action::TerminalInput(b"\t".to_vec()), terminal),
                ("up", Action::TerminalInput(b"\x1b[A".to_vec()), terminal),
                ("down", Action::TerminalInput(b"\x1b[B".to_vec()), terminal),
                ("right", Action::TerminalInput(b"\x1b[C".to_vec()), terminal),
                ("left", Action::TerminalInput(b"\x1b[D".to_vec()), terminal),
                ("ctrl+\\", Action::TerminalInput(vec![0x1c]), terminal),
                ("ctrl+n", Action::TerminalInput(vec![0x0e]), terminal),
                ("escape", Action::None, normal),
            ],
        );
    }

    fn press(
        bindings: &mut Keybindings,
        character: &str,
        code: Code,
        modifiers: Modifiers,
        terminal_open: bool,
    ) -> Action {
        let focused = if matches!(bindings.mode(), Mode::Insert | Mode::Visual) {
            KeybindingContext::Composer
        } else {
            KeybindingContext::Workspace
        };
        bindings.handle_in_context(
            &Key::Character(character.into()),
            Physical::Code(code),
            modifiers,
            Some(character),
            DispatchContext {
                focused,
                terminal_available: terminal_open,
                composer_available: false,
            },
        )
    }

    #[test]
    fn leader_t_opens_terminal_and_enters_terminal_mode() {
        let mut bindings = Keybindings::default();

        assert!(matches!(
            press(&mut bindings, " ", Code::Space, Modifiers::empty(), false),
            Action::None
        ));
        assert_eq!(bindings.mode_label(), "LEADER");

        assert!(matches!(
            press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), false),
            Action::ToggleTerminal
        ));
        assert_eq!(bindings.mode_label(), "TERMINAL");
    }

    #[test]
    fn global_config_can_remap_leader_commands() {
        let config = KeybindingConfig {
            leader: ",".to_owned(),
            toggle_terminal: "x".to_owned(),
            ..KeybindingConfig::default()
        };
        let mut bindings = Keybindings::from_config(config);

        assert!(matches!(
            press(&mut bindings, ",", Code::Comma, Modifiers::empty(), false),
            Action::None
        ));
        assert!(matches!(
            press(&mut bindings, "x", Code::KeyX, Modifiers::empty(), false),
            Action::ToggleTerminal
        ));
    }

    #[test]
    fn leader_accepts_an_optional_separator() {
        let mut bindings = Keybindings::default();

        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        press(&mut bindings, "-", Code::Minus, Modifiers::empty(), false);

        assert!(matches!(
            press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), false),
            Action::ToggleTerminal
        ));
    }

    #[test]
    fn leader_t_hides_a_visible_terminal() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), true);

        assert!(matches!(
            press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), true),
            Action::ToggleTerminal
        ));
        assert_eq!(bindings.mode_label(), "NORMAL");
    }

    #[test]
    fn leader_n_starts_a_provider_independent_session() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        assert!(matches!(
            press(&mut bindings, "n", Code::KeyN, Modifiers::empty(), false),
            Action::NewSession
        ));
        // The new session's composer is not focused yet, so the mode may not
        // run ahead of focus: INSERT without the composer focused would swallow
        // every subsequent key.
        assert_eq!(bindings.mode_label(), "NORMAL");
    }

    #[test]
    fn leader_i_enters_a_visible_agent_composer() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);

        let action = bindings.handle(
            &Key::Character("i".into()),
            Physical::Code(Code::KeyI),
            Modifiers::empty(),
            Some("i"),
            false,
            true,
            false,
            false,
        );

        assert!(matches!(action, Action::EnterComposer));
        assert_eq!(bindings.mode_label(), "NORMAL");
    }

    #[test]
    fn entering_an_insert_like_mode_is_always_an_action() {
        // Every key that would put Agency into an insert-like mode must ask the
        // application to focus the owning surface instead of mutating the mode
        // behind focus's back.
        for (composer_available, terminal_available, expected) in [
            (true, false, Action::EnterComposer),
            (false, true, Action::EnterTerminal),
            (true, true, Action::EnterComposer),
            (false, false, Action::None),
        ] {
            let mut bindings = Keybindings::default();
            let action = bindings.handle_in_context(
                &Key::Character("i".into()),
                Physical::Code(Code::KeyI),
                Modifiers::empty(),
                Some("i"),
                DispatchContext {
                    focused: KeybindingContext::Toolbar,
                    terminal_available,
                    composer_available,
                },
            );

            assert_eq!(action, expected);
            assert_eq!(
                bindings.mode_label(),
                "NORMAL",
                "mode changed before focus moved"
            );
        }
    }

    #[test]
    fn agent_composer_accepts_input_while_explorer_is_open() {
        let mut bindings = Keybindings {
            mode: Mode::Insert,
            ..Keybindings::default()
        };

        let action = bindings.handle_in_context(
            &Key::Character("h".into()),
            Physical::Code(Code::KeyH),
            Modifiers::empty(),
            Some("h"),
            DispatchContext::focused(KeybindingContext::Composer),
        );

        assert!(matches!(action, Action::AgentAppend(text) if text == "h"));
        assert_eq!(bindings.mode_label(), "INSERT");
    }

    #[test]
    fn leader_shortcuts_open_explorer_sessions_and_mcp() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        assert!(matches!(
            press(&mut bindings, "e", Code::KeyE, Modifiers::empty(), false),
            Action::ToggleActivity(Activity::Explorer)
        ));

        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        assert!(matches!(
            press(&mut bindings, "s", Code::KeyS, Modifiers::empty(), false),
            Action::ToggleActivity(Activity::Sessions)
        ));

        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        assert!(matches!(
            press(&mut bindings, "m", Code::KeyM, Modifiers::empty(), false),
            Action::ToggleActivity(Activity::Mcp)
        ));
    }

    #[test]
    fn leader_a_toggles_the_agent_menu() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);

        assert!(matches!(
            press(&mut bindings, "a", Code::KeyA, Modifiers::empty(), false),
            Action::ToggleAgentMenu
        ));
        assert_eq!(bindings.mode_label(), "NORMAL");
    }

    #[test]
    fn global_config_can_remap_the_agent_menu() {
        let config = KeybindingConfig {
            toggle_agent_menu: "p".to_owned(),
            ..KeybindingConfig::default()
        };
        let mut bindings = Keybindings::from_config(config);
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);

        assert!(matches!(
            press(&mut bindings, "p", Code::KeyP, Modifiers::empty(), false),
            Action::ToggleAgentMenu
        ));
    }

    #[test]
    fn the_agent_menu_navigates_with_vim_keys() {
        let context = DispatchContext::focused(KeybindingContext::AgentMenu);
        let mut bindings = Keybindings::default();
        let cases = [
            ("k", Action::AgentMenuPrevious),
            ("j", Action::AgentMenuNext),
            ("g", Action::AgentMenuFirst),
            ("enter", Action::AgentMenuConfirm),
            ("escape", Action::AgentMenuClose),
            ("shift+g", Action::AgentMenuLast),
            ("up", Action::AgentMenuPrevious),
            ("down", Action::AgentMenuNext),
        ];

        for (binding, expected) in cases {
            let stroke = parse_vscode_key(binding);
            assert_eq!(
                bindings.handle_in_context(
                    &stroke.key,
                    stroke.physical,
                    stroke.modifiers,
                    stroke.text.as_deref(),
                    context,
                ),
                expected,
                "`{binding}` should resolve in the agent menu"
            );
        }
    }

    /// The menu floats above another surface, so keys it does not bind must not
    /// leak into the surface behind it.
    #[test]
    fn the_agent_menu_owns_input_while_it_is_open() {
        let context = DispatchContext::focused(KeybindingContext::AgentMenu);
        let mut bindings = Keybindings::default();

        for binding in ["h", "l", "i", "d"] {
            let stroke = parse_vscode_key(binding);
            assert_eq!(
                bindings.handle_in_context(
                    &stroke.key,
                    stroke.physical,
                    stroke.modifiers,
                    stroke.text.as_deref(),
                    context,
                ),
                Action::None,
                "`{binding}` should not reach the surface behind the agent menu"
            );
        }
    }

    /// The leader is resolved before the focused surface, so an open menu can
    /// always be dismissed with the same sequence that opened it.
    #[test]
    fn the_leader_still_reaches_an_open_agent_menu() {
        let context = DispatchContext::focused(KeybindingContext::AgentMenu);
        let mut bindings = Keybindings::default();
        let space = parse_vscode_key("space");
        let a = parse_vscode_key("a");

        assert_eq!(
            bindings.handle_in_context(
                &space.key,
                space.physical,
                space.modifiers,
                space.text.as_deref(),
                context,
            ),
            Action::None
        );
        assert_eq!(
            bindings.handle_in_context(&a.key, a.physical, a.modifiers, a.text.as_deref(), context),
            Action::ToggleAgentMenu
        );
    }

    #[test]
    fn normal_mode_navigates_an_open_toolbar_with_vim_keys() {
        let mut bindings = Keybindings::default();

        assert!(matches!(
            bindings.handle(
                &Key::Character("j".into()),
                Physical::Code(Code::KeyJ),
                Modifiers::empty(),
                Some("j"),
                false,
                false,
                true,
                false,
            ),
            Action::ToolbarNext
        ));
        assert!(matches!(
            bindings.handle(
                &Key::Character("G".into()),
                Physical::Code(Code::KeyG),
                Modifiers::SHIFT,
                Some("G"),
                false,
                false,
                true,
                false,
            ),
            Action::ToolbarLast
        ));
        assert!(matches!(
            bindings.handle(
                &Key::Named(Named::Enter),
                Physical::Code(Code::Enter),
                Modifiers::empty(),
                None,
                false,
                false,
                true,
                false,
            ),
            Action::ToolbarOpen
        ));
        assert!(matches!(
            bindings.handle(
                &Key::Character("d".into()),
                Physical::Code(Code::KeyD),
                Modifiers::empty(),
                Some("d"),
                false,
                false,
                true,
                false,
            ),
            Action::ToolbarTrash
        ));
    }

    #[test]
    fn explorer_accepts_arrow_and_vim_navigation() {
        let mut bindings = Keybindings::default();
        let cases = [
            (
                Key::Named(Named::ArrowUp),
                Physical::Code(Code::ArrowUp),
                Action::ExplorerPrevious,
            ),
            (
                Key::Character("j".into()),
                Physical::Code(Code::KeyJ),
                Action::ExplorerNext,
            ),
            (
                Key::Named(Named::ArrowLeft),
                Physical::Code(Code::ArrowLeft),
                Action::ExplorerCollapse,
            ),
            (
                Key::Character("l".into()),
                Physical::Code(Code::KeyL),
                Action::ExplorerExpand,
            ),
            (
                Key::Named(Named::Enter),
                Physical::Code(Code::Enter),
                Action::ExplorerOpen,
            ),
        ];

        for (key, physical, expected) in cases {
            let action = bindings.handle(
                &key,
                physical,
                Modifiers::empty(),
                None,
                false,
                false,
                true,
                true,
            );
            assert_eq!(
                std::mem::discriminant(&action),
                std::mem::discriminant(&expected)
            );
        }
    }

    #[test]
    fn explorer_does_not_receive_composer_bindings() {
        let config = KeybindingConfig {
            leader: ",".to_owned(),
            ..KeybindingConfig::default()
        };
        let mut bindings = Keybindings {
            mode: Mode::Insert,
            ..Keybindings::from_config(config)
        };

        let explorer = DispatchContext::focused(KeybindingContext::Explorer);
        assert_eq!(
            bindings.handle_in_context(
                &Key::Character(",".into()),
                Physical::Code(Code::Comma),
                Modifiers::empty(),
                Some(","),
                explorer,
            ),
            Action::None
        );
        assert_eq!(bindings.mode(), Mode::Insert);
        bindings.set_mode(Mode::Normal);

        assert!(matches!(
            bindings.handle_in_context(
                &Key::Character(",".into()),
                Physical::Code(Code::Comma),
                Modifiers::empty(),
                Some(","),
                explorer,
            ),
            Action::None
        ));
        assert_eq!(bindings.mode_label(), "LEADER");

        assert!(matches!(
            bindings.handle_in_context(
                &Key::Character("s".into()),
                Physical::Code(Code::KeyS),
                Modifiers::empty(),
                Some("s"),
                explorer,
            ),
            Action::ToggleActivity(Activity::Sessions)
        ));
    }

    #[test]
    fn pending_leader_suffix_is_global_across_modes() {
        for receiving_mode in [Mode::Normal, Mode::Insert, Mode::Visual, Mode::Terminal] {
            let mut bindings = Keybindings::default();
            press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
            bindings.mode = receiving_mode;

            assert!(matches!(
                press(&mut bindings, "s", Code::KeyS, Modifiers::empty(), false),
                Action::ToggleActivity(Activity::Sessions)
            ));
            assert_eq!(bindings.mode_label(), "NORMAL");
        }
    }

    #[test]
    fn normal_mode_leader_starts_while_composer_has_focus() {
        let mut bindings = Keybindings::default();
        let composer = DispatchContext {
            focused: KeybindingContext::Composer,
            composer_available: true,
            terminal_available: false,
        };

        assert_eq!(
            bindings.handle_in_context(
                &Key::Character(" ".into()),
                Physical::Code(Code::Space),
                Modifiers::empty(),
                Some(" "),
                composer,
            ),
            Action::None
        );
        assert!(bindings.is_leader_pending());
        assert_eq!(
            bindings.handle_in_context(
                &Key::Character("s".into()),
                Physical::Code(Code::KeyS),
                Modifiers::empty(),
                Some("s"),
                composer,
            ),
            Action::ToggleActivity(Activity::Sessions)
        );
    }

    #[test]
    fn agent_mode_never_interprets_space_as_leader() {
        let mut bindings = Keybindings {
            mode: Mode::Insert,
            ..Keybindings::default()
        };

        assert!(matches!(
            press(&mut bindings, " ", Code::Space, Modifiers::empty(), false),
            Action::AgentAppend(text) if text == " "
        ));
        assert_eq!(bindings.mode_label(), "INSERT");

        assert!(matches!(
            press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), false),
            Action::AgentAppend(text) if text == "t"
        ));
        assert_eq!(bindings.mode_label(), "INSERT");
    }

    #[test]
    fn agent_space_works_without_an_iced_text_payload() {
        let mut bindings = Keybindings {
            mode: Mode::Insert,
            ..Keybindings::default()
        };

        assert!(matches!(
            bindings.handle(
                &Key::Character(" ".into()),
                Physical::Code(Code::Space),
                Modifiers::empty(),
                None,
                false,
                true,
                false,
                false,
            ),
            Action::AgentAppend(text) if text == " "
        ));
    }

    #[test]
    fn agent_paste_uses_the_clipboard_action() {
        let mut bindings = Keybindings {
            mode: Mode::Insert,
            ..Keybindings::default()
        };

        assert!(matches!(
            press(&mut bindings, "v", Code::KeyV, Modifiers::CTRL, false),
            Action::AgentPaste
        ));
    }

    #[test]
    fn agent_control_a_selects_all() {
        let mut bindings = Keybindings {
            mode: Mode::Insert,
            ..Keybindings::default()
        };

        assert!(matches!(
            press(&mut bindings, "a", Code::KeyA, Modifiers::CTRL, false),
            Action::AgentSelectAll
        ));
    }

    #[test]
    fn terminal_mode_never_interprets_space_as_leader() {
        let mut bindings = Keybindings {
            mode: Mode::Terminal,
            ..Keybindings::default()
        };

        assert!(matches!(
            press(&mut bindings, " ", Code::Space, Modifiers::empty(), true),
            Action::TerminalInput(bytes) if bytes == b" "
        ));
        assert_eq!(bindings.mode_label(), "TERMINAL");

        assert!(matches!(
            press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), true),
            Action::TerminalInput(bytes) if bytes == b"t"
        ));
        assert_eq!(bindings.mode_label(), "TERMINAL");
    }

    #[test]
    fn terminal_shortcuts_are_forwarded() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), false);

        assert!(matches!(
            press(&mut bindings, "\\", Code::Backslash, Modifiers::CTRL, true),
            Action::TerminalInput(bytes) if bytes == vec![0x1c]
        ));
        assert!(matches!(
            press(&mut bindings, "n", Code::KeyN, Modifiers::CTRL, true),
            Action::TerminalInput(bytes) if bytes == vec![0x0e]
        ));
        assert_eq!(bindings.mode_label(), "TERMINAL");
    }

    #[test]
    fn escape_returns_to_normal_without_terminal_input() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), false);

        assert!(matches!(
            bindings.handle(
                &Key::Named(Named::Escape),
                Physical::Code(Code::Escape),
                Modifiers::empty(),
                None,
                true,
                false,
                false,
                false,
            ),
            Action::None
        ));
        assert_eq!(bindings.mode_label(), "NORMAL");
    }
}
