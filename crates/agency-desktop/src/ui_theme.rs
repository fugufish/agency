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

pub fn thinking_badge() -> container::Style {
    container::Style::default()
        .color(DARK_TEXT)
        .background(WARNING)
        .border(Border {
            color: WARNING,
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

#[derive(Debug, Clone, Copy)]
pub enum SessionStatus {
    Active,
    Running,
    Waiting,
    Resume,
}

pub fn session_status_badge(status: SessionStatus) -> container::Style {
    let color = match status {
        SessionStatus::Active => PRIMARY,
        SessionStatus::Running => SUCCESS,
        SessionStatus::Waiting => WARNING,
        SessionStatus::Resume => TEXT,
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

pub fn tool_button(selected: bool, status: button::Status) -> button::Style {
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
        radius: 7.0.into(),
    };
    style
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
