use iced::widget::{button, container, markdown, svg};
use iced::{Border, Color, Font, Padding, Shadow, Theme, Vector, color, theme};

pub const BACKGROUND: Color = color!(0x1a1b26);
pub const SURFACE: Color = color!(0x1f2335);
pub const SURFACE_RAISED: Color = color!(0x292e42);
pub const SURFACE_SELECTED: Color = color!(0x2f4572);
pub const BORDER: Color = color!(0x3b4261);
pub const TEXT: Color = color!(0xc0caf5);
pub const DARK_TEXT: Color = color!(0x1a1b26);
pub const PRIMARY: Color = color!(0x7aa2f7);
pub const SUCCESS: Color = color!(0x9ece6a);
pub const WARNING: Color = color!(0xe0af68);
pub const DANGER: Color = color!(0xf7768e);

/// Maps syntax tokens onto the application palette so highlighted code stays
/// inside Agency's Tokyo Night theme instead of importing a foreign palette.
pub fn syntax_color(source: Option<Color>) -> Color {
    let Some(source) = source else {
        return TEXT;
    };
    if source.r > source.g * 1.18 {
        DANGER
    } else if source.g > source.r * 1.12 && source.g > source.b {
        SUCCESS
    } else if source.r > source.b && source.g > source.b {
        WARNING
    } else if source.b > source.r || source.b > source.g {
        PRIMARY
    } else {
        TEXT
    }
}

pub fn theme() -> Theme {
    Theme::custom(
        "Agency",
        theme::Palette {
            background: BACKGROUND,
            text: TEXT,
            primary: PRIMARY,
            success: SUCCESS,
            warning: WARNING,
            danger: DANGER,
        },
    )
}

pub fn rail() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(BACKGROUND)
}

pub fn activity_bar() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(BACKGROUND)
        .border(Border {
            color: BORDER,
            width: 1.0,
            radius: 0.0.into(),
        })
}

pub fn diff_line(kind: crate::diffs::DiffLineKind) -> container::Style {
    use crate::diffs::DiffLineKind;

    let (text, background) = match kind {
        DiffLineKind::Addition => (SUCCESS, Color { a: 0.12, ..SUCCESS }),
        DiffLineKind::Deletion => (DANGER, Color { a: 0.12, ..DANGER }),
        DiffLineKind::Hunk => (PRIMARY, Color { a: 0.16, ..PRIMARY }),
        DiffLineKind::Metadata => (WARNING, SURFACE_RAISED),
        DiffLineKind::Context => (TEXT, BACKGROUND),
    };
    container::Style::default()
        .color(text)
        .background(background)
}

pub fn diff_gutter() -> container::Style {
    container::Style::default()
        .color(PRIMARY)
        .background(SURFACE)
        .border(Border {
            color: BORDER,
            width: 0.0,
            radius: 0.0.into(),
        })
}

pub fn status_bar() -> container::Style {
    container::Style::default().color(TEXT).background(SURFACE)
}

pub fn tab_bar() -> container::Style {
    container::Style::default().color(TEXT).background(SURFACE)
}

pub fn worktree_tab(selected: bool, status: button::Status) -> button::Style {
    let background = if selected {
        SURFACE_SELECTED
    } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        SURFACE_RAISED
    } else {
        SURFACE
    };
    let mut style = button::Style::default().with_background(background);
    style.text_color = TEXT;
    style.border = Border {
        color: if selected {
            PRIMARY
        } else {
            Color::TRANSPARENT
        },
        width: 1.0,
        radius: 4.0.into(),
    };
    style
}

pub fn agent_badge() -> container::Style {
    container::Style::default()
        .color(PRIMARY)
        .background(SURFACE_SELECTED)
        .border(Border {
            color: PRIMARY,
            width: 1.0,
            radius: 4.0.into(),
        })
}

pub fn user_message() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(SURFACE_RAISED)
        .border(Border {
            color: BORDER,
            width: 1.0,
            radius: 6.0.into(),
        })
}

pub fn text_selection() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(SURFACE_SELECTED)
}

pub fn block_cursor() -> container::Style {
    container::Style::default().background(PRIMARY)
}

