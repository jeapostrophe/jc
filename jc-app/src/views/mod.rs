pub mod close_confirm;
pub mod code_view;
pub mod keybinding_help;
pub mod pane;
pub mod picker;
pub mod project_state;
pub mod session_state;
pub mod todo_view;
pub mod workspace;

use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::input::InputState;

use crate::language::Language;

/// Trait for views that support line search via `LineSearchPickerDelegate`.
pub trait LineSearchable: Sized + 'static {
  fn editor_text(&self, cx: &App) -> String;
  fn language_name(&self) -> Language;
  fn scroll_to_line(&self, line: u32, window: &mut Window, cx: &mut Context<Self>);
}

/// Reads the full text content from an editor widget.
pub fn editor_text(editor: &Entity<InputState>, cx: &App) -> String {
  editor.read(cx).value().as_ref().to_string()
}

/// Scrolls an editor widget so the given 0-based `line` is approximately centered.
pub fn scroll_editor_to_line<V: 'static>(
  editor: &Entity<InputState>,
  line: u32,
  window: &mut Window,
  cx: &mut Context<V>,
) {
  editor.update(cx, |editor, cx| {
    editor.set_cursor_position(gpui_component::input::Position::new(line, 0), window, cx);
    editor.scroll_to_center_line(line, cx);
  });
}

/// One warning line over an editor pane: a conflict, or a save that failed
/// (`SharedFile::banner`), else nothing.
pub fn file_banner(text: Option<String>, cx: &App) -> AnyElement {
  let Some(text) = text else { return div().into_any_element() };
  let theme = cx.theme();
  div()
    .px_2()
    .py_1()
    .bg(theme.warning)
    .text_sm()
    .text_color(theme.warning_foreground)
    .child(text)
    .into_any_element()
}
