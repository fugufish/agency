# Agency contributor guidance

## UI color and theming

- All fixed application colors must come from
  `crates/agency-desktop/src/ui_theme.rs`. Do not add hexadecimal colors or
  one-off widget palettes in views or assets. User-configurable semantic mode
  colors belong in `config.rs` and are the only exception.
- New widgets must use the shared Agency theme or a semantic style helper from
  `ui_theme` (`rail`, `status_bar`, `icon_button`, `session_button`, etc.).
- Keep every surface in the same Tokyo Night hierarchy: `BACKGROUND` for the
  application canvas and rails, `SURFACE` for controls and bars,
  `SURFACE_RAISED` for hover/pressed states, and `SURFACE_SELECTED` for selection.
- Text on dark surfaces uses `TEXT`. Interactive focus and selection use
  `PRIMARY`; borders use `BORDER`. Success, warning, and error states use their
  matching semantic tokens.
- Icons and their surrounding controls must have the same background, border,
  hover, pressed, and focus treatment as equivalent text controls.
- Never rely on a light-system default inside the dark application theme.
  Check enabled, hovered, pressed, focused/selected, and disabled states when
  adding an element.
- Preserve readable contrast: normal text should target WCAG AA (4.5:1), and
  large text, icons, borders, and focus indicators should target at least 3:1.

## Confirmation modals

- Destructive actions must open a confirmation modal that names the affected
  item and clearly labels the destructive button.
- While a modal is open, it owns input and blocks interaction with content
  behind it. Enter confirms the primary action and Escape cancels and closes
  the modal. Always provide equivalent visible pointer controls.
- Confirmation modals must use shared semantic styles from `ui_theme`; use the
  danger token for destructive actions.