pub fn modal_backdrop() -> container::Style {
    container::Style::default().background(Color {
        a: 0.72,
        ..BACKGROUND
    })
}

pub fn modal() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(SURFACE)
        .border(Border {
            color: BORDER,
            width: 1.0,
            radius: 8.0.into(),
        })
}

/// Pointer-operable twin of [`agent_badge`] for the status bar's agent chip.
pub fn agent_chip(open: bool, status: button::Status) -> button::Style {
    let background = if open || matches!(status, button::Status::Hovered | button::Status::Pressed)
    {
        SURFACE_RAISED
    } else {
        SURFACE_SELECTED
    };
    let mut style = button::Style::default().with_background(background);
    style.text_color = PRIMARY;
    style.border = Border {
        color: PRIMARY,
        width: 1.0,
        radius: 4.0.into(),
    };
    style
}

/// Panel for a menu that floats above another surface rather than covering the
/// application the way [`modal`] does.
pub fn floating_menu() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(SURFACE)
        .border(Border {
            color: PRIMARY,
            width: 1.0,
            radius: 8.0.into(),
        })
}

pub fn menu_entry(selected: bool, status: button::Status) -> button::Style {
    let highlighted =
        selected || matches!(status, button::Status::Hovered | button::Status::Pressed);
    let background = if selected {
        SURFACE_SELECTED
    } else if highlighted {
        SURFACE_RAISED
    } else {
        SURFACE
    };
    let mut style = button::Style::default().with_background(background);
    style.text_color = TEXT;
    style.border = Border {
        color: if highlighted { PRIMARY } else { BORDER },
        width: 1.0,
        radius: 5.0.into(),
    };
    style
}

pub fn dialog_button(danger: bool, status: button::Status) -> button::Style {
    let accent = if danger { DANGER } else { PRIMARY };
    let background = if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        SURFACE_RAISED
    } else {
        SURFACE
    };
    let mut style = button::Style::default().with_background(background);
    style.text_color = if danger { DANGER } else { TEXT };
    style.border = Border {
        color: accent,
        width: 1.0,
        radius: 5.0.into(),
    };
    style
}

pub fn icon_button(status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => SURFACE_RAISED,
        button::Status::Active | button::Status::Disabled => SURFACE,
    };
    let mut style = button::Style::default().with_background(background);
    style.text_color = TEXT;
    style.border = Border {
        color: BORDER,
        width: 1.0,
        radius: 5.0.into(),
    };
    style
}

pub fn slash_command_button(selected_by_keyboard: bool, status: button::Status) -> button::Style {
    let selected =
        selected_by_keyboard || matches!(status, button::Status::Hovered | button::Status::Pressed);
    let mut style =
        button::Style::default().with_background(if selected { SURFACE_SELECTED } else { SURFACE });
    style.text_color = TEXT;
    style.border = Border {
        color: if selected { PRIMARY } else { BORDER },
        width: 1.0,
        radius: 5.0.into(),
    };
    style
}

pub fn agent_type_badge(codex: bool) -> container::Style {
    let color = if codex { PRIMARY } else { WARNING };
    container::Style::default()
        .color(color)
        .background(Color { a: 0.14, ..color })
        .border(Border {
            color,
            width: 1.0,
            radius: 4.0.into(),
        })
}

pub fn trash_button(status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let mut style = button::Style::default();
    if hovered {
        style = style.with_background(Color { a: 0.18, ..DANGER });
    }
    style.text_color = DANGER;
    style.border = Border {
        color: if hovered { DANGER } else { Color::TRANSPARENT },
        width: 1.0,
        radius: 4.0.into(),
    };
    style
}

pub fn icon() -> svg::Style {
    svg::Style { color: Some(TEXT) }
}

pub fn tool_icon(selected: bool) -> svg::Style {
    svg::Style {
        color: Some(if selected { PRIMARY } else { TEXT }),
    }
}

pub fn shortcut_badge() -> container::Style {
    container::Style::default()
        .color(DARK_TEXT)
        .background(PRIMARY)
        .border(Border {
            color: TEXT,
            width: 1.0,
            radius: 4.0.into(),
        })
}

