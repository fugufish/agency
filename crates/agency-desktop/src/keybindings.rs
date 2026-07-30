use crate::config::KeybindingConfig;
use iced::keyboard::{
    Key, Modifiers,
    key::{Code, Named, Physical},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Terminal,
    Composer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeIndicator {
    Normal,
    Terminal,
    Composer,
    Leader,
}

impl ModeIndicator {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Terminal => "TERMINAL",
            Self::Composer => "COMPOSER",
            Self::Leader => "LEADER",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Sessions,
    Explorer,
    Diffs,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    WorktreePrevious,
    WorktreeNext,
    WorktreeSelect(usize),
    ToggleActivity(Activity),
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
    AgentAppend(String),
    AgentBackspace,
    AgentPaste,
    AgentSelectAll,
    AgentSubmit,
    TerminalInput(Vec<u8>),
}

pub struct Keybindings {
    config: KeybindingConfig,
    mode: Mode,
    leader_pending: bool,
    leader_separator_pending: bool,
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
        }
    }

    pub fn is_composer_active(&self) -> bool {
        self.mode == Mode::Composer
    }

    pub fn is_leader_pending(&self) -> bool {
        self.leader_pending
    }

    pub fn show_sessions_hint(&self) -> &str {
        &self.config.show_sessions
    }

    pub fn show_explorer_hint(&self) -> &str {
        &self.config.show_explorer
    }

    pub fn show_diffs_hint(&self) -> &str {
        &self.config.show_diffs
    }

    pub fn toggle_terminal_hint(&self) -> &str {
        &self.config.toggle_terminal
    }

    pub fn enter_active_view_hint(&self) -> &str {
        &self.config.enter_active_view
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
                Mode::Terminal => ModeIndicator::Terminal,
                Mode::Composer => ModeIndicator::Composer,
            }
        }
    }

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
        if self.leader_pending {
            return self.handle_leader_suffix(
                key,
                physical_key,
                modifiers,
                terminal_visible,
                agent_visible,
            );
        }

        match self.mode {
            Mode::Normal => self.handle_normal(
                key,
                physical_key,
                modifiers,
                terminal_visible,
                agent_visible,
                toolbar_visible,
                explorer_active,
                diff_activity_visible,
                diff_viewer_visible,
            ),
            Mode::Terminal => self.handle_terminal(key, physical_key, modifiers, text),
            Mode::Composer => self.handle_agent(key, physical_key, modifiers, text),
        }
    }

    fn handle_normal(
        &mut self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
        terminal_visible: bool,
        agent_visible: bool,
        toolbar_visible: bool,
        explorer_active: bool,
        diff_activity_visible: bool,
        diff_viewer_visible: bool,
    ) -> Action {
        if modifiers.is_empty()
            && configured_character(key, physical_key, &self.config.leader, " ", Code::Space)
        {
            self.leader_pending = true;
            return Action::None;
        }

        if diff_viewer_visible
            && modifiers == Modifiers::CTRL
            && is_character(key, physical_key, "c", Code::KeyC)
        {
            return Action::DiffClose;
        }

        if diff_viewer_visible && modifiers.is_empty() {
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

        if diff_activity_visible {
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

        if toolbar_visible
            && !explorer_active
            && modifiers == Modifiers::SHIFT
            && is_character(key, physical_key, "g", Code::KeyG)
        {
            return Action::ToolbarLast;
        }

        if toolbar_visible && explorer_active && modifiers.is_empty() {
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

        if toolbar_visible && !explorer_active && modifiers.is_empty() {
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
            if !explorer_active && is_character(key, physical_key, "h", Code::KeyH) {
                return Action::WorktreePrevious;
            }
            if !explorer_active && is_character(key, physical_key, "l", Code::KeyL) {
                return Action::WorktreeNext;
            }
        }

        if modifiers.is_empty()
            && terminal_visible
            && configured_character(
                key,
                physical_key,
                &self.config.enter_active_view,
                "i",
                Code::KeyI,
            )
        {
            self.mode = Mode::Terminal;
        }
        if modifiers.is_empty()
            && agent_visible
            && configured_character(
                key,
                physical_key,
                &self.config.enter_active_view,
                "i",
                Code::KeyI,
            )
        {
            self.mode = Mode::Composer;
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
            &self.config.show_explorer,
            "e",
            Code::KeyE,
        ) {
            self.mode = Mode::Normal;
            return Action::ToggleActivity(Activity::Explorer);
        }
        if configured_character(
            key,
            physical_key,
            &self.config.show_sessions,
            "s",
            Code::KeyS,
        ) {
            self.mode = Mode::Normal;
            return Action::ToggleActivity(Activity::Sessions);
        }
        if configured_character(key, physical_key, &self.config.show_diffs, "d", Code::KeyD) {
            self.mode = Mode::Normal;
            return Action::ToggleActivity(Activity::Diffs);
        }
        if configured_character(key, physical_key, &self.config.new_session, "n", Code::KeyN) {
            self.mode = Mode::Composer;
            return Action::NewSession;
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
            self.mode = Mode::Composer;
            return Action::None;
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

        match key.as_ref() {
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
        }
    }
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
            _ => return None,
        })
    }

    #[test]
    fn normal_mode_vscode_keybindings_map_to_actions() {
        let normal = ModeIndicator::Normal;
        let composer = ModeIndicator::Composer;
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
                ("space d", Action::ToggleActivity(Activity::Diffs), normal),
                ("space n", Action::NewSession, composer),
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
        assert_mode_bindings(
            Mode::Normal,
            terminal_context,
            [("i", Action::None, terminal)],
        );
    }

    #[test]
    fn composer_mode_vscode_keybindings_map_to_actions() {
        let composer = ModeIndicator::Composer;
        let normal = ModeIndicator::Normal;
        let context = Context {
            agent_visible: true,
            ..Context::default()
        };

        assert_mode_bindings(
            Mode::Composer,
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
        bindings.handle(
            &Key::Character(character.into()),
            Physical::Code(code),
            modifiers,
            Some(character),
            terminal_open,
            false,
            false,
            false,
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
        assert_eq!(bindings.mode_label(), "COMPOSER");
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

        assert!(matches!(action, Action::None));
        assert_eq!(bindings.mode_label(), "COMPOSER");
    }

    #[test]
    fn agent_composer_accepts_input_while_explorer_is_open() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        bindings.handle(
            &Key::Character("i".into()),
            Physical::Code(Code::KeyI),
            Modifiers::empty(),
            Some("i"),
            false,
            true,
            true,
            true,
        );

        let action = bindings.handle(
            &Key::Character("h".into()),
            Physical::Code(Code::KeyH),
            Modifiers::empty(),
            Some("h"),
            false,
            true,
            true,
            true,
        );

        assert!(matches!(action, Action::AgentAppend(text) if text == "h"));
        assert_eq!(bindings.mode_label(), "COMPOSER");
    }

    #[test]
    fn leader_e_opens_explorer_and_leader_s_opens_sessions() {
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
        assert_eq!(bindings.mode_label(), "NORMAL");
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
    fn explorer_respects_composer_mode_then_configured_leader_in_normal_mode() {
        let config = KeybindingConfig {
            leader: ",".to_owned(),
            ..KeybindingConfig::default()
        };
        let mut bindings = Keybindings {
            mode: Mode::Composer,
            ..Keybindings::from_config(config)
        };

        assert!(matches!(
            bindings.handle(
                &Key::Character(",".into()),
                Physical::Code(Code::Comma),
                Modifiers::empty(),
                Some(","),
                false,
                true,
                true,
                true,
            ),
            Action::AgentAppend(text) if text == ","
        ));
        assert_eq!(bindings.mode_label(), "COMPOSER");

        assert!(matches!(
            bindings.handle(
                &Key::Named(Named::Escape),
                Physical::Code(Code::Escape),
                Modifiers::empty(),
                None,
                false,
                true,
                true,
                true,
            ),
            Action::None
        ));
        assert_eq!(bindings.mode_label(), "NORMAL");

        assert!(matches!(
            bindings.handle(
                &Key::Character(",".into()),
                Physical::Code(Code::Comma),
                Modifiers::empty(),
                Some(","),
                false,
                true,
                true,
                true,
            ),
            Action::None
        ));
        assert_eq!(bindings.mode_label(), "LEADER");

        assert!(matches!(
            bindings.handle(
                &Key::Character("s".into()),
                Physical::Code(Code::KeyS),
                Modifiers::empty(),
                Some("s"),
                false,
                true,
                true,
                true,
            ),
            Action::ToggleActivity(Activity::Sessions)
        ));
    }

    #[test]
    fn pending_leader_suffix_is_global_across_modes() {
        for receiving_mode in [Mode::Normal, Mode::Composer, Mode::Terminal] {
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
    fn agent_mode_never_interprets_space_as_leader() {
        let mut bindings = Keybindings {
            mode: Mode::Composer,
            ..Keybindings::default()
        };

        assert!(matches!(
            press(&mut bindings, " ", Code::Space, Modifiers::empty(), false),
            Action::AgentAppend(text) if text == " "
        ));
        assert_eq!(bindings.mode_label(), "COMPOSER");

        assert!(matches!(
            press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), false),
            Action::AgentAppend(text) if text == "t"
        ));
        assert_eq!(bindings.mode_label(), "COMPOSER");
    }

    #[test]
    fn agent_space_works_without_an_iced_text_payload() {
        let mut bindings = Keybindings {
            mode: Mode::Composer,
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
            mode: Mode::Composer,
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
            mode: Mode::Composer,
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
