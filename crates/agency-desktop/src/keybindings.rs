use iced::keyboard::{
    Key, Modifiers,
    key::{Code, Named, Physical},
};
use std::time::{Duration, Instant};

use crate::config::KeybindingConfig;

const ESCAPE_TIMEOUT: Duration = Duration::from_millis(150);

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
    Escape,
}

impl ModeIndicator {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Terminal => "TERMINAL",
            Self::Composer => "COMPOSER",
            Self::Leader => "LEADER",
            Self::Escape => "ESC…",
        }
    }
}

pub enum Action {
    None,
    ShowSessions,
    ShowExplorer,
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
    terminal_prefix_pending: bool,
    escape_pending_at: Option<Instant>,
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
            terminal_prefix_pending: false,
            escape_pending_at: None,
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

    pub fn enter_active_view_hint(&self) -> &str {
        &self.config.enter_active_view
    }

    #[cfg(test)]
    pub fn mode_label(&self) -> &'static str {
        self.mode_indicator().label()
    }

    pub fn mode_indicator(&self) -> ModeIndicator {
        if self.escape_pending_at.is_some() {
            ModeIndicator::Escape
        } else if self.leader_pending {
            ModeIndicator::Leader
        } else {
            match self.mode {
                Mode::Normal => ModeIndicator::Normal,
                Mode::Terminal => ModeIndicator::Terminal,
                Mode::Composer => ModeIndicator::Composer,
            }
        }
    }

    pub fn tick(&mut self, now: Instant) -> Action {
        if self
            .escape_pending_at
            .is_some_and(|started| now.duration_since(started) >= ESCAPE_TIMEOUT)
        {
            self.escape_pending_at = None;
            Action::TerminalInput(vec![0x1b])
        } else {
            Action::None
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
    ) -> Action {
        if modifiers.is_empty()
            && configured_character(key, physical_key, &self.config.leader, " ", Code::Space)
        {
            self.leader_pending = true;
            return Action::None;
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
        if configured_character(
            key,
            physical_key,
            &self.config.show_explorer,
            "e",
            Code::KeyE,
        ) {
            self.mode = Mode::Normal;
            return Action::ShowExplorer;
        }
        if configured_character(
            key,
            physical_key,
            &self.config.show_sessions,
            "s",
            Code::KeyS,
        ) {
            self.mode = Mode::Normal;
            return Action::ShowSessions;
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
            self.mode = if terminal_visible {
                Mode::Normal
            } else {
                Mode::Terminal
            };
            return Action::ToggleTerminal;
        }
        Action::None
    }

    fn handle_terminal(
        &mut self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> Action {
        if let Some(started) = self.escape_pending_at.take() {
            if matches!(key.as_ref(), Key::Named(Named::Escape)) {
                if started.elapsed() < ESCAPE_TIMEOUT {
                    self.mode = Mode::Normal;
                    return Action::None;
                }

                self.escape_pending_at = Some(Instant::now());
                return Action::TerminalInput(vec![0x1b]);
            }

            let mut bytes = vec![0x1b];
            bytes.extend(terminal_bytes(key, modifiers, text));
            return Action::TerminalInput(bytes);
        }

        if matches!(key.as_ref(), Key::Named(Named::Escape)) {
            self.escape_pending_at = Some(Instant::now());
            return Action::None;
        }

        if self.terminal_prefix_pending {
            self.terminal_prefix_pending = false;

            if modifiers.control() && is_character(key, physical_key, "n", Code::KeyN) {
                self.mode = Mode::Normal;
                return Action::None;
            }

            let mut bytes = vec![0x1c];
            bytes.extend(terminal_bytes(key, modifiers, text));
            return Action::TerminalInput(bytes);
        }

        if modifiers.control() && is_character(key, physical_key, "\\", Code::Backslash) {
            self.terminal_prefix_pending = true;
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
            Action::ShowExplorer
        ));

        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        assert!(matches!(
            press(&mut bindings, "s", Code::KeyS, Modifiers::empty(), false),
            Action::ShowSessions
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
            Action::ShowSessions
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
                Action::ShowSessions
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
    fn terminal_escape_chord_returns_to_normal_mode() {
        let mut bindings = Keybindings::default();
        press(&mut bindings, " ", Code::Space, Modifiers::empty(), false);
        press(&mut bindings, "t", Code::KeyT, Modifiers::empty(), false);
        press(&mut bindings, "\\", Code::Backslash, Modifiers::CTRL, true);
        press(&mut bindings, "n", Code::KeyN, Modifiers::CTRL, true);

        assert_eq!(bindings.mode_label(), "NORMAL");
    }

    #[test]
    fn double_escape_returns_to_normal_without_terminal_input() {
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
        assert_eq!(bindings.mode_label(), "ESC…");

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

    #[test]
    fn single_escape_is_forwarded_after_timeout() {
        let mut bindings = Keybindings {
            mode: Mode::Terminal,
            escape_pending_at: Some(Instant::now() - ESCAPE_TIMEOUT),
            ..Keybindings::default()
        };

        assert!(matches!(
            bindings.tick(Instant::now()),
            Action::TerminalInput(bytes) if bytes == vec![0x1b]
        ));
        assert_eq!(bindings.mode_label(), "TERMINAL");
    }
}