pub fn counter_badge() -> container::Style {
    container::Style::default()
        .color(PRIMARY)
        .background(SURFACE_RAISED)
        .border(Border {
            color: PRIMARY,
            width: 1.0,
            radius: 6.0.into(),
        })
}

pub fn disclosure_icon() -> svg::Style {
    svg::Style {
        color: Some(PRIMARY),
    }
}

pub fn tree_item_icon(directory: bool) -> svg::Style {
    svg::Style {
        color: Some(if directory { WARNING } else { TEXT }),
    }
}

pub fn danger_icon() -> svg::Style {
    svg::Style {
        color: Some(DANGER),
    }
}

pub fn user_arrow() -> svg::Style {
    svg::Style {
        color: Some(PRIMARY),
    }
}

pub fn markdown_settings() -> markdown::Settings {
    markdown::Settings::with_text_size(
        15,
        markdown::Style {
            font: Font::default(),
            inline_code_highlight: markdown::Highlight {
                background: SURFACE_RAISED.into(),
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 4.0.into(),
                },
            },
            inline_code_padding: Padding {
                top: 1.0,
                right: 1.0,
                bottom: 1.0,
                left: 1.0,
            },
            inline_code_color: TEXT,
            inline_code_font: Font::MONOSPACE,
            code_block_font: Font::MONOSPACE,
            link_color: PRIMARY,
        },
    )
}

pub fn session_button(selected: bool, status: button::Status) -> button::Style {
    let background = if selected {
        SURFACE_SELECTED
    } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        SURFACE_RAISED
    } else {
        SURFACE
    };
    let mut style = button::Style::default().with_background(background);
    style.text_color = TEXT;
    style.border = Border {
        color: if selected { PRIMARY } else { BORDER },
        width: 1.0,
        radius: 6.0.into(),
    };
    if selected {
        style.shadow = Shadow {
            color: Color { a: 0.22, ..PRIMARY },
            offset: Vector::new(0.0, 1.0),
            blur_radius: 5.0,
        };
    }
    style
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Active,
    Thinking,
    Waiting,
    Idle,
    Resume,
}

pub fn agent_status_badge(status: AgentStatus, pulse: f32) -> container::Style {
    let color = match status {
        AgentStatus::Active => SUCCESS,
        AgentStatus::Thinking => WARNING,
        AgentStatus::Waiting => DANGER,
        AgentStatus::Idle => TEXT,
        AgentStatus::Resume => PRIMARY,
    };
    let animated = status == AgentStatus::Active;
    let background_alpha = if animated {
        0.12 + pulse.clamp(0.0, 1.0) * 0.18
    } else {
        0.08
    };
    let mut style = container::Style::default()
        .color(color)
        .background(Color {
            a: background_alpha,
            ..color
        })
        .border(Border {
            color,
            width: if animated { 1.5 } else { 1.0 },
            radius: 4.0.into(),
        });
    if animated {
        style.shadow = Shadow {
            color: Color {
                a: 0.12 + pulse.clamp(0.0, 1.0) * 0.2,
                ..color
            },
            offset: Vector::default(),
            blur_radius: 3.0 + pulse.clamp(0.0, 1.0) * 5.0,
        };
    }
    style
}

pub fn mcp_server_state_badge(state: crate::McpServerState) -> container::Style {
    let color = match state {
        crate::McpServerState::Connected => SUCCESS,
        crate::McpServerState::Error => DANGER,
        crate::McpServerState::RequiresAuthentication => WARNING,
    };
    container::Style::default()
        .color(color)
        .background(SURFACE_RAISED)
        .border(Border {
            color,
            width: 1.0,
            radius: 4.0.into(),
        })
}

pub fn mcp_access_badge() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(SURFACE_SELECTED)
        .border(Border {
            color: BORDER,
            width: 1.0,
            radius: 4.0.into(),
        })
}

pub fn mcp_agent_card() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(SURFACE)
        .border(Border {
            color: BORDER,
            width: 1.0,
            radius: 6.0.into(),
        })
}

