use std::fs;
use std::path::{Path, PathBuf};

use iced::Color;
use serde::{Deserialize, Serialize};

pub const WORKSPACE_CONFIG_DIRECTORY: &str = ".agency";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GlobalConfig {
    pub default_agent: DefaultAgent,
    pub keybindings: KeybindingConfig,
    pub mode_colors: ModeColorConfig,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DefaultAgent {
    #[default]
    Codex,
    Claude,
}

impl GlobalConfig {
    pub fn load() -> Result<Self, String> {
        let path = global_config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let source = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        toml::from_str(&source)
            .map_err(|error| format!("Could not parse {}: {error}", path.display()))
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub maximized: bool,
    pub fullscreen: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1_200.0,
            height: 760.0,
            x: None,
            y: None,
            maximized: false,
            fullscreen: false,
        }
    }
}

impl WindowState {
    pub fn load() -> Result<Self, String> {
        let path = window_state_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let source = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        let state: Self = toml::from_str(&source)
            .map_err(|error| format!("Could not parse {}: {error}", path.display()))?;

        if !state.width.is_finite()
            || !state.height.is_finite()
            || state.width < 800.0
            || state.height < 520.0
            || state.x.is_some_and(|value| !value.is_finite())
            || state.y.is_some_and(|value| !value.is_finite())
        {
            return Err(format!(
                "Window state in {} contains invalid geometry",
                path.display()
            ));
        }

        Ok(state)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = window_state_path()?;
        let directory = path
            .parent()
            .ok_or_else(|| format!("Invalid window state path: {}", path.display()))?;
        fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
        let source = toml::to_string_pretty(self)
            .map_err(|error| format!("Could not serialize window state: {error}"))?;
        fs::write(&path, source)
            .map_err(|error| format!("Could not write {}: {error}", path.display()))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeybindingConfig {
    pub leader: String,
    pub show_explorer: String,
    pub show_sessions: String,
    pub new_session: String,
    pub toggle_terminal: String,
    pub enter_active_view: String,
}

impl Default for KeybindingConfig {
    fn default() -> Self {
        Self {
            leader: " ".to_owned(),
            show_explorer: "e".to_owned(),
            show_sessions: "s".to_owned(),
            new_session: "n".to_owned(),
            toggle_terminal: "t".to_owned(),
            enter_active_view: "i".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModeColorConfig {
    pub normal: String,
    pub terminal: String,
    pub agent: String,
    pub leader: String,
    pub escape: String,
}

impl Default for ModeColorConfig {
    fn default() -> Self {
        Self {
            normal: "#7a88cf".to_owned(),
            terminal: "#9ece6a".to_owned(),
            agent: "#7dcfff".to_owned(),
            leader: "#e0af68".to_owned(),
            escape: "#f7768e".to_owned(),
        }
    }
}

pub struct ModeColors {
    pub normal: Color,
    pub terminal: Color,
    pub agent: Color,
    pub leader: Color,
    pub escape: Color,
}

impl ModeColors {
    pub fn from_config(config: &ModeColorConfig) -> Self {
        let defaults = ModeColorConfig::default();

        Self {
            normal: configured_color("AGENCY_MODE_COLOR_NORMAL", &config.normal, &defaults.normal),
            terminal: configured_color(
                "AGENCY_MODE_COLOR_TERMINAL",
                &config.terminal,
                &defaults.terminal,
            ),
            agent: configured_color("AGENCY_MODE_COLOR_AGENT", &config.agent, &defaults.agent),
            leader: configured_color("AGENCY_MODE_COLOR_LEADER", &config.leader, &defaults.leader),
            escape: configured_color("AGENCY_MODE_COLOR_ESCAPE", &config.escape, &defaults.escape),
        }
    }
}

fn configured_color(environment: &str, configured: &str, fallback: &str) -> Color {
    std::env::var(environment)
        .ok()
        .as_deref()
        .and_then(parse_hex_color)
        .or_else(|| parse_hex_color(configured))
        .or_else(|| parse_hex_color(fallback))
        .expect("built-in mode colors must be valid")
}

pub fn global_config_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("AGENCY_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("config.toml"));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("agency/config.toml"));
    }
    if cfg!(windows)
        && let Some(path) = std::env::var_os("APPDATA")
    {
        return Ok(PathBuf::from(path).join("Agency/config.toml"));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".config/agency/config.toml"))
        .ok_or_else(|| "Could not determine the global Agency config directory".to_owned())
}

fn window_state_path() -> Result<PathBuf, String> {
    global_config_path().map(|path| path.with_file_name("window-state.toml"))
}

pub fn workspace_config_directory(workspace: &Path) -> PathBuf {
    workspace.join(WORKSPACE_CONFIG_DIRECTORY)
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() != 6 {
        return None;
    }

    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::from_rgb8(red, green, blue))
}

#[cfg(test)]
mod tests {
    use super::{
        DefaultAgent, GlobalConfig, WindowState, parse_hex_color, workspace_config_directory,
    };
    use std::path::Path;

    #[test]
    fn parses_partial_global_config_over_defaults() {
        let config: GlobalConfig = toml::from_str(
            r##"
                default_agent = "claude"

                [mode_colors]
                agent = "#ffffff"
            "##,
        )
        .unwrap();

        assert_eq!(config.default_agent, DefaultAgent::Claude);
        assert_eq!(config.keybindings.show_explorer, "e");
        assert_eq!(config.keybindings.toggle_terminal, "t");
        assert_eq!(config.mode_colors.agent, "#ffffff");
    }

    #[test]
    fn parses_partial_window_state_over_defaults() {
        let state: WindowState = toml::from_str(
            r#"
                width = 1440.0
                height = 900.0
                maximized = true
            "#,
        )
        .unwrap();

        assert_eq!(state.width, 1440.0);
        assert_eq!(state.height, 900.0);
        assert_eq!(state.x, None);
        assert!(state.maximized);
        assert!(!state.fullscreen);
    }

    #[test]
    fn workspace_configuration_lives_in_dot_agency() {
        assert_eq!(
            workspace_config_directory(Path::new("/work/project")),
            Path::new("/work/project/.agency")
        );
    }

    #[test]
    fn parses_hash_prefixed_and_plain_rgb_hex_colors() {
        assert_eq!(
            parse_hex_color("#7dcfff"),
            Some(iced::Color::from_rgb8(0x7d, 0xcf, 0xff))
        );
        assert_eq!(
            parse_hex_color("9ECE6A"),
            Some(iced::Color::from_rgb8(0x9e, 0xce, 0x6a))
        );
    }

    #[test]
    fn rejects_invalid_colors() {
        assert_eq!(parse_hex_color("#fff"), None);
        assert_eq!(parse_hex_color("not-a-color"), None);
    }
}
