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

/// Renders a warning banner when a file has been externally modified and auto-merge failed.
pub fn external_change_banner(externally_modified: bool, cx: &App) -> AnyElement {
  if externally_modified {
    let theme = cx.theme();
    div()
      .px_2()
      .py_1()
      .bg(theme.warning)
      .text_sm()
      .text_color(theme.warning_foreground)
      .child("Merge conflict \u{2014} press Cmd-R to reload from disk (unsaved edits will be lost)")
      .into_any_element()
  } else {
    div().into_any_element()
  }
}