pub fn tool_button(selected: bool, focused: bool, status: button::Status) -> button::Style {
    let background = if selected {
        SURFACE_SELECTED
    } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        SURFACE_RAISED
    } else {
        BACKGROUND
    };
    let mut style = button::Style::default().with_background(background);
    style.text_color = TEXT;
    style.border = Border {
        color: if selected || focused {
            PRIMARY
        } else {
            Color::TRANSPARENT
        },
        width: if focused { 2.0 } else { 1.0 },
        radius: 7.0.into(),
    };
    style
}

pub fn focus_surface(focused: bool) -> container::Style {
    container::Style::default().border(Border {
        color: if focused { PRIMARY } else { Color::TRANSPARENT },
        width: 1.0,
        radius: 2.0.into(),
    })
}

pub fn file_entry(selected: bool, status: button::Status) -> button::Style {
    let background = if selected {
        SURFACE_SELECTED
    } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        SURFACE_RAISED
    } else {
        BACKGROUND
    };
    let mut style = button::Style::default().with_background(background);
    style.text_color = TEXT;
    style.border = Border {
        color: Color::TRANSPARENT,
        width: 1.0,
        radius: 4.0.into(),
    };
    style
}

pub fn tree_root() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(SURFACE)
        .border(Border {
            color: BORDER,
            width: 1.0,
            radius: 5.0.into(),
        })
}

/// The card every reported activity uses — file changes, commands, reads, and
/// plugin installs — accented by how that activity ended.
pub fn status_card(status: &str) -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(SURFACE)
        .border(Border {
            color: status_accent(status),
            width: 1.0,
            radius: 7.0.into(),
        })
}

pub fn status_badge(status: &str) -> container::Style {
    let accent = status_accent(status);
    container::Style::default()
        .color(accent)
        .background(SURFACE_RAISED)
        .border(Border {
            color: accent,
            width: 1.0,
            radius: 4.0.into(),
        })
}

fn status_accent(status: &str) -> Color {
    const FINISHED: [&str; 3] = ["completed", "installed", "read"];
    const FAILED: [&str; 4] = ["failed", "error", "declined", "denied"];

    if FINISHED
        .iter()
        .any(|finished| status.eq_ignore_ascii_case(finished))
    {
        SUCCESS
    } else if FAILED
        .iter()
        .any(|failed| status.eq_ignore_ascii_case(failed))
    {
        DANGER
    } else {
        WARNING
    }
}

/// Terminal output, shown the way a terminal shows it.
pub fn terminal_output() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(BACKGROUND)
        .border(Border {
            color: BORDER,
            width: 1.0,
            radius: 5.0.into(),
        })
}

pub fn file_change_row() -> container::Style {
    container::Style::default()
        .color(TEXT)
        .background(BACKGROUND)
        .border(Border {
            color: BORDER,
            width: 1.0,
            radius: 5.0.into(),
        })
}

pub fn file_change_count() -> container::Style {
    container::Style::default()
        .color(PRIMARY)
        .background(SURFACE_RAISED)
        .border(Border {
            color: PRIMARY,
            width: 1.0,
            radius: 8.0.into(),
        })
}

pub fn file_change_badge(action: &str) -> container::Style {
    let color = if action.eq_ignore_ascii_case("created") {
        SUCCESS
    } else if action.eq_ignore_ascii_case("deleted") {
        DANGER
    } else if action.eq_ignore_ascii_case("moved") {
        WARNING
    } else {
        PRIMARY
    };
    container::Style::default()
        .color(color)
        .background(SURFACE_RAISED)
        .border(Border {
            color,
            width: 1.0,
            radius: 4.0.into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_tool_button_has_persistent_primary_highlight() {
        let style = tool_button(true, false, button::Status::Active);

        assert_eq!(style.background, Some(SURFACE_SELECTED.into()));
        assert_eq!(style.border.color, PRIMARY);
        assert_eq!(style.border.width, 1.0);
    }

    #[test]
    fn inactive_tool_button_has_no_highlight() {
        let style = tool_button(false, false, button::Status::Active);

        assert_eq!(style.background, Some(BACKGROUND.into()));
        assert_eq!(style.border.color, Color::TRANSPARENT);
    }
}
